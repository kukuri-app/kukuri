-- #1702: 信頼値の照会を、対象ごとの集計と basis のページで返す（ADR 0026 §10）。
--
-- 対象（利用者の pubkey）ごとに、生きている risk signal（利用者が対象の行と、著者の対応を通した内容の
-- 行）を trust_target_signals に 1 行ずつ置き、その追加・削除を trust_target_totals へ差分で足し引きする。
-- risk_signals と risk_signal_subject_authors の変更は trigger が trust_target_signals へ写す。書込みの
-- 時点で期限を過ぎた行は入れず、後から期限を過ぎた行は cn-user-api の掃除（1 分ごと）が消す。

-- 集計に使う相対成分の半減期（日）。cn-user-api が起動時に COMMUNITY_NODE_TRUST_RELATIVE_HALF_LIFE_DAYS と
-- 揃え、違う半減期で作った集計は背景で作り直す。
CREATE TABLE cn_safety.trust_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    relative_half_life_days DOUBLE PRECISION NOT NULL CHECK (relative_half_life_days > 0)
);

INSERT INTO cn_safety.trust_settings (relative_half_life_days) VALUES (30);

CREATE TABLE cn_safety.trust_target_signals (
    target_pubkey TEXT NOT NULL,
    signal_id TEXT NOT NULL,
    -- 絶対成分（csam / cse / grooming）か。basis の並び（絶対成分が先）にも使う。
    absolute BOOLEAN NOT NULL,
    persisted_at TIMESTAMPTZ NOT NULL,
    -- 保持期間と失効時刻の早い方。過ぎた行は掃除が消す。
    removal_at TIMESTAMPTZ NOT NULL,
    -- 寄与の大きさ（1/1000 単位。cleared と nsfw / objectionable は 0）。
    units INTEGER NOT NULL CHECK (units >= 0),
    -- pull で開示できる範囲（confirmed の絶対成分で cleared でない行の visibility）。開示しない行は NULL。
    disclosure TEXT CHECK (disclosure IN ('public', 'subscribed_nodes')),
    -- trust_version の元（signal id・appeal 状態・operator の確定時刻・失効時刻の hash）。
    digest BIGINT NOT NULL,
    PRIMARY KEY (target_pubkey, signal_id)
);

CREATE INDEX trust_target_signals_page
    ON cn_safety.trust_target_signals (target_pubkey, absolute, persisted_at, signal_id);
CREATE INDEX trust_target_signals_disclosed
    ON cn_safety.trust_target_signals (target_pubkey, persisted_at, signal_id)
    WHERE disclosure IS NOT NULL;
CREATE INDEX trust_target_signals_disclosed_public
    ON cn_safety.trust_target_signals (target_pubkey, persisted_at, signal_id)
    WHERE disclosure = 'public';
CREATE INDEX trust_target_signals_signal ON cn_safety.trust_target_signals (signal_id);
CREATE INDEX trust_target_signals_removal ON cn_safety.trust_target_signals (removal_at);

-- 対象ごとの集計。相対成分は relative_at まで半減期 half_life_days で減衰させた和を持ち、照会時に
-- 照会の時刻まで減衰させる。行が 0 になった対象の行は消す。
CREATE TABLE cn_safety.trust_target_totals (
    target_pubkey TEXT PRIMARY KEY,
    signals BIGINT NOT NULL,
    absolute_units BIGINT NOT NULL,
    relative_units DOUBLE PRECISION NOT NULL,
    -- relative_units に入っている項の数。0 になったら和を 0 に戻す（足し引きの丸めを残さない）。
    relative_terms BIGINT NOT NULL,
    relative_at TIMESTAMPTZ NOT NULL,
    half_life_days DOUBLE PRECISION NOT NULL,
    disclosed_public_units BIGINT NOT NULL,
    disclosed_subscribed_units BIGINT NOT NULL,
    -- trust_target_signals の digest の XOR。
    digest BIGINT NOT NULL
);

CREATE INDEX trust_target_totals_half_life ON cn_safety.trust_target_totals (half_life_days);

-- 失効時刻（RFC3339 の文字列）。無ければ無期限、読めない値は期限切れとして扱う。保存前の検証（#700）が
-- RFC3339 以外を拒否するので、Postgres が chrono より緩く読む形は書き込まれない。
CREATE FUNCTION cn_safety.trust_expires_at(value TEXT) RETURNS TIMESTAMPTZ
LANGUAGE plpgsql STABLE AS $$
BEGIN
    RETURN COALESCE(value::timestamptz, 'infinity');
EXCEPTION WHEN others THEN
    RETURN '-infinity';
END;
$$;

-- risk signal 1 行の寄与の大きさ（1/1000 単位）。cn-trust の signal_contribution と同じ
-- （severity の大きさ × confidence、nsfw / objectionable と cleared は 0）。
CREATE FUNCTION cn_safety.trust_signal_units(
    category TEXT, severity TEXT, confidence SMALLINT, appeal_status TEXT
) RETURNS INTEGER LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE
        WHEN appeal_status = 'cleared' OR category IN ('nsfw', 'objectionable') THEN 0
        ELSE CASE severity
                 WHEN 'critical' THEN 10 WHEN 'high' THEN 7 WHEN 'medium' THEN 4 WHEN 'low' THEN 2
             END * LEAST(COALESCE(confidence, 100), 100)
    END
$$;

-- 時刻は epoch 秒にして、接続ごとのタイムゾーンの設定によらない値にする。
CREATE FUNCTION cn_safety.trust_signal_digest(
    id TEXT, appeal_status TEXT, operator_adjusted_at TIMESTAMPTZ, expires_at TEXT
) RETURNS BIGINT LANGUAGE sql STABLE AS $$
    SELECT ('x' || left(md5(concat(id, '|', COALESCE(appeal_status, 'none'), '|',
        COALESCE(extract(epoch FROM operator_adjusted_at)::text, ''), '|',
        COALESCE(expires_at, ''))), 16))::bit(64)::bigint
$$;

-- risk signal 1 行を、対象 target_pubkey の trust_target_signals の行にする。期限を過ぎていれば行を返さない。
CREATE FUNCTION cn_safety.trust_target_entry(signal cn_safety.risk_signals, target_pubkey TEXT)
RETURNS SETOF cn_safety.trust_target_signals LANGUAGE sql STABLE AS $$
    SELECT entry.*
    FROM (
        SELECT
            target_pubkey,
            signal.id,
            signal.category IN ('csam', 'cse', 'grooming') AS absolute,
            signal.persisted_at,
            LEAST(signal.retention_expires_at, cn_safety.trust_expires_at(signal.expires_at)) AS removal_at,
            cn_safety.trust_signal_units(
                signal.category, signal.severity, signal.confidence, signal.appeal_status),
            CASE WHEN signal.category IN ('csam', 'cse', 'grooming')
                      AND signal.basis IN ('known_hash_match', 'provider_verdict')
                      AND signal.visibility IN ('public', 'subscribed_nodes')
                      AND signal.appeal_status IS DISTINCT FROM 'cleared'
                 THEN signal.visibility END,
            cn_safety.trust_signal_digest(
                signal.id, signal.appeal_status, signal.operator_adjusted_at, signal.expires_at)
    ) AS entry (target_pubkey, signal_id, absolute, persisted_at, removal_at, units, disclosure, digest)
    WHERE entry.removal_at > now()
$$;

-- risk signal 1 行の、すべての対象（利用者が対象なら target_id、内容なら著者の対応）の行。
CREATE FUNCTION cn_safety.trust_signal_entries(signal cn_safety.risk_signals)
RETURNS SETOF cn_safety.trust_target_signals LANGUAGE sql STABLE AS $$
    SELECT entry.*
    FROM (
        SELECT signal.target_id WHERE signal.target = 'user_pubkey'
        UNION ALL
        SELECT author_pubkey FROM cn_safety.risk_signal_subject_authors
        WHERE target = signal.target AND target_id = signal.target_id
    ) AS targets (target_pubkey)
    CROSS JOIN LATERAL cn_safety.trust_target_entry(signal, targets.target_pubkey) AS entry
    ORDER BY entry.target_pubkey
$$;

CREATE FUNCTION cn_safety.trust_risk_signal_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        DELETE FROM cn_safety.trust_target_signals WHERE signal_id = OLD.id;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        -- 内容の著者の対応を読む前に、対応の追加・削除（#1699 の回収の trigger）と同じ advisory lock を
        -- 共有で取る。期限削除が同じ内容の最後の行を消す取引と重なっても、対応の有無を確定した順に読む。
        IF NEW.target IN ('post_id', 'blob_cid') THEN
            PERFORM pg_advisory_xact_lock_shared(
                hashtextextended('cn_safety.risk_signal_subject_authors', 0));
        END IF;
        INSERT INTO cn_safety.trust_target_signals
        SELECT * FROM cn_safety.trust_signal_entries(NEW);
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER trust_risk_signal_inserted_or_deleted
    AFTER INSERT OR DELETE ON cn_safety.risk_signals
    FOR EACH ROW EXECUTE FUNCTION cn_safety.trust_risk_signal_changed();

-- 保持期間の書き直し（毎時の全行の UPDATE）は値が変わった行だけが動かす。
CREATE TRIGGER trust_risk_signal_updated
    AFTER UPDATE OF target, target_id, category, severity, basis, visibility, confidence,
        expires_at, appeal_status, persisted_at, retention_expires_at, operator_adjusted_at
    ON cn_safety.risk_signals
    FOR EACH ROW WHEN (
        (OLD.target, OLD.target_id, OLD.category, OLD.severity, OLD.basis, OLD.visibility,
         OLD.confidence, OLD.expires_at, OLD.appeal_status, OLD.persisted_at,
         OLD.retention_expires_at, OLD.operator_adjusted_at)
        IS DISTINCT FROM
        (NEW.target, NEW.target_id, NEW.category, NEW.severity, NEW.basis, NEW.visibility,
         NEW.confidence, NEW.expires_at, NEW.appeal_status, NEW.persisted_at,
         NEW.retention_expires_at, NEW.operator_adjusted_at))
    EXECUTE FUNCTION cn_safety.trust_risk_signal_changed();

CREATE FUNCTION cn_safety.trust_subject_author_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM cn_safety.trust_target_signals entry
        USING cn_safety.risk_signals signal
        WHERE signal.target = OLD.target AND signal.target_id = OLD.target_id
          AND entry.target_pubkey = OLD.author_pubkey AND entry.signal_id = signal.id;
    ELSE
        INSERT INTO cn_safety.trust_target_signals
        SELECT entry.*
        FROM cn_safety.risk_signals signal
        CROSS JOIN LATERAL cn_safety.trust_target_entry(signal, NEW.author_pubkey) AS entry
        WHERE signal.target = NEW.target AND signal.target_id = NEW.target_id;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER trust_subject_author_changed
    AFTER INSERT OR DELETE ON cn_safety.risk_signal_subject_authors
    FOR EACH ROW EXECUTE FUNCTION cn_safety.trust_subject_author_changed();

-- trust_target_signals の 1 行の追加・削除を、その対象の集計へ足し引きする。
CREATE FUNCTION cn_safety.trust_target_signal_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    entry cn_safety.trust_target_signals;
    delta INTEGER;
    totals cn_safety.trust_target_totals;
BEGIN
    IF TG_OP = 'INSERT' THEN
        entry := NEW;
        delta := 1;
    ELSE
        entry := OLD;
        delta := -1;
    END IF;
    -- 行の無い対象は作ってから lock する。同時に 0 行になって消えた行は作り直す。
    LOOP
        SELECT * INTO totals FROM cn_safety.trust_target_totals
        WHERE target_pubkey = entry.target_pubkey FOR UPDATE;
        EXIT WHEN FOUND;
        INSERT INTO cn_safety.trust_target_totals
        SELECT entry.target_pubkey, 0, 0, 0, 0, entry.persisted_at, relative_half_life_days, 0, 0, 0
        FROM cn_safety.trust_settings
        ON CONFLICT (target_pubkey) DO NOTHING;
    END LOOP;
    totals.signals := totals.signals + delta;
    totals.digest := totals.digest # entry.digest;
    IF entry.absolute THEN
        totals.absolute_units := totals.absolute_units + delta * entry.units;
        IF entry.disclosure = 'public' THEN
            totals.disclosed_public_units := totals.disclosed_public_units + delta * entry.units;
        ELSIF entry.disclosure = 'subscribed_nodes' THEN
            totals.disclosed_subscribed_units := totals.disclosed_subscribed_units + delta * entry.units;
        END IF;
    ELSIF entry.units > 0 THEN
        IF entry.persisted_at > totals.relative_at THEN
            totals.relative_units := totals.relative_units * power(0.5::float8,
                extract(epoch FROM entry.persisted_at - totals.relative_at)::float8
                    / 86400 / totals.half_life_days);
            totals.relative_at := entry.persisted_at;
        END IF;
        totals.relative_terms := totals.relative_terms + delta;
        totals.relative_units := CASE WHEN totals.relative_terms = 0 THEN 0
            ELSE totals.relative_units + delta * entry.units * power(0.5::float8,
                extract(epoch FROM totals.relative_at - entry.persisted_at)::float8
                    / 86400 / totals.half_life_days)
        END;
    END IF;
    IF totals.signals = 0 THEN
        DELETE FROM cn_safety.trust_target_totals WHERE target_pubkey = entry.target_pubkey;
    ELSE
        UPDATE cn_safety.trust_target_totals SET
            signals = totals.signals,
            absolute_units = totals.absolute_units,
            relative_units = totals.relative_units,
            relative_terms = totals.relative_terms,
            relative_at = totals.relative_at,
            disclosed_public_units = totals.disclosed_public_units,
            disclosed_subscribed_units = totals.disclosed_subscribed_units,
            digest = totals.digest
        WHERE target_pubkey = entry.target_pubkey;
    END IF;
    RETURN NULL;
END;
$$;

-- 集計の行の lock は取引の最後に取る。書込みの取引が先に取る lock（著者の対応の advisory lock 等）と、
-- 集計の行の lock の順が取引ごとに逆にならないようにする。
CREATE CONSTRAINT TRIGGER trust_target_signal_changed
    AFTER INSERT OR DELETE ON cn_safety.trust_target_signals
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION cn_safety.trust_target_signal_changed();

-- 移行時の 1 回だけ、既存の生きている行から作る（集計は上の trigger が足す）。
INSERT INTO cn_safety.trust_target_signals
SELECT entry.*
FROM cn_safety.risk_signals signal
CROSS JOIN LATERAL cn_safety.trust_signal_entries(signal) AS entry;

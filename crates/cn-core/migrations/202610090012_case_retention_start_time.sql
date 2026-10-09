-- #1704 AC-1: 案件保持の期限を行に持たず、起算点の列と保持区分ごとの日数で判定する（ADR 0034 §1・§4）。
-- 日数は cn-user-api の起動時と `cn-cli retention sweep` が operator config から書く。日数を変えると
-- 既存の行にもすぐ効き、行は書き直さない。下の既定値は operator config の既定と同じ。
-- 旧平文の確認（202610090006）より後に適用する。確認で止まった DB は #1705 より前の版を一度起動して sealing
-- してから適用し直すので、その版が書く期限の列を、確認を通るまで消さない。信頼値の集計（202610090011）が
-- 期限の列を使う関数と trigger も、ここで起算点と日数の判定に置き換える。

CREATE TABLE cn_admin.retention_days (
    category TEXT PRIMARY KEY,
    days INTEGER NOT NULL CHECK (days > 0)
);
INSERT INTO cn_admin.retention_days (category, days) VALUES
    ('report', 180),
    ('report_contact', 90),
    ('tester_feedback', 180),
    ('rights_request_active', 730),
    ('rights_request_resolved', 365),
    ('rights_request_rejected', 180),
    ('rights_request_contact', 180),
    ('rights_request_identity', 180),
    ('rights_request_evidence', 180),
    ('rights_request_history', 365),
    ('operator_audit', 365),
    ('moderation_event', 180),
    ('risk_signal', 180);

-- 保持区分の期間。読取りは「起算点 > 基準時刻 − 期間」、期限削除は「起算点 <= 基準時刻 − 期間」で判定する。
CREATE FUNCTION cn_admin.retention_interval(TEXT) RETURNS INTERVAL
LANGUAGE sql STABLE AS $$
    SELECT make_interval(days => days) FROM cn_admin.retention_days WHERE category = $1
$$;

-- 権利侵害申出の状態の保持区分（措置済み・却下等・未解決）。起算点は最終の状態遷移の時刻（updated_at）。
CREATE FUNCTION cn_legal.rights_request_retention(TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE
        WHEN $1 = 'actioned' THEN 'rights_request_resolved'
        WHEN $1 IN ('declined', 'out_of_scope', 'withdrawn') THEN 'rights_request_rejected'
        ELSE 'rights_request_active'
    END
$$;

-- 信頼値の集計（#1702）の行の removal_at は、運営者が付けた失効時刻だけにする。保持期間は保存時刻と日数で
-- 判定し（書込みの時点はこの関数、後からは cn-user-api の掃除）、日数を変えても集計の行は書き直さない。
CREATE OR REPLACE FUNCTION cn_safety.trust_target_entry(signal cn_safety.risk_signals, target_pubkey TEXT)
RETURNS SETOF cn_safety.trust_target_signals LANGUAGE sql STABLE AS $$
    SELECT entry.*
    FROM (
        SELECT
            target_pubkey,
            signal.id,
            signal.category IN ('csam', 'cse', 'grooming') AS absolute,
            signal.persisted_at,
            cn_safety.trust_expires_at(signal.expires_at) AS removal_at,
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
      AND entry.persisted_at > now() - cn_admin.retention_interval('risk_signal')
$$;

UPDATE cn_safety.trust_target_signals entry
SET removal_at = cn_safety.trust_expires_at(signal.expires_at)
FROM cn_safety.risk_signals signal
WHERE signal.id = entry.signal_id;

CREATE INDEX trust_target_signals_persisted ON cn_safety.trust_target_signals (persisted_at);

-- 掃除が集計に反映した risk signal の保持日数（上の既定と同じ値から始める）。日数を延ばしたときに、
-- 期限切れとして外していた行を集計へ戻す範囲を決める。
ALTER TABLE cn_safety.trust_settings ADD COLUMN risk_signal_days INTEGER NOT NULL DEFAULT 180;

DROP TRIGGER trust_risk_signal_updated ON cn_safety.risk_signals;

-- 期限の列と、その索引を消す。
ALTER TABLE cn_admin.reports DROP COLUMN expires_at;
ALTER TABLE cn_admin.tester_feedback DROP COLUMN expires_at;
ALTER TABLE cn_legal.sensitive_items DROP COLUMN expires_at;
ALTER TABLE cn_legal.rights_requests DROP COLUMN expires_at;
ALTER TABLE cn_legal.rights_request_events DROP COLUMN expires_at;
ALTER TABLE cn_admin.operator_actions DROP COLUMN expires_at;
ALTER TABLE cn_safety.signed_moderation_events DROP COLUMN retention_expires_at;
ALTER TABLE cn_safety.risk_signals DROP COLUMN retention_expires_at;

-- 値が変わった行だけが集計の行を作り直す。
CREATE TRIGGER trust_risk_signal_updated
    AFTER UPDATE OF target, target_id, category, severity, basis, visibility, confidence,
        expires_at, appeal_status, persisted_at, operator_adjusted_at
    ON cn_safety.risk_signals
    FOR EACH ROW WHEN (
        (OLD.target, OLD.target_id, OLD.category, OLD.severity, OLD.basis, OLD.visibility,
         OLD.confidence, OLD.expires_at, OLD.appeal_status, OLD.persisted_at,
         OLD.operator_adjusted_at)
        IS DISTINCT FROM
        (NEW.target, NEW.target_id, NEW.category, NEW.severity, NEW.basis, NEW.visibility,
         NEW.confidence, NEW.expires_at, NEW.appeal_status, NEW.persisted_at,
         NEW.operator_adjusted_at))
    EXECUTE FUNCTION cn_safety.trust_risk_signal_changed();

-- 期限削除が起算点の索引の範囲だけを読むための索引。reports・tester_feedback の created_at と
-- operator_actions の occurred_at には既存の索引がある。
CREATE INDEX idx_cn_legal_sensitive_items_retention
    ON cn_legal.sensitive_items (data_category, created_at);
CREATE INDEX idx_cn_legal_rights_requests_retention
    ON cn_legal.rights_requests (cn_legal.rights_request_retention(status), updated_at);
CREATE INDEX idx_cn_legal_rights_request_events_occurred_at
    ON cn_legal.rights_request_events (occurred_at);
CREATE INDEX idx_cn_safety_events_persisted_at
    ON cn_safety.signed_moderation_events (persisted_at);
CREATE INDEX idx_cn_safety_risk_signals_persisted_at
    ON cn_safety.risk_signals (persisted_at);

-- 期限の書き直しが無くなったので、append-only の 2 表は期限削除だけを許す。
CREATE OR REPLACE FUNCTION cn_legal.reject_rights_request_event_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       AND current_setting('kukuri.retention_cleanup', true) = 'on' THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'rights_request_events are append-only';
END;
$$;

CREATE OR REPLACE FUNCTION cn_admin.reject_operator_action_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE'
       AND current_setting('kukuri.retention_cleanup', true) = 'on' THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'operator_actions are append-only';
END;
$$;

-- #1221 R5-F: CN の再取得可能な保存物を、node の受入下限 W・容量 B・保持期間 T で有界に回収する
-- （2026-09-26 ユーザー決定）。
--
-- W = max(現在 − T, 容量超過で回収した最古の時刻) を永続化し、単調に上げる。署名済み envelope の作成時刻が
-- W 未満の投稿は索引へ入らず（trigger が拒否する）、W 未満の撤回 marker は残す必要が無い（再受入が起きない）。
-- 索引・撤回 marker・関係のアクション・scan verdict・内容 scan cache を W で 1 回 128 件以内に回収する。
-- 計数（units）は 5 表の行数の合計で、容量 B と比べる。

CREATE TABLE cn_index.retention_state (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    -- 受入下限 W（unix 秒）。単調に上がる。
    floor BIGINT NOT NULL DEFAULT 0,
    -- 回収対象の行数の合計。
    units BIGINT NOT NULL DEFAULT 0,
    -- 容量 B（行数）と保持期間 T（秒）。indexer が起動時に運用設定から書く。
    capacity BIGINT,
    retention_secs BIGINT
);

-- 撤回 marker は撤回対象の署名済み envelope の作成時刻を持つ。既存の marker は作成時刻が分からないため、
-- 移行時刻に時計のずれの許容（600 秒）を足した上界を置く（W がこれを越えるまで残る）。
ALTER TABLE cn_index.known_post_withdrawals
    ADD COLUMN created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT + 600);
ALTER TABLE cn_index.known_post_withdrawals ALTER COLUMN created_at DROP DEFAULT;
CREATE INDEX known_post_withdrawals_created_at ON cn_index.known_post_withdrawals (created_at);

-- 関係のアクションは起点の投稿の作成時刻（フォローは観測した時刻）を持つ。
ALTER TABLE cn_index.relation_actions
    ADD COLUMN created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT + 600);
ALTER TABLE cn_index.relation_actions ALTER COLUMN created_at DROP DEFAULT;
CREATE INDEX relation_actions_created_at ON cn_index.relation_actions (created_at);

CREATE INDEX scan_verdicts_updated_at ON cn_safety.scan_verdicts (updated_at);
CREATE INDEX index_entries_verdict ON cn_index.index_entries (verdict_id);
CREATE INDEX content_scan_cache_completed_at ON cn_safety.content_scan_cache (completed_at);

CREATE FUNCTION cn_index.retention_count_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE cn_index.retention_state
    SET units = units + CASE WHEN TG_OP = 'INSERT' THEN 1 ELSE -1 END;
    RETURN NULL;
END;
$$;

CREATE TRIGGER retention_count_index_entries
    AFTER INSERT OR DELETE ON cn_index.index_entries
    FOR EACH ROW EXECUTE FUNCTION cn_index.retention_count_changed();
CREATE TRIGGER retention_count_withdrawals
    AFTER INSERT OR DELETE ON cn_index.known_post_withdrawals
    FOR EACH ROW EXECUTE FUNCTION cn_index.retention_count_changed();
CREATE TRIGGER retention_count_relation_actions
    AFTER INSERT OR DELETE ON cn_index.relation_actions
    FOR EACH ROW EXECUTE FUNCTION cn_index.retention_count_changed();
CREATE TRIGGER retention_count_scan_verdicts
    AFTER INSERT OR DELETE ON cn_safety.scan_verdicts
    FOR EACH ROW EXECUTE FUNCTION cn_index.retention_count_changed();
CREATE TRIGGER retention_count_content_scan_cache
    AFTER INSERT OR DELETE ON cn_safety.content_scan_cache
    FOR EACH ROW EXECUTE FUNCTION cn_index.retention_count_changed();

-- 移行時の 1 回だけ、既存の行数から計数を作る。
INSERT INTO cn_index.retention_state (id, units)
SELECT TRUE,
       (SELECT COUNT(*) FROM cn_index.index_entries)
     + (SELECT COUNT(*) FROM cn_index.known_post_withdrawals)
     + (SELECT COUNT(*) FROM cn_index.relation_actions)
     + (SELECT COUNT(*) FROM cn_safety.scan_verdicts)
     + (SELECT COUNT(*) FROM cn_safety.content_scan_cache);

-- 撤回済み、または作成時刻が W 未満の投稿は索引へ入れない。marker と W を同じ文で読み、回収と並行しても
-- 「marker を消した後で W が古い」状態を見ない。
CREATE OR REPLACE FUNCTION cn_index.reject_known_withdrawn_entry() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended(
        json_build_array(NEW.scope_kind, NEW.scope_id, NEW.object_id)::text, 0
    ));
    IF EXISTS (
        SELECT 1 FROM cn_index.known_post_withdrawals
        WHERE scope_kind = NEW.scope_kind AND scope_id = NEW.scope_id AND object_id = NEW.object_id
        UNION ALL
        SELECT 1 FROM cn_index.retention_state WHERE NEW.created_at < floor
    ) THEN
        RAISE EXCEPTION 'known withdrawn or expired post cannot be indexed';
    END IF;
    RETURN NEW;
END
$$;

-- 撤回は索引行を消す。作成時刻が W 未満の撤回は、索引へ再び入れないため marker を残さない。
CREATE OR REPLACE FUNCTION cn_index.apply_verified_withdrawal() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended(
        json_build_array(NEW.scope_kind, NEW.scope_id, NEW.object_id)::text, 0
    ));
    DELETE FROM cn_index.index_entries
    WHERE scope_kind = NEW.scope_kind AND scope_id = NEW.scope_id AND object_id = NEW.object_id;
    IF EXISTS (SELECT 1 FROM cn_index.retention_state WHERE NEW.created_at < floor) THEN
        RETURN NULL;
    END IF;
    RETURN NEW;
END
$$;

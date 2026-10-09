-- #1704 AC-1: 案件保持の期限を行に持たず、起算点の列と保持区分ごとの日数で判定する（ADR 0034 §1・§4）。
-- 日数は cn-user-api の起動時と `cn-cli retention sweep` が operator config から書く。日数を変えると
-- 既存の行にもすぐ効き、行は書き直さない。下の既定値は operator config の既定と同じ。

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

-- 期限の列と、その索引を消す。
ALTER TABLE cn_admin.reports DROP COLUMN expires_at;
ALTER TABLE cn_admin.tester_feedback DROP COLUMN expires_at;
ALTER TABLE cn_legal.sensitive_items DROP COLUMN expires_at;
ALTER TABLE cn_legal.rights_requests DROP COLUMN expires_at;
ALTER TABLE cn_legal.rights_request_events DROP COLUMN expires_at;
ALTER TABLE cn_admin.operator_actions DROP COLUMN expires_at;
ALTER TABLE cn_safety.signed_moderation_events DROP COLUMN retention_expires_at;
ALTER TABLE cn_safety.risk_signals DROP COLUMN retention_expires_at;

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

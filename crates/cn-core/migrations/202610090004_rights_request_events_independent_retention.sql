-- #1706: 判断・通知履歴は申出本体とは別の期限で消す。履歴の保持（365 日）が本体の期限削除を待たせないよう、
-- 本体を参照する外部キーを外す。履歴は本体と同じ取引でだけ書かれる（append_event）。
ALTER TABLE cn_legal.rights_request_events
    DROP CONSTRAINT IF EXISTS rights_request_events_request_id_fkey;

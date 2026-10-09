-- #1714: readiness の索引の整合検査が、索引の真実源と判定の表を全行読まないようにする。
--
-- 索引の行数は、5 表の行数の計数と同じ trigger（retention_count_changed）が同じ UPDATE で数え、readiness はこの
-- 1 行を読む。失敗の理由（scan_failed / provider_unavailable / unscanned）で許可になった判定は、数えずに保存の時点で
-- 拒否する（検索の読み口はこの判定を通すため。判定の作成は失敗を許可にしない）。
--
-- 計数の初期値を実行中の書込みと食い違わせないよう、最初に索引への書込みを止める（startup は旧版のコンテナが
-- 動いたまま migration を回すことがある）。lock は索引 → 判定 → retention_state の順に取り、旧版の書込み
-- （索引 → 判定（FK の確認）→ retention_state（trigger））と循環させない。
LOCK TABLE cn_index.index_entries IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE cn_safety.scan_verdicts
    ADD CONSTRAINT scan_verdicts_failure_not_allowed
    CHECK (action <> 'allow' OR reason_code NOT IN ('scan_failed', 'provider_unavailable', 'unscanned'));

ALTER TABLE cn_index.retention_state ADD COLUMN index_entries BIGINT NOT NULL DEFAULT 0;

CREATE OR REPLACE FUNCTION cn_index.retention_count_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    delta BIGINT := CASE WHEN TG_OP = 'INSERT' THEN 1 ELSE -1 END;
BEGIN
    UPDATE cn_index.retention_state
    SET units = units + delta,
        index_entries = index_entries + CASE WHEN TG_TABLE_NAME = 'index_entries' THEN delta ELSE 0 END;
    RETURN NULL;
END;
$$;

UPDATE cn_index.retention_state SET index_entries = (SELECT COUNT(*) FROM cn_index.index_entries);

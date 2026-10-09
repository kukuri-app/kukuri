-- #1699 AC-1: 内容の risk signal の行が 1 行も無くなったら、その内容の著者の対応を同じ取引で消す
-- （ADR 0034 §1）。対応を足す取引（`insert_subject_author`）とは advisory lock で直列化する。足す側は
-- 共有で取り、ここは排他で取ってから残りの risk signal を確かめるので、確定した順に結果が決まる。
-- lock は取引ごとに 1 つなので、1 回の期限削除で消える内容の数によらない。
CREATE FUNCTION cn_safety.reclaim_risk_signal_subject_authors() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended('cn_safety.risk_signal_subject_authors', 0));
    IF NOT EXISTS (
        SELECT 1 FROM cn_safety.risk_signals
        WHERE target = OLD.target AND target_id = OLD.target_id
    ) THEN
        DELETE FROM cn_safety.risk_signal_subject_authors
        WHERE target = OLD.target AND target_id = OLD.target_id;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER risk_signal_subject_authors_reclaimed
    AFTER DELETE ON cn_safety.risk_signals
    FOR EACH ROW WHEN (OLD.target IN ('post_id', 'blob_cid'))
    EXECUTE FUNCTION cn_safety.reclaim_risk_signal_subject_authors();

-- これまで期限で消えた risk signal の対応は残っていた。参照先の無い行を移行時に 1 回だけ消す。
DELETE FROM cn_safety.risk_signal_subject_authors a
WHERE NOT EXISTS (
    SELECT 1 FROM cn_safety.risk_signals s
    WHERE s.target = a.target AND s.target_id = a.target_id
);

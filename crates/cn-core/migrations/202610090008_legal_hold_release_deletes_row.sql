-- #1706: legal hold は解除で行を消す。解除の時刻・actor・hold ID は operator audit（legal_hold.release）に残る。
-- 解除済みの行を消してから解除の列を外す。列と一緒に、解除の CHECK と部分一意索引
-- （idx_cn_legal_one_active_hold_per_target）も消えるので、1 対象 1 hold は全体の一意索引で守る。
DELETE FROM cn_legal.legal_holds WHERE released_at IS NOT NULL;
ALTER TABLE cn_legal.legal_holds
    DROP COLUMN released_by,
    DROP COLUMN released_at;
CREATE UNIQUE INDEX IF NOT EXISTS idx_cn_legal_one_hold_per_target
    ON cn_legal.legal_holds (target_kind, target_id);

-- #1650: 自分への follow の edge（フォロワー）を、相手（subject）の順に小分けに読む（端末間の同期と移行でフォロワーを
-- 送る）。既存の target の索引は更新時刻の順なので、相手の順の続きの位置から読めない。
CREATE INDEX IF NOT EXISTS idx_follow_edges_target_subject
  ON follow_edges (target_pubkey, subject_pubkey);

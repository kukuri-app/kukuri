-- #1704 AC-3: 解除と有効の期限検索を、それぞれの起算点から上限つきで読む。
CREATE INDEX idx_cn_trust_observations_revoked_retention
    ON cn_trust.observations (received_at) WHERE NOT active;
CREATE INDEX idx_cn_trust_observations_active_retention
    ON cn_trust.observations (observed_at) WHERE active;

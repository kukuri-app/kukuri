CREATE TABLE cn_trust_observation_nodes (
    base_url TEXT PRIMARY KEY NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    needs_reconsent INTEGER NOT NULL DEFAULT 0 CHECK (needs_reconsent IN (0, 1)),
    revocation_pending INTEGER NOT NULL DEFAULT 0 CHECK (revocation_pending IN (0, 1))
);
CREATE TABLE cn_trust_observation_pending (
    base_url TEXT NOT NULL,
    observation_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    envelope_id TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    PRIMARY KEY (base_url, observation_key)
);
CREATE INDEX cn_trust_observation_pending_key
    ON cn_trust_observation_pending (observation_key, created_at);

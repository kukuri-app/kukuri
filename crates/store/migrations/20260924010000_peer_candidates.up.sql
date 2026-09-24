CREATE TABLE peer_candidates (
    scope TEXT NOT NULL,
    source TEXT NOT NULL,
    endpoint_id TEXT NOT NULL,
    endpoint_addr BLOB NOT NULL,
    accounted_bytes INTEGER NOT NULL,
    seen_ms INTEGER NOT NULL,
    PRIMARY KEY (scope, source, endpoint_id)
);

CREATE INDEX peer_candidates_window
    ON peer_candidates (scope, source, seen_ms, endpoint_id);
CREATE INDEX peer_candidates_learned_age
    ON peer_candidates (source, seen_ms);

CREATE TABLE peer_candidate_budget (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    learned_bytes INTEGER NOT NULL DEFAULT 0
);
INSERT INTO peer_candidate_budget (id) VALUES (1);

CREATE TABLE peer_seed_state (
    scope TEXT PRIMARY KEY,
    digest BLOB NOT NULL
);

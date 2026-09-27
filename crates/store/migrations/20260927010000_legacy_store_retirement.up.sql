CREATE TABLE legacy_store_retirement (
    kind TEXT PRIMARY KEY,
    cursor TEXT NOT NULL,
    done_at INTEGER
);

CREATE TABLE IF NOT EXISTS author_relationship_cache (
    local_author_pubkey TEXT NOT NULL,
    author_pubkey TEXT NOT NULL,
    following INTEGER NOT NULL,
    followed_by INTEGER NOT NULL,
    mutual INTEGER NOT NULL,
    friend_of_friend INTEGER NOT NULL,
    friend_of_friend_via_pubkeys_json TEXT NOT NULL,
    derived_at INTEGER NOT NULL,
    PRIMARY KEY (local_author_pubkey, author_pubkey)
);

CREATE INDEX IF NOT EXISTS idx_author_relationship_cache_local_author
    ON author_relationship_cache(local_author_pubkey, author_pubkey);

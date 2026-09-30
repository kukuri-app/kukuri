-- #1442: プロフィールのタイムラインに、手元の投稿の行(topic で受け取り検証済み)を著者と channel の索引の範囲で読む。
CREATE INDEX IF NOT EXISTS idx_object_index_cache_author_created
    ON object_index_cache(author_pubkey, channel_id, created_at DESC, object_id DESC);

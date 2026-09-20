-- #1239: 自分の既存の repost を、docs の全件読みではなく projection の索引で引く。
-- repost の行だけを対象にした式の索引で、`find_author_reposts_of` の条件と同じ式を使う。
CREATE INDEX IF NOT EXISTS idx_object_index_cache_repost_source
    ON object_index_cache (
        topic_id,
        author_pubkey,
        json_extract(repost_of_json, '$.source_object_id'),
        created_at
    )
    WHERE object_kind = 'repost';

-- #1419 AC-4: 表示設定 ON の間に cache へ置いた成人向けの blob を、OFF に戻したときに cache 全体を走査せずに消す。
CREATE INDEX remote_content_cache_adult_blob
    ON remote_content_cache(scope_key, is_protected, cache_key)
    WHERE kind = 'blob';

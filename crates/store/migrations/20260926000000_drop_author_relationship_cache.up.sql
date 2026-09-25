-- #1221 R4-D: 関係は follow edge から読むときに求める。全件再計算の cache を撤去する。
DROP INDEX IF EXISTS idx_author_relationship_cache_local_author;
DROP TABLE IF EXISTS author_relationship_cache;

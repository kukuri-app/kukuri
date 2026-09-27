-- #1221 R5-H: 旧 selector の巡回位置。旧 replica の同期を撤去したので使わない。
-- 需要の索引（idx_cn_index_supported_recent_demand）は bucket reader が使うので残す。
DROP TABLE cn_index.legacy_scope_cursor;

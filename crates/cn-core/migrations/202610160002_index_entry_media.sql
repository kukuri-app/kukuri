-- #1062 AC-2: 索引の行が参照する添付の blob hash。添付 hash を対象にした判断（malware・権利侵害）を、候補の
-- 読取り・取込の書込で照合する。索引の行と同じ鍵で持ち、行の削除（取込の除外・scope の回収・保持期限の回収）
-- に連鎖して消える。この版より前に索引へ入った行には記録が無く、再取込か保持期限で入れ替わるまで照合の外にある。
CREATE TABLE cn_index.index_entry_media (
    scope_kind TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    blob_hash TEXT NOT NULL,
    PRIMARY KEY (scope_kind, scope_id, object_id, blob_hash),
    FOREIGN KEY (scope_kind, scope_id, object_id)
        REFERENCES cn_index.index_entries (scope_kind, scope_id, object_id) ON DELETE CASCADE
);

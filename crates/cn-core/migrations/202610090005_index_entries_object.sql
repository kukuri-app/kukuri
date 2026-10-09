-- #1708: 権利侵害申出の受付と送信防止の適用は、投稿 id だけで索引の真実源を引く。等値の照会だけなので hash にする。
CREATE INDEX index_entries_object ON cn_index.index_entries USING hash (object_id);

-- #1221 R5-H: 新形式(時間 bucket)の writer へ切り替えた時刻。保護移行が全 kind で終端へ達したときに 1 回だけ入れ、
-- 以後は消さない(再起動・restore で旧 writer へ戻さない)。
CREATE TABLE writer_cutover (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    switched_at INTEGER NOT NULL
);

-- 取り下げの docs への書込みの永続 outbox。元投稿の位置と操作時の bucket の 2 行を積み、書けた行だけを消す。
CREATE TABLE withdrawal_write_outbox (
    withdrawal_envelope_id TEXT NOT NULL,
    replica_id TEXT NOT NULL,
    target_object_id TEXT NOT NULL,
    envelope_json TEXT NOT NULL,
    target_replica_id TEXT,
    PRIMARY KEY (withdrawal_envelope_id, replica_id)
);

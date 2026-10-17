-- #1840 AC-4 / ADR 0068: 公開スレッドの配信中の Live を、終わった Live の数に比例せず読む(配信中の行だけの部分索引)。
CREATE INDEX idx_live_session_cache_live_now
    ON live_session_cache(topic_id, channel_id, started_at DESC, session_id DESC)
    WHERE status = 'live';

-- #1221 R5-H: owner が account 経路で受け取った private channel の参加・退出。rotation の宛先と参加者数をページで読む。
-- left_at が NULL の行が参加中。updated_at は record の時刻(参加は joined_at、退出は left_at)で、新しいものだけで置き換える。
CREATE TABLE private_channel_participants (
    channel_id TEXT NOT NULL,
    epoch_id TEXT NOT NULL,
    participant_pubkey TEXT NOT NULL,
    left_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (channel_id, epoch_id, participant_pubkey)
);

CREATE INDEX idx_private_channel_participants_active
    ON private_channel_participants(channel_id, participant_pubkey)
    WHERE left_at IS NULL;

-- 回転の後に届いた参加 record へ grant を送る前に、その相手が channel で既知か(退出を含む)を点照合する。
CREATE INDEX idx_private_channel_participants_pubkey
    ON private_channel_participants(channel_id, participant_pubkey);

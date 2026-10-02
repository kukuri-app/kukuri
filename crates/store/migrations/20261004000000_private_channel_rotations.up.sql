-- #1219 AC-2: owner の鍵更新の操作と、参加者の表の数(ADR 0018 §8、ADR 0061 §9)。
--
-- 鍵更新は新しい世代の鍵の行を先に予約し、その世代の ID を操作 ID にする。rotation_from は元の世代で、この端末の
-- 鍵更新が終わるまで残る。rotation_after は配布の cursor(参加者の公開鍵)で、確定と account 同期への記録が済むまで
-- NULL。配布を終えたら両方を NULL にする(完了の記録を残さない)。
ALTER TABLE private_channel_epochs ADD COLUMN rotation_from TEXT;
ALTER TABLE private_channel_epochs ADD COLUMN rotation_after TEXT;
CREATE INDEX idx_private_channel_epochs_rotation
  ON private_channel_epochs (channel_id, epoch_id) WHERE rotation_from IS NOT NULL;

-- 資格喪失(channel の owner と mutual でない参加者。owner 自身は含めない)の印。follow の edge と参加者の行の
-- 書込みで更新し、(channel, epoch) ごとの参加中の数と資格喪失の数を trigger で保つ(view と auto rotate の判定は
-- 1 行を読む)。
ALTER TABLE private_channel_participants ADD COLUMN stale INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_private_channel_participants_member
  ON private_channel_participants (participant_pubkey, channel_id);
CREATE TABLE private_channel_participant_counts (
  channel_id TEXT NOT NULL,
  epoch_id TEXT NOT NULL,
  active INTEGER NOT NULL,
  stale INTEGER NOT NULL,
  PRIMARY KEY (channel_id, epoch_id)
);

UPDATE private_channel_participants SET stale = COALESCE((
  SELECT channel.owner_pubkey != private_channel_participants.participant_pubkey AND NOT (
    EXISTS (SELECT 1 FROM follow_edges WHERE subject_pubkey = channel.owner_pubkey
      AND target_pubkey = private_channel_participants.participant_pubkey AND status = 'active')
    AND EXISTS (SELECT 1 FROM follow_edges
      WHERE subject_pubkey = private_channel_participants.participant_pubkey
      AND target_pubkey = channel.owner_pubkey AND status = 'active'))
  FROM private_channels AS channel
  WHERE channel.channel_id = private_channel_participants.channel_id LIMIT 1), 0);
INSERT INTO private_channel_participant_counts (channel_id, epoch_id, active, stale)
  SELECT channel_id, epoch_id, SUM(left_at IS NULL), SUM(left_at IS NULL AND stale)
  FROM private_channel_participants GROUP BY channel_id, epoch_id;

CREATE TRIGGER private_channel_participant_counts_insert
AFTER INSERT ON private_channel_participants
BEGIN
  INSERT OR IGNORE INTO private_channel_participant_counts (channel_id, epoch_id, active, stale)
    VALUES (NEW.channel_id, NEW.epoch_id, 0, 0);
  UPDATE private_channel_participant_counts
    SET active = active + (NEW.left_at IS NULL), stale = stale + (NEW.left_at IS NULL AND NEW.stale)
    WHERE channel_id = NEW.channel_id AND epoch_id = NEW.epoch_id;
END;

CREATE TRIGGER private_channel_participant_counts_update
AFTER UPDATE OF left_at, stale ON private_channel_participants
BEGIN
  UPDATE private_channel_participant_counts
    SET active = active + (NEW.left_at IS NULL) - (OLD.left_at IS NULL),
        stale = stale + (NEW.left_at IS NULL AND NEW.stale) - (OLD.left_at IS NULL AND OLD.stale)
    WHERE channel_id = NEW.channel_id AND epoch_id = NEW.epoch_id;
END;

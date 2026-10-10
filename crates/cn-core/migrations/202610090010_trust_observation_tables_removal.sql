-- #1699 AC-2: 観測の revision と、観測提供の取消時刻の表を廃止する（ADR 0026 §8.3・§8.4）。
-- relation_version は評価に使った観測の digest で表し、取消は本人の同意の行を消して表す。
-- 取消の行が無効にしていた同意の行（取消より前の同意）を消して、移行の前後で有効な同意を変えない。
DELETE FROM cn_user.policy_consents c
USING cn_trust.observation_sharing_revocations r
WHERE c.subscriber_pubkey = r.observer_pubkey
  AND c.policy_slug = 'trust_observation_sharing'
  AND c.accepted_at <= r.revoked_at;

DROP TABLE cn_trust.observation_sharing_revocations;
DROP TABLE cn_trust.observation_target_revisions;
DROP SEQUENCE cn_trust.observation_revision_seq;

# ADR 0020: Pairwise DM v1

## Status
Accepted

## Date
2026-03-30

## Base Branch
`main`

## Related
- `docs/adr/0012-topic-first_progressive_community_filtering_draft.md`
- `docs/adr/0018-channel-first-sidebar-and-unified-epoch-lifecycle.md`
- `docs/adr/0019-profile-avatar-blob-data-classification.md`
- [ADR 0055](0055-demand-owned-network-work.md)（account受信routeへの段階移行）

## Summary
- `chat` ではなく `DM` を正式方針にする。
- DM v1 は `1on1 / global pairwise / mutual 限定 / offline 可 / local-only transcript / local-only delete / account-key E2E / image+video attachment 対応` で固定する。
- private channel の UI / secure storage / blob transport / social graph は再利用するが、`docs` を正本にする private channel data plane は使わない。DM は `private channel audience v1` の外側にある別データプレーンとして定義する。

2026-09-23の輸送追補: 暗号frame・署名ACK・mutual・保護outboxの契約は本ADRを維持する。
輸送routeは[ADR 0055 §4](0055-demand-owned-network-work.md)に従い、署名済みaccount受信先へ
sealed参照とACKを移行した。2026-09-26のR4-Dで旧pairwise topicの購読・hint送信・ACKを撤去し、
DMの輸送はaccount routeだけになった。重複した有効ACKは最初に保存した配達時刻を維持する。
以下のpairwise輸送表は元のv1設計であり、現行の輸送は追補が定める（暗号鍵の導出は維持）。

## Feature Data Classification

| 対象 | Canonical Source | Local Cache / Projection | v1 扱い |
|---|---|---|---|
| DM transcript | local SQLite DM store | same | local only |
| DM outbox / retry state | local SQLite DM store | same | local only |
| DM attachment blob | local encrypted blob store | blob cache | local only |
| DM transport frame / ack | pairwise hint + blob transport | なし | transient |
| social graph mutual 判定 | public author docs replica | SQLite projection | send gate 入力 |

## Decision

### 1. Conversation boundary
- DM は `sorted(local_pubkey, peer_pubkey)` から導く stable `dm_id` を持つ。
- 1 pair = 1 conversation とし、topic 配下には置かない。
- `open/send` は mutual 相手にだけ許可する。
- mutual が崩れた後も local history は読めるが、`send` と queued retry は停止する。mutual 復帰で再開する。

### 2. Crypto / transport
- account key から secp256k1 ECDH で pairwise root secret を導出し、HKDF で `frame key` と `attachment key` を domain-separated に派生する。
- 本文は encrypted `DirectMessageFrameV1` として送る。
- attachment blob は plaintext のまま送らず、message / attachment 単位の subkey で暗号化して送る。
- `GossipHint` に DM 専用 variant を追加し、opaque な `dm_id / message_id` 通知だけを流す。
- sender は encrypted frame を local outbox に保持し、recipient 到達時に再送する。recipient は decrypt / store 後に `DirectMessageAckV1` を返し、sender は retry state を外す。

### 3. Attachment scope
- v1 attachment は `image/*` と `video/*` のみ。
- cardinality は single attach に揃え、`1 message = 1 image` または `1 video + poster` に固定する。
- video は既存 poster pipeline を再利用し、poster 生成を必須にする。
- attachment manifest は encrypted frame の内側にのみ入れる。

### 4. Persistence / delete
- `dm_conversations`, `dm_messages`, `dm_outbox`, `dm_message_tombstones` を local store に持つ。
- delete は message 単位と conversation 全消去の両方を持つが、どちらも local only。
- local delete は transcript と local attachment 参照を消す。queued outgoing message の delete は future delivery も中止する。
- delete 済み message が retry / duplicate receive で復活しないよう、`message_id` tombstone は保持する。

### 5. API / desktop surface
- `kukuri-core` に `DirectMessageFrameV1`, `DirectMessageAckV1`, pairwise secret / topic, frame / attachment encrypt-decrypt helper を追加する。
- `kukuri-app-api` / `desktop-runtime` に `open_direct_message`, `list_direct_messages`, `list_direct_message_messages`, `send_direct_message`, `delete_direct_message_message`, `clear_direct_message`, `get_direct_message_status` を追加する。
- desktop は `AuthorDetail` の mutual 相手に `Message` action を出し、DM 一覧は private channel とは別 surface に置く。

## Consequences
- DM は docs replica に書かれないため、shared durable replica を truth source にしない。
- sender が local delete した後の recipient 側 attachment 再取得は保証しない。
- `reaction / repost / live / game / arbitrary file` は scope 外とし、v1 は text + reply-to + image/video attachment に限定する。

## Test Plan
- `dm_pairwise_secret_derives_same_topic_for_pair`
- `dm_frame_encrypt_decrypt_roundtrip_and_tamper_reject`
- `dm_send_requires_mutual_relationship`
- `dm_offline_outbox_delivers_after_reconnect`
- `dm_local_delete_prevents_duplicate_reinsert`
- `dm_restart_resumes_pending_outbox`
- `pairwise_dm_offline_text_image_video_delivery_and_local_delete`

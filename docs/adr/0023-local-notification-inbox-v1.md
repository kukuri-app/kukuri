# ADR 0023: Local Notification Inbox v1

## Status
Accepted

## Date
2026-04-05

## Base Branch
`main`

## Related
- `docs/adr/0002-feature-data-classification-template.md`
- `docs/adr/0013-social-graph-foundation-draft.md`
- `docs/adr/0016-repost-data-classification.md`
- `docs/adr/0018-channel-first-sidebar-and-unified-epoch-lifecycle.md`
- `docs/adr/0020-pairwise-dm-v1.md`
- `docs/adr/0022-local-author-mute-and-social-management.md`

## Feature Data Classification
- Feature 名: local notification inbox v1
- Durable / Transient: durable local-only notification inbox + local unread state
- Canonical Source: local SQLite `notifications`
- Replicated?: No
- Rebuildable From: local notification records only
- Public Replica / Private Replica / Local Only:
  - notification inbox / unread state / saved preview: local only
  - source content canonical source: 既存 public replica / private replica / DM path
- Gossip Hint 必要有無: No new hint type
- Blob 必要有無: new blob ownership は持たず、既存 payload/blob cache を参照
- SQLite projection 必要有無: Yes
- 必須 contract:
  - `remote_reply_to_local_post_creates_single_unread_reply_notification`
  - `public_or_private_post_with_pubkey_mention_creates_mention_notification`
  - `simple_repost_of_local_post_creates_repost_notification`
  - `quote_repost_of_local_post_creates_quote_notification`
  - `incoming_dm_frame_creates_single_direct_message_notification_after_store`
  - `incoming_follow_edge_to_local_author_creates_followed_notification_for_observed_author`
  - `notification_overlap_uses_precedence_and_does_not_double_insert`
  - `restart_or_manual_hydration_does_not_backfill_or_duplicate_notifications`
  - `mark_notification_read_and_mark_all_read_update_unread_count`
- 必須 scenario:
  - public/private reply, mention, repost/quote repost, DM, observed follow を 2-3 client で確認する

## Decision
- notification v1 は `local-only durable inbox` とし、shared replica、community-node、他 device に notification object を持たせない。
- v1 の notification kind は `mention`, `reply`, `repost`, `quote_repost`, `direct_message`, `followed` に固定する。
- notification は「この端末が新規に受信したイベント」からだけ生成し、feature 導入時の backfill は行わない。
- topic / private-channel subscription の remote `DocEvent` で取り込んだ `post | comment | repost` から `reply`, `mention`, `repost`, `quote_repost` を生成する。
- author subscription の remote `DocEvent` で取り込んだ `graph/follows/*` から `followed` を生成する。
- DM subscription の `GossipHint::DirectMessageFrame` は decrypt/store 成功後だけ `direct_message` を生成する。
- 判定規則は次に固定する。
  - `reply`: `reply_to_object_id` が local author の object を直接指すときだけ
  - `mention`: post/comment 本文または quote commentary 内の `@<64hex pubkey>` が local author pubkey と一致するときだけ
  - `repost`: public simple repost で `repost_of.source_author_pubkey == local author`
  - `quote_repost`: public quote repost で `repost_of.source_author_pubkey == local author`
  - `direct_message`: incoming DM 1 message ごとに 1 件
  - `followed`: active incoming follow edge が local author を target にしたときだけ。完全性は observed author replica only
- self-authored event、local write、restart / hydration / query だけで見えた既存 data は notification 化しない。
- 同一 source event からは 1 notification だけ作る。重複判定の優先順位は `reply > quote_repost > repost > followed > mention` に固定する。
- dedupe key は docs 系が `(recipient_pubkey, kind, source_envelope_id)`、DM 系が `(recipient_pubkey, kind, dm_id, message_id)` である。
- notification record の最小 shape は `notification_id`, `kind`, `actor_pubkey`, `source_envelope_id?`, `source_replica_id?`, `topic_id?`, `channel_id?`, `object_id?`, `dm_id?`, `message_id?`, `preview_text?`, `created_at`, `received_at`, `read_at?` とする。
- inbox は `received_at DESC` で読み、v1 は `read/unread` のみを持つ。dismiss / archive / toast / push は scope 外とする。
- private channel / DM preview は local-only snapshot として保持してよい。

## Public Interfaces
- `kukuri-store`
  - `NotificationRow`
  - `NotificationKind`
  - `put_notification_if_absent`
  - `list_notifications`
  - `mark_notification_read`
  - `mark_all_notifications_read`
  - `count_unread_notifications`
- `kukuri-app-api`
  - `NotificationView`
  - `NotificationStatusView`
  - `list_notifications`
  - `mark_notification_read`
  - `mark_all_notifications_read`
  - `get_notification_status`
- `desktop-runtime` / Tauri / `apps/desktop/src/lib/api.ts`
  - notification APIs をそのまま公開する
- desktop shell route / pane / badge 配置
  - この ADR の外とし、後続 UI slice で決める

## Consequences
- notification inbox は端末ローカルで durable だが cross-device sync されない。
- `followed` は observed-only なので、未知 follower を完全には捕捉しない。
- v1 は過去イベントの一括 backfill を行わないため、feature 有効化時の inbox は空開始になる。
- reaction / live / game / private-channel moderation などの通知は v1 scope に含めない。

## 2026-09-24 改訂: #1221 R1-D の有界な一覧と既読

- GUI の `list_notifications` 全件取得は廃止し、既存の `(received_at, notification_id)` 索引で20件ずつ前後へ読む。CLI の明示的な全件一覧は同じページを順に読む入口として維持する。通知kind、対象範囲、端末ローカル正本は変えない。
- 未読数はSQLiteの単一状態行と挿入・個別既読・削除の差分で保持し、毎回の `COUNT(*)` をやめる。既存行の初期数はこのmigrationで一度算出する。画面イベントでは表示中のページだけを更新し、定期badge確認は状態行だけを読む。
- 「すべて既読」はdispatch挿入順のwatermarkを進め、履歴全行を書き換えずに即時反映する。旧dispatch連番のない行にも同じ既読時刻を適用する。後から届いた古い時刻の通知は新しい連番なので未読のまま残る。個別に保存済みの `read_at` は維持し、watermarkで既読になった行の `read_at` は直近の一括既読時刻として返す。
- dismiss / archive は引き続き対象外。旧GUI全件コマンドと通知イベント後の全件再取得は撤去対象とする。

# #1221 R4-D: DM の起動・関係更新を差分にし、旧 pairwise を撤去する

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R4-D。区分 C、Scope revision
`2026-09-24-outcome-v5-r2a-retention-1`、基準は #1368 merge `2fb57253`。依存の R4-A・R4-C は完了済み。

- DM の送受信と ACK は account の受信 route だけを使う（R4.2 の account 単位の再送 owner と R4-A の宛先解決を再利用）。
  相手ごとの pairwise topic の購読 task、pairwise の送信・ACK、同じ DM の二重送信を撤去する。
- 起動時は会話・outbox・全 mutual を列挙しない。未 ACK の outbox は既存の due 索引から再送 owner が読む。
- 関係（following / followed_by / mutual / friend_of_friend）は、読むときに対象の author の follow edge から求める。
  1 つの edge の変化で全関係の再計算・全 author の購読・DM 購読の再構築をしない。関係の cache table と全件再計算
  （`rebuild_author_relationships`）を撤去する。
- 相手の follow / unfollow は、R4-B の follow の offer（unfollow も送る）で署名済みの edge を受け取り、手元の edge に
  保存する。mutual の解除は次の送信・受信の判定に反映し、保護 outbox は ACK まで残す。
- 会話ヘッダーの接続 peer 数（`peer_count`）は view・CLI schema・UI から撤去し、「送信可能／送信不可」だけを表示する
  （2026-09-26 ユーザー決定）。

対象外: author の購読 task の寿命と起動時の followee の購読（R2-C）、DM の本文・暗号 frame・添付の形式、新しい DM の機能。

## 受入条件

| ID | 対象・期待結果 | 判定 |
| --- | --- | --- |
| AC-1 | DM の送信は outbox 保存の後、account の再送 owner だけが account route で送る。受信した frame の ACK も account route だけで返す。pairwise topic の購読・publish は 0 | 実 Iroh で DM→ACK が届き、pairwise の購読・publish が 0 |
| AC-2 | 起動時の DM の再開は会話・outbox・mutual を列挙せず、再送 owner を起動するだけ。offline からの再起動で未 ACK の outbox を送る | 起動時の読取り件数が履歴 10 倍でも同じ。再起動後に ACK まで届く |
| AC-3 | 関係は対象 author の edge から読むときに求め、edge の変化で全件の再計算・購読の再構築をしない。follow / unfollow の offer で相手の edge を反映する | 履歴 10 倍で関係 1 件の変更と 1 件の読取りの作業量が同じ。mutual 解除後は送信・受信 0、保護 outbox は残る |
| AC-4 | DM の状態 view から `peer_count` を撤去し、ヘッダーは送信可能／送信不可を表示する | CLI schema・生成型・UI・表示の確認 |

維持する条件: INVAR-1 mutual の判定を送信・受信・添付保存の前に確かめる。INVAR-2 未 ACK の outbox を失敗・期限で消さない。
INVAR-3 message ID・暗号 frame・tombstone・署名 ACK の照合を変えない。INVAR-4 friend-of-friend の意味（自分が follow する
相手のうち対象を follow している相手、自分と follow 済みの相手を除く）を変えない。

## 入口と副作用

| ID | 対応 | 入口 → helper | 期待結果 / 禁止する副作用 |
| --- | --- | --- | --- |
| T1 | AC-1 | `send_direct_message` → outbox 保存 → 再送 owner `flush_due_direct_message_outbox` → `publish_account_receive_dm_frame` | 宛先未解決なら保護 row を残して次の tick。pairwise の publish・購読 0 |
| T2 | AC-1 | account の受信 route → `ingest_direct_message_frame` → `publish_direct_message_ack` → `publish_account_receive_dm_ack` | ACK は frame を送った provider へ account route で返す。mutual でなければ返さない |
| T3 | AC-2 | 起動 → `resume_direct_message_state` → `start_direct_message_outbox_retry` | 会話・outbox・mutual の列挙 0。DM 購読の起動 0 |
| T4 | AC-3 | `get_author_relationship`（SQLite / Memory） | 自分と対象の edge 2 件を主キーで点読し、friend_of_friend は自分の followee と対象への edge の主キー結合 |
| T5 | AC-3 | `unfollow_author` → `queue_public_notification_offer(Follow)` / 受信 `ingest_public_notification_offer(Follow)` → `put_envelope` | 自分を指す署名済み edge だけを保存。unfollow は通知を作らない |
| T6 | AC-4 | `DirectMessageStatusView` → CLI schema・生成型・会話ヘッダー | `peer_count` 無し。ヘッダーは「送信可能／送信不可」 |

## 実装の照合（監査前）

- 撤去: `service/social_helpers.rs`（全関係の再計算、全 mutual の列挙、DM 購読の reconcile / 再起動）、DM 購読 task の
  registry と再起動の期限、shutdown の DM 購読停止、`AppService::rebuild_author_relationships` とその全呼出し、
  `ensure_direct_message_subscription`、`get_direct_message_topic_status`（`DirectMessageTopicStatusView`、IPC）、
  pairwise の frame・ACK の publish、送信直後の即時送信、`direct_message_topic_peer_count`。
- store: `author_relationship_cache` を migration `20260926000000` で削除し、`rebuild_author_relationships` を trait から外した。
  `AuthorRelationshipProjectionRow::derive` が edge から関係を作る（self と関係無しは `None`、following の相手は FoF にしない）。
- 受信の frame 取込みと ACK の照合（`handle_direct_message_hint`）は account route からだけ呼ばれる。暗号 frame・
  message ID・tombstone・署名 ACK の照合は変えない。
- 相手の edge は follow の offer と、需要のある相手の author 購読の追いつき（R5-C の author reader）で届く。DM を開く・
  送る・状態を見る操作と private channel の参加時は、その相手 1 人の author を従来どおり購読する（寿命の整理は R2-C）。
  起動時の全会話・全 outbox・全 mutual の購読はしない。

## 検証（局所）

- 変更前に失敗する条件: 旧実装は unfollow で相手へ offer を送らず（`unfollow_sends_a_follow_offer_to_the_target` は
  offer 0 で時間切れ）、follow の offer の受信で edge を保存しなかった（`unfollow_offer_revokes_mutual_and_keeps_protected_outbox`
  は mutual のまま）。送信・再送は pairwise の publish を行い（`dm_due_owner_processes_bounded_new_and_retry_lanes` の
  publish 0 が不成立）、起動時は全 mutual の DM 購読を作った。
- store: `relationships_derived_from_edges`（SQLite / Memory で following・followed_by・mutual・FoF と via の除外）、
  `author_relationship_join_reads_by_primary_keys`（関係の読取りの query plan に `SCAN` が無い）。migration の往復・
  schema golden・全 162 件が成功。
- app-api: `dm_restart_resends_outbox_by_account_route_and_recipient_keeps_local_delete`（再起動後の owner 1 件、
  account の offer で送信し保護 outbox は ACK まで残る、pairwise の購読・publish 0、受信側は DM を開かずに会話一覧へ出て、
  手元で消した message は再取込みで戻らない）、unfollow の 2 件、既存の account route の DM・ACK・mutual 再確認の各 test。
  app-api lib 全 551 件（`iroh-integration-tests` つき）が成功。
- 実 Iroh `real_account_route_fetches_bound_provider_manifest_and_reflects_dm`（保存した outbox が account route で届き、
  署名 ACK で outbox が消える）。harness `pairwise_dm_offline_text_image_video_delivery_and_local_delete`（2 runtime、
  offline 送信・再起動・添付・ACK・手元削除）が成功。
- desktop: `tsc --noEmit`、会話ヘッダー・貼付け・routing・view model・i18n parity・API 契約の vitest。IPC 型は
  `cargo xtask ipc-types` で再生成、wire fixture は `UPDATE_FIXTURES=1` で再生成。
- `cargo clippy --all-targets -D warnings`（app-api は `iroh-integration-tests` つき、store・desktop-runtime・cli・harness）、
  `cargo fmt --check`、`oversized-files` は増加なし。全体 suite は PR の CI。

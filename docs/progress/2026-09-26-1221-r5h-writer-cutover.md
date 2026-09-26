# #1221 R5-H: writer 切替と旧定常同期・暫定選択器・旧 queue の撤去

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R5-H と、2026-09-26 のユーザー決定「R5-H 旧sync撤去後の受信・配送・索引・案内」
「R5-H 参加recordのowner配送」。基準は #1374 merge `051e1bfb`。依存の R2-C・R4-D・R5-D・R5-F・R5-G は完了済み。

- 保護移行(R5-G)が全 kind で終端へ達した端末で、新形式(ADR 0054 §1・§2 の時間 bucket)の writer へ自動で 1 回だけ切り替える。
- 切替後は新形式だけへ書く。旧形式の保存済みデータは手元から従来どおり読む。旧版が旧形式へ書いた新着を自動で回復する経路は作らない。
- client の旧 namespace の定常 sync を撤去し、受信を hint の exact 読取りと有界な読み直しにする。
- CN の無上限 `ReplicaEvent` queue・legacy selector・旧 replica 経路を撤去し、索引を bucket reader の 64 枠の巡回にする
  (需要のある scope は 30 秒、それ以外は `poll_interval`)。

対象外: 旧保存領域の回収(R5-I)、R2-B・R2-D。

## 受入条件と状態

| ID | 対象・期待結果 | 状態・試験 |
| --- | --- | --- |
| AC-1 | 切替状態 1 行。移行完了で 1 回だけ保存し、再起動・restore で戻らない | 実装済み。store `the_writer_switches_once_after_the_migration_and_stays_switched`、desktop-runtime `the_writer_switches_once_after_the_migration_and_keeps_it_across_restarts` |
| AC-2 | 各 record を §2 の replica と key へ。日の境界・再試行で宛先が変わらない | 実装済み(Dome を除く)。app-api `bucket_writer` の 3 件。版つき cursor は作らない(既存 cursor の `created_at` から bucket が一意。ADR 0054 §3) |
| AC-3 | 切替後は旧 replica へ書かない | 実装済み(Dome を除く。未決)。同上。切替後の本人投稿は保護所有先へ入る(desktop-runtime `own_posts_written_after_the_switch_are_protected_and_restored`) |
| AC-4 | 旧 sync なしの受信(hint の exact 読取り、lease の開始・再接続・日の境界の 1 ページの読み直し、通知) | 実装済み。実 Iroh `real_iroh_hint_exact_read_and_bounded_rereads_receive_new_posts_without_sync`、`the_day_boundary_reread_waits_until_the_next_bucket`、規模 `reads_do_not_grow_from_one_thousand_to_one_hundred_thousand_entries`(新着の受信を hint の読取りで数える) |
| AC-5 | 参加・退出 record と handoff grant の account 経路の配送、owner の SQLite 保存 | 実装済み(非 owner の参加者数は未決)。app-api `participant_leave_while_the_owner_is_offline_arrives_after_the_owner_restarts`、private_channels の friend_only・friend_plus・invite・leave、store `private_channel_participants_keep_the_newest_record_and_page_by_pubkey` |
| AC-6 | client の旧 sync の撤去 | 実装済み。docs-sync `local_bucket_reads_stay_idle_after_seed_reapply_and_close_preserves_data`、開いた handle は 128 まで(`writes_across_many_day_buckets_keep_open_handles_at_the_limit`)。sync status の docs の活動時刻は hint の取込みで記録する |
| AC-7 | CN の撤去と bucket reader の周期 | 実装済み。cn-indexer `demanded_scope_is_read_each_demand_interval_and_others_each_poll` ほか |
| AC-8 | 新側は 64 scope / 256 要求の内側 | lease(R2-C)と CN の 64 枠 |
| AC-9 | 実 Iroh の client/CN で切替・再起動・日と epoch の境界 | client は AC-4 の試験、CN は `two_clients_feed_one_cn_through_bounded_bucket_reader`・`registered_private_epoch_reads_only_the_disclosing_provider` |
| AC-10 | 文書 | ADR 0054 §6、ADR 0055 §1.1、ADR 0048 §7、本書 |

## 入口と副作用

| 入口 | helper | 結果 |
| --- | --- | --- |
| 起動 | `DesktopRuntime`(`writer_switched_at` → `AppService::switch_writer`、`resume_withdrawal_writes`) | 最初の書込みより前に切替状態を渡す。取り下げの outbox を記録済みの宛先へ再開 |
| 保護移行の 1 ステップ | `protected_migration_step` → `SqliteStore::switch_writer_if_migrated` | 全 kind が終端なら 1 回だけ時刻を保存して切り替える |
| 投稿・返信・repost | `ServiceHandles::scope_write_replica`(署名の `created_at`) | public は topic bucket、private は現 epoch の channel bucket。manifest も同じ bucket |
| プロフィールの行 | `author_index_replica` | 作成時の author bucket |
| reaction | `scope_write_replica`(ミリ秒の署名時刻) | 作成時の bucket。識別は対象の投稿の replica |
| 取り下げ | `queue_withdrawal_writes` → `resume_withdrawal_writes` | 操作時の bucket(locator つき)と元投稿の位置の 2 行。書けた行だけを消す |
| live/game の作成・更新 | `scope_write_replica`・`update_locator_replica` | 作成時の bucket に最新 state、日が変わった更新は更新時の bucket に署名済み envelope |
| profile・follow・block・asset・Dome preset/移動 | `persist_author_event` | 制御領域の key はそのまま、更新時の author bucket に署名済み envelope |
| lease の task | `scope_receive.rs`(`spawn_subscription_task`・`apply_content_hint`・`reread_scope`) | replica を開かない。hint の exact 読取り、開始・作り直し・日の境界で 1 ページと session の索引(種類ごとに 64 件) |
| session の表示 | `set_session_display` → `read_session` | 手元に無ければ provider から exact に読む。remote の manifest は provider から取得。projection に行が無い session は hint・読み直し・表示の channel の scope で探す |
| session の hint | `GossipHint::SessionChanged { sent_at }` | 送った時刻を載せ、同じ session の続く更新を gossip が重複として落とさない(旧版は無視する) |
| private channel の参加・退出 | `record_private_channel_participant` → `queue_epoch_control` | 手元の docs と参加者の表へ置き、owner でなければ epoch の鍵で封じて dm_outbox(`epoch-control:`)へ。DM の再送 owner が `EpochControl` で送り、ACK で消す |
| 回転 | `distribute_epoch_handoff_grants` | 参加者の表を 128 件ずつ読み、grant を旧 replica に置いて各参加者へ届ける |
| 制御 record の受信 | `ingest_epoch_control_offer` | owner は参加者の表へ、参加者は grant を手元の旧 epoch の replica へ置いて ACK |
| author の lease の読み直し | `hydrate_author_record` | 自分を指す Active の follow を新しく保存したら followed の通知(id は envelope から) |
| 保護移行(切替後) | `own_envelope_plan` | bucket へ書いた投稿のプロフィールの行を作成時の author bucket から写す |

## 撤去したもの・残したもの

- 撤去(client): `start_sync` の呼出し、`sync_requested`・`sync_peer_ids`、`reapply_sync_peers`、`restart_replica_sync`(trait 既定と
  `ReloadableDocsSync` の転送を含む)、`doc_start_sync`、`ensure_replica` の sync 分岐と `LocalThenRemote` の暗黙の同期開始、
  docs の event による反映・通知・窓の追いつき・recovery tick・backoff・probe・通知の baseline、空ページのときの再開と
  cooldown の台帳(`replica_sync_restart_deadlines`・`empty_recovery_candidates`)、直前の epoch の replica の保持、
  `IrohDocsNode::docs_sync_handle` と重複 field、`explicit_docs_sync.rs`、docs-sync の `connect_candidates` の試験と relay の sync 試験。
- 残した: `set_seed_peers`・`learn_peer`(reader の候補台帳)、`DocReadProtocol`(SyncHandle は page reader だけが使う。
  提供する namespace は読む間だけ開く)。
- 撤去(引継ぎ後): docs の全 epoch の参加 record の prefix 全件読み(`fetch_private_channel_participants_from_replica`・
  `active_private_channel_participants`)、grant の Frozen の policy を契機にした remote 読取り、CN の
  `IngestPipeline::ingest_recent_scope`(試験は `ingest_changed_keys` の本番経路へ)。
- 撤去(CN): `worker.rs` の `mpsc::unbounded_channel::<ReplicaEvent>` と生産・消費、`refresh_pass` の旧 replica 経路、`participant.rs`
  (legacy selector 一式)、`PublicReplicaReadMode`、migration `202609260003_drop_legacy_scope_cursor.sql`。失効 scope の巡回と R5-F の回収は
  `maintenance.rs` の `IndexMaintenance` へ移した。

## 更新案内(リリースノートの下書き)

配布のリリースノートは tag 間の commit から自動生成する(`docs/runbooks/release.md`)。手で足す項目の規則は無いため、文面をここに残す。

> この版から投稿・リアクション等の保存形式が変わります。更新した端末は、手元のデータの移行が済むと自動で新しい形式へ切り替わります。
> 切替後は、旧版の端末が書いた新しい投稿はこの版に表示されません(旧版のまま使っている相手には更新を案内してください)。
> 手元に保存済みの投稿・ブックマーク等はそのまま表示できます。

## 検証(局所)

- store: `protected_migration`・`migrations`・参加者の表の試験、schema golden を再生成(世代数 43)。169 件成功。
- app-api(`iroh-integration-tests`): lib 489 件成功・2 件失敗(下記「未決」の Dome と友達の友達)。
  mutation check(外すと失敗することを確認して戻した): 参加 record の送信、受け取った grant の保存、ACK での outbox の削除、
  transport の読み手の候補の hint topic、session の読み直し、followed の通知、session の scope、session の hint の送った時刻
  (旧担当の分は前任の記録)。
- docs-sync 66 件、iroh-node 47 件、transport 136 件、blob-service 12 件、core 145 件、kukuri-cli 全件成功。
  handle の上限と閉じた bucket の提供は、上限の処理・読む間の open を外すと失敗する。
- desktop-runtime: lib 316 件成功・3 件失敗(下記「未決」の非 owner の参加者数 2 件と自分の別端末のプロフィール 1 件)。
  切替後の本人投稿の保護の試験は、修正前に移行が止まって失敗した。1 回だけ IdentityStorage の試験群が止まったまま
  進まない実行があった(再実行と単独の実行では再現しない)。
- harness: 23 件成功(`private_channel_invite_connectivity` は session の scope の修正で成功)。
- scenario: `community_node_public_connectivity`(session の hint の送った時刻の修正で成功)・
  `community_node_index_query_client`・`community_node_report_routing`・`community_node_trust_relation_client`・
  `e2e-smoke` 成功。
- CN: cn-indexer 全件(Postgres)、`cargo xtask cn-e2e` 13 件成功。cn-core の rendezvous の 3 件は Valkey が無いため
  時間切れ(本 branch の変更の外)。
- clippy(`cargo xtask rust-check`・`cn-check`、app-api は `iroh-integration-tests` つきでも)・fmt・oversized-files・
  ipc-types・iroh resource contract は成功。

## 未決(ユーザーの判断を待つ)

- Dome instance の配置と参照: id は context と owner から決まり時刻を持たない。切替後も旧 topic/channel replica へ書き、
  instance manifest・hosting record の読取りは手元だけで、remote の Dome は一覧に出ない
  (`metaverse_room_events_replicate_between_iroh_peers`)。bucket へ移すと発見が有界にならない。
- 非 owner の参加者数: 参加・退出 record は owner にだけ届く。非 owner の表には自分の record しか無く、参加者数は 1 と表示
  される(desktop-runtime の friend-only・friend-plus の restore の試験が非 owner に 2・3 を求める)。
- 更新前からの参加者: owner の参加者の表は、更新後に届いた record だけを持つ。更新前に参加した相手は、次の参加・退出・
  redeem まで rotation の宛先に入らない。
- 友達の友達: profile を開いても、相手が他の相手を指す follow は手元だけを読む(R5-C)。旧 sync が無いので導けない
  (`social_graph_derives_friend_of_friend_and_clears_after_unfollow`)。
- 自分の別端末の投稿: 自分のプロフィールは手元だけを読む(R5-C)。同じ account の別端末の投稿は出ない
  (desktop-runtime `profile_timeline_reads_author_public_posts_across_untracked_topics`)。

## 既知の制約

- rotation は参加者の表を 128 件ずつ読み、宛先ごとに grant を作って outbox へ積む。操作の中で参加者数に比例する。
- friend-only の関係の確認(`stale_participant_count`)は、現 epoch の参加者を 128 件ずつ読む。
- 判断 4 の契機の「author の hint」は wire に無い。author の lease の開始と日の境界で読む。

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
| AC-2 | 各 record を §2 の replica と key へ。日の境界・再試行で宛先が変わらない | 実装済み(版つき cursor と Dome を除く)。app-api `bucket_writer` の 3 件 |
| AC-3 | 切替後は旧 replica へ書かない | 実装済み(Dome を除く)。同上 |
| AC-4 | 旧 sync なしの受信(hint の exact 読取り、lease の開始・再接続・日の境界の 1 ページの読み直し、通知) | 実装済み。実 Iroh `real_iroh_hint_exact_read_and_bounded_rereads_receive_new_posts_without_sync`、`the_day_boundary_reread_waits_until_the_next_bucket`、規模 `reads_do_not_grow_from_one_thousand_to_one_hundred_thousand_entries`(新着の受信を hint の読取りで数える) |
| AC-5 | 参加・退出 record と handoff grant の account 経路の配送、owner の SQLite 保存 | **未実装** |
| AC-6 | client の旧 sync の撤去 | 実装済み(sync status の診断の置換は未確認)。docs-sync `local_bucket_reads_stay_idle_after_seed_reapply_and_close_preserves_data`(書込みで開いた namespace も同期を始めない) |
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
| lease の task | `scope_receive.rs`(`spawn_subscription_task`・`apply_content_hint`・`reread_scope`) | replica を開かない。hint の exact 読取り、開始・作り直し・日の境界で 1 ページ |

## 撤去したもの・残したもの

- 撤去(client): `start_sync` の呼出し、`sync_requested`・`sync_peer_ids`、`reapply_sync_peers`、`restart_replica_sync`(trait 既定と
  `ReloadableDocsSync` の転送を含む)、`doc_start_sync`、`ensure_replica` の sync 分岐と `LocalThenRemote` の暗黙の同期開始、
  docs の event による反映・通知・窓の追いつき・recovery tick・backoff・probe・通知の baseline、空ページのときの再開と
  cooldown の台帳(`replica_sync_restart_deadlines`・`empty_recovery_candidates`)、直前の epoch の replica の保持、
  `IrohDocsNode::docs_sync_handle` と重複 field、`explicit_docs_sync.rs`、docs-sync の `connect_candidates` の試験と relay の sync 試験。
- 残した: `set_seed_peers`・`learn_peer`(reader の候補台帳)、`DocReadProtocol`(SyncHandle は page reader だけが使う)。
- 撤去(CN): `worker.rs` の `mpsc::unbounded_channel::<ReplicaEvent>` と生産・消費、`refresh_pass` の旧 replica 経路、`participant.rs`
  (legacy selector 一式)、`PublicReplicaReadMode`、migration `202609260003_drop_legacy_scope_cursor.sql`。失効 scope の巡回と R5-F の回収は
  `maintenance.rs` の `IndexMaintenance` へ移した。

## 更新案内(リリースノートの下書き)

配布のリリースノートは tag 間の commit から自動生成する(`docs/runbooks/release.md`)。手で足す項目の規則は無いため、文面をここに残す。

> この版から投稿・リアクション等の保存形式が変わります。更新した端末は、手元のデータの移行が済むと自動で新しい形式へ切り替わります。
> 切替後は、旧版の端末が書いた新しい投稿はこの版に表示されません(旧版のまま使っている相手には更新を案内してください)。
> 手元に保存済みの投稿・ブックマーク等はそのまま表示できます。

## 検証(局所)

- store: `protected_migration` と `migrations` の試験、schema golden を再生成(世代数 42)。
- app-api: lib 440 件成功(旧 sync の event 経路を前提にした試験は撤去、integrity の試験は hint の取込みへ、規模の試験は hint の exact 読取りへ改めた)。
  実 Iroh の受信の試験 2 件成功。mutation check: 開始時の読み直しを外す・hint の読取りを外す・通知を外すと、それぞれ失敗することを確認して戻した。
  bucket writer の試験は、切替の判定を常に偽にすると 3 件とも失敗することを確認した。
- docs-sync: 65 件成功(1 件は既存の ignored)。
- desktop-runtime: 切替の試験 1 件。
- CN: cn-indexer 全件、cn-core の migration、`cargo xtask cn-e2e` 13 件(CN 部分の担当の記録)。

## 未解決・残作業

- AC-5(参加 record と handoff grant の account 経路の配送、owner 側の SQLite 保存とページ読み)。旧 sync を撤去したため、
  現状は owner の参加者数・rotation の宛先と、参加者の新 epoch への移行が手元に届いた record だけで動く。
- timeline/profile/thread の版つき cursor(既存の `created_at` から bucket が一意に決まる)。
- Dome instance の配置(id に時刻が無い)。
- sync status の診断(`last_docs_activity_at` 等)の置き換えの確認。

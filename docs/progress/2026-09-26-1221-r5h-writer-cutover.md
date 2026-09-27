# #1221 R5-H: writer 切替と旧定常同期・暫定選択器・旧 queue の撤去

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R5-H と、2026-09-26 のユーザー決定「R5-H 旧sync撤去後の受信・配送・索引・案内」
「R5-H 参加recordのowner配送」、2026-09-27 のユーザー決定「R5-H 旧sync撤去後の発見と表示」。基準は #1374 merge `051e1bfb`。依存の R2-C・R4-D・R5-D・R5-F・R5-G は完了済み。

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
| AC-2 | 各 record を §2 の replica と key へ。日の境界・再試行で宛先が変わらない | 実装済み。app-api `bucket_writer` の 3 件。版つき cursor は作らない(既存 cursor の `created_at` から bucket が一意。ADR 0054 §3)。Dome の instance と hosting の記録は owner の制御領域の instance id の key(決定 1。app-api `instance_lookup_reads_a_constant_number_of_docs_records`) |
| AC-3 | 切替後は旧 replica へ書かない | 実装済み。切替後の本人投稿は保護所有先へ入る(desktop-runtime `own_posts_written_after_the_switch_are_protected_and_restored`)。Dome は app-api `switched_dome_records_skip_legacy_replicas_and_private_ones_stay_in_the_channel`(作成・hosting・接続・削除・private の作成と hosting の後に旧 topic/channel replica に Dome の記録が 0 件) |
| AC-4 | 旧 sync なしの受信(hint の exact 読取り、lease の開始・再接続・日の境界の 1 ページの読み直し、通知) | 実装済み。実 Iroh `real_iroh_hint_exact_read_and_bounded_rereads_receive_new_posts_without_sync`・`a_second_reread_reads_the_head_again`・`the_day_boundary_reread_takes_in_a_post_whose_hint_was_dropped_the_day_before`、`the_day_boundary_reread_waits_until_the_next_bucket`、プロフィールの手元の author bucket(`profile_page_one_shows_posts_written_after_the_switch`)、private の通知 offer(`a_private_offer_from_a_switched_writer_creates_a_mention`)、規模 `reads_do_not_grow_from_one_thousand_to_one_hundred_thousand_entries`(新着の受信を hint の読取りで数える) |
| AC-5 | 参加・退出 record と handoff grant の account 経路の配送、owner の SQLite 保存 | 実装済み。app-api `participant_leave_while_the_owner_is_offline_arrives_after_the_owner_restarts`・`a_join_record_arriving_after_the_rotation_still_gets_the_handoff_grant`、private_channels の friend_only・friend_plus・invite・leave、store `private_channel_participants_keep_the_newest_record_and_page_by_pubkey`。参加者数は owner だけ(決定 2。desktop-runtime friend-only・friend-plus の restore、frontend `settings panel shows the participant count only on the owner device`)。更新前からの参加者の移行(決定 3。app-api `legacy_participants_move_to_the_table_one_bounded_window_at_a_time`・`a_participant_from_before_the_update_receives_the_first_rotation_grant`) |
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
| live/game の作成・更新 | `scope_write_replica`・`session_write_replica` | 更新の envelope の署名時刻の bucket に最新 state と署名済み envelope。日が変わった更新は更新の日の bucket へ移して locator を置く |
| profile・follow・block・asset・Dome preset/移動 | `persist_author_event` | 制御領域の key はそのまま、更新時の author bucket に署名済み envelope |
| lease の task | `scope_receive.rs`(`spawn_subscription_task`・`apply_content_hint`・`reread_scope`) | replica を開かない。hint の exact 読取り、開始・作り直し・日の境界で 1 ページと session の索引(種類ごとに 64 件)。読み直しは照合の台帳を使わず、毎回各 bucket の先頭から読む |
| session の表示 | `set_session_display` → `read_session` | 手元に無ければ provider から exact に読む。remote の manifest は provider から取得。projection に行が無い session は hint・読み直し・表示の channel の scope で探す |
| session の hint | `GossipHint::SessionChanged { sent_at }` | 送った時刻を載せ、同じ session の続く更新を gossip が重複として落とさない(旧版は無視する) |
| private channel の参加・退出 | `record_private_channel_participant` → `queue_epoch_control` | 手元の docs と参加者の表へ置き、owner でなければ epoch の鍵で封じて dm_outbox(`epoch-control:`)へ。DM の再送 owner が `EpochControl` で送り、ACK で消す |
| 回転 | `distribute_epoch_handoff_grants` | 参加者の表を 128 件ずつ読み、grant を旧 replica に置いて各参加者へ届ける。回転の後に届いた直前の epoch の参加 record には `grant_current_epoch_to_late_participant` で現 epoch の grant を送る |
| 参加者数の表示 | `private_channel_diagnostics` | owner の端末だけが参加者の表の現 epoch の人数を返す。owner 以外は `None`(CLI は null、画面は出さない) |
| 保護移行 kind `owner_participants` | `migrate_legacy_private_channel_participants` | owner の channel の現 epoch の旧 docs の参加 record を、pubkey の hex の接頭辞の窓(128 件まで。埋まれば 1 桁細かく)ずつ参加者の表へ移す。位置は保護移行の台帳 |
| Dome の anchor | `dome_anchor_candidates`・`dome_anchors`・`dome_record_replica`・`read_dome_record` | Dome の作成時刻(session の state の `created_at`)の scope bucket(private は現 epoch、回転の前の epoch の同じ日の bucket も読む)。日で変わらない。作成時刻は行の session の state(手元に無ければ session を読む)、行が無ければ公開は制御領域の Instance、private は locator から |
| Dome instance・hosting・layout・削除の記録 | `persist_dome_instance_manifest`・`persist_dome_hosting_records`・`persist_dome_layout_commit`・`save_dome_deletion` | 公開は owner の制御領域の instance id の key、private は anchor(hosting は最後の epoch の分を 1 key)。読取りは exact、無ければ旧 context replica の手元 |
| session(live・game・Dome)の書込み先 | `session_write_replica`・`persist_session_envelope_and_locator` | 切替後は、更新の envelope の署名時刻の scope bucket(private は現 epoch)。日が変わった更新・旧 replica・旧 epoch の bucket の session はその日の bucket へ移し、locator(`sessions/<kind>/<id>/locator`)を置く。書き手も読み手と同じく bucket と署名時刻の一致を確かめる |
| session の読取り | `session_target_candidates`(`session_locators`)・`reread_sessions` | 現在と直前の bucket の locator が指す replica を先に読む。読み直しは locator の key も拾う。private の bucket は capability を持つ手元の docs からも読む |
| Dome の接続 | `service/dome_connection_store.rs` | 提案した Dome の anchor に書き、知っている Dome の anchor と旧 context replica を読む。切替後に旧 replica へ書こうとすると `dome_connection_legacy_guard` が拒む |
| private の Dome の locator | `spawn_owner_dome_heartbeat_task`・`persist_dome_locator` | owner の端末の hosting が 1 日 1 回、channel の現 epoch のその日の bucket に anchor を置く |
| Dome の一覧・接続の Instance | `append_context_domes`・`heartbeat_dome_owners`・`list_context_dome_instances` | 手元の行の owner・自分・heartbeat の host(context と合わせて instance id を導けるとき)・接続の端点の owner について exact に読む。replica を走査しない |
| author の lease の follow の窓 | `hydrate_author_keys`(`AuthorKeyReader::remote_follow_keys`) | provider の follow の窓(512 key、手元と同じ昇順・docs author 指定)を 1 ページ読み、手元に無い key を provider から反映 |
| プロフィールの列 | `profile_timeline_page` | 手元の旧 author replica と、cursor が選ぶ author bucket のうち手元にあるもの(`has_local_replica`。namespace を作らない)を合わせる。自分も含め、手元のページが埋まらないときだけ remote の旧 author replica と author bucket を読む |
| 制御 record の受信 | `ingest_epoch_control_offer` | owner は参加者の表へ(直前の epoch の参加 record なら現 epoch の grant も送る)、参加者は grant を手元の旧 epoch の replica へ置いて ACK |
| private の通知 offer の受信 | `ingest_private_notification_offer` | 現 epoch の旧 replica と、現 epoch の bucket(切替後の送り手)を受け付ける。bucket と作成時刻は投稿の検証で確かめる |
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

2026-09-27(決定 1〜5 の実装後、`999a135e` の時点)。

- app-api(`iroh-integration-tests`): lib 494 件成功。desktop-runtime: lib 319 件成功。
- harness(`RUST_MIN_STACK=67108864`、`--test-threads=1`): 23 件成功。kukuri-cli: 全件成功。store: 169 件成功。
- scenario: `community_node_public_connectivity`・`community_node_index_query_client`・`community_node_report_routing`・
  `community_node_trust_relation_client`・`e2e-smoke` 成功。
- frontend: tsc、vitest(components/extended・lib/api・shell の channels・presentation・data・privateChannelEntry)
  49 file 433 件成功。
- `cargo xtask rust-check`(clippy)・fmt・`ipc-types --check`・oversized-files(baseline を更新)成功。
- mutation check(外すと失敗することを確認して戻した): 決定 1 は heartbeat の owner の導出(単体の一覧の試験が失敗)と
  制御領域の remote 読取り(`metaverse_room_events_replicate_between_iroh_peers` と単体の試験が失敗)。決定 2 は owner
  だけの人数(desktop-runtime の 2 件が失敗)と画面の条件(frontend の試験が失敗)。決定 3 は参加者の表への保存(実 Iroh の
  試験が失敗)と接頭辞の細分(単体の試験が失敗)。決定 4 は provider の follow の窓(実 Iroh と単体の試験が失敗)。決定 5 は
  自分の分岐(単体と `static_peer::profile_timeline_reads_author_public_posts_across_untracked_topics` が失敗)。
- CN の crate は変更していないため、CN の結合試験と cn-e2e は前回(引継ぎ前)の結果のまま。
- 追加決定(Dome の残りの記録と private の Dome)の後: app-api(`iroh-integration-tests`)lib 496 件、desktop-runtime
  lib 319 件、harness 23 件成功。`cargo xtask rust-check`・fmt・`ipc-types --check`・oversized-files(baseline を縮小)成功。
  mutation check: private の instance を制御領域へ置くと 2 件とも失敗、locator の読取りを外すと参加者の一覧の試験が
  失敗、接続の記録を旧 topic replica へ書くと旧 replica の試験が失敗、Dome の作成を旧 replica にして書込み先の移し替え
  も外すと接続の旧 replica の拒否(`dome_connection_legacy_guard`)で失敗。
- 追加決定(回転後の session と切替前の live・game)の後: app-api(`iroh-integration-tests`)lib 499 件、desktop-runtime
  lib 319 件、harness 23 件成功。`cargo xtask rust-check`・clippy・fmt・oversized-files 成功。試験は app-api
  `session_rehome` の 3 件(切替前の live・game を切替後に更新しても旧 replica のキーが増えず読み手に届く、回転後の更新が
  旧 epoch の bucket に書かれず新 epoch の参加者は読め外れた参加者は読めない、現在の bucket の locator が state の replica
  へ導く)。mutation check: 旧 epoch の bucket を書込み先に残すと回転の試験、旧 replica の公開の session を移さないと切替前の
  試験、locator の読取りを外すと locator の試験が失敗。

以前の記録(引継ぎ前): docs-sync 66 件、iroh-node 47 件、transport 136 件、blob-service 12 件、core 145 件成功。
cn-indexer 全件(Postgres)、`cargo xtask cn-e2e` 13 件成功。cn-core の rendezvous の 3 件は Valkey が無いため時間切れ
(本 branch の変更の外)。

## 2026-09-27 の決定の実装(2 回目の引継ぎ)

- 決定 1 Dome: instance と hosting の記録を owner の制御領域の instance id の key へ置き、旧 context replica へは
  書かない。hosting は最後の epoch の分を 1 key(epoch ごとの key の prefix 全件読みを撤去)。読取りは制御領域の
  exact、無ければ旧 context replica の手元。訪問者は手元の行の owner と heartbeat の host(context と合わせて instance
  id を導けるとき)から owner を知る。Dome の接続の Instance も context の replica の prefix 全件読みをやめた
  (`service/dome_instance_support.rs`)。
- 決定 2 参加者数: `JoinedPrivateChannelView.participant_count` を `Option` にし、owner だけが返す。CLI の schema は
  nullable、IPC 型・views の golden・viewsContract を再生成、画面は値があるときだけ出す。
- 決定 3 更新前からの参加者: 保護移行の kind `owner_participants`(`migrate_legacy_private_channel_participants`)。
- 決定 4 友達の友達: author の lease の読み直しで provider の follow の窓(512 key)を 1 ページ読む。
- 決定 5 自分の別端末の投稿: `profile_timeline_page` の自分の分岐を撤去。

## 監査 FAIL の修正(2026-09-27、PR #1375)

- B1 日が変わった session の更新: 書込み先を署名時刻の bucket にし、移したら locator。書き手の検証にも bucket と
  署名時刻の一致を入れた。試験 `session_rehome::a_session_updated_on_a_later_day_stays_readable`(前日の bucket を
  source に前日の作成時刻で書き、live・game・private の game・Dome の chat の読み手と owner の次の操作を確かめる)。
- B2 プロフィールの 1 ページ目: 手元の author bucket を合わせる。試験 `profile_page_one_shows_posts_written_after_the_switch`
  (自分と、同じ docs を読む別の端末)、docs-sync `has_local_replica_does_not_create_a_namespace`。
- B3 読み直しの位置の持ち越し: 読み直しは照合の台帳を使わない。試験は AC-4 の 2 件(実 Iroh)。
- B4 private の通知 offer: 現 epoch の bucket を受け付ける。試験は AC-4 の 1 件。
- B5 private の Dome の anchor: Dome の作成時刻から導く。試験 `dome_placement::private_dome_hosting_records_stay_at_the_creation_day_anchor`
  (切替前と回転前の Dome の作成時刻を前日にし、hosting の記録が前日の bucket にあり、行を持つ参加者が読め、owner が閉じる)。
- B6 CLI の 3 台の daemon の試験: 参加 record が outbox の周期(約 2 秒)で届く前に回転し、b に grant が送られなかった。
  直前の epoch の参加 record に現 epoch の grant を送る。試験 `a_join_record_arriving_after_the_rotation_still_gets_the_handoff_grant`。
  Linux(Docker)で回転の伝播は通るようになったが、同じ試験は後段の Dome の接続(`process_e2e.rs:475`)で失敗する
  (下の「未決」)。
- 検証: app-api(`iroh-integration-tests`)lib 506 件、desktop-runtime lib 319 件、harness 23 件、kukuri-cli(Windows)、docs-sync 成功。`cargo xtask rust-check`・fmt・oversized-files・`ipc-types --check` 成功。Linux の `process_e2e` は Docker で回転の伝播まで通り、Dome の接続で失敗(未決)。1 回だけ `private_live_session_reaches_a_member_through_the_channel_hint` が全件実行の負荷で時間切れ(単独 4 回と全件の再実行は成功)。
- 時刻の注入: app-api に時計の抽象は無く、envelope の署名時刻は core が現在時刻で付ける。日をまたぐ場面は、前日の
  bucket・前日の作成時刻・前日の時刻で署名した投稿を置くことで作った。

## 未決(ユーザーの判断を待つ)

- 別の端末の Dome の接続の記録: 接続の提案・選択・合意は、提案した Dome の anchor(owner の端末の docs)にだけ
  あり、読取りは手元だけ(下の既知の制約)。受け手の端末は提案を読めず、CLI の `three_real_daemons_exchange_content_and_preserve_private_boundaries`
  (Linux の CI だけで動く)が `list_dome_connection_topology` の伝播期限で失敗する。選択肢: (a) `dome-topology` の hint と
  topology の読取りで、知っている Dome の anchor の接続の key を provider から有界に読む、(b) 提案・合意を受け手の owner
  へ account 経路(receive offer)で届ける、(c) 試験の期待を変える(ユーザー判断が要る)。

次は 2026-09-27 の追加決定で解消した。

- Dome の残りの記録(session・接続・layout・削除)も旧 replica へ書かない: session と接続は Dome の anchor(作成時の
  scope bucket)、layout・削除は公開なら owner の制御領域、private なら anchor。
- private channel の Dome の instance・hosting・layout・削除は公開の領域へ置かず、channel の bucket(anchor)に置く。
  参加者は heartbeat と locator、手元の行から見つける。試験は app-api
  `a_participant_lists_a_hosted_private_dome_and_an_outsider_cannot`(行の無い参加者は一覧に出し、参加していない端末は
  読めない)と `switched_dome_records_skip_legacy_replicas_and_private_ones_stay_in_the_channel`(制御領域と公開 bucket に
  private の Dome の記録が無い)。

## 既知の制約

- rotation は参加者の表を 128 件ずつ読み、宛先ごとに grant を作って outbox へ積む。操作の中で参加者数に比例する。
- friend-only の関係の確認(`stale_participant_count`)は、現 epoch の参加者を 128 件ずつ読む。
- 判断 4 の契機の「author の hint」は wire に無い。author の lease の開始と日の境界で読む。
- follow の窓は author の lease の開始と日の境界ごとに最大 512 key を provider から読む(手元の docs に無い key は毎回
  読む。30 秒の期限で打ち切る)。block の窓は手元だけ。
- 更新前からの参加者の移行は、接頭辞の窓 1 つを 1 ステップで読む。空の接頭辞も 1 ステップを使う(窓が埋まった
  接頭辞ごとに最大 16)。
- CN が hosting する Dome の heartbeat は host が CN で owner を導けない。行の無い Dome は heartbeat だけでは見つからない
  (手元の行がある Dome は owner の制御領域から読む)。「owner の profile から」は、行と heartbeat の host の owner として
  実装した(owner の profile の key から Dome を列挙する経路は作っていない)。
- 更新前に作った Dome の instance は旧 context replica にだけあり、owner が instance を更新(customize・asset の追加・
  移動・削除)するまで訪問者の一覧に出ない。
- 切替の前も公開の Dome の instance と hosting の記録は owner の制御領域にだけ書くため、旧版の端末は新しい公開の
  Dome を切替の前から一覧に出せない(private は切替前は旧 channel replica)。
- 接続の記録は、提案した Dome を読み手が知っている(行・heartbeat・自分)ときだけ読める。接続の記録の読取りは手元だけ
  (旧 sync の撤去後の既存の挙動のまま。provider からは読まない)。
- private の Dome の locator は owner の端末の hosting だけが置く。CN の hosting・hosting していない Dome は、行が無い
  参加者には見つからない。
- 切替前に作った Dome の session は、切替後の最初の書込みで更新の日の bucket へ移す。移す前の行(旧 replica)を持つ
  訪問者は、移した後の更新を読むまで旧 replica の版を見る。
- session を移した locator は移した日の bucket にだけあり、読み手は現在と直前の bucket の locator を読む。3 日以上
  更新の無い session を、行を持たない読み手は locator から見つけられない(行の source と id の時刻からは読む)。
- 回転の後に届いた参加 record に grant を送るのは、直前の epoch の参加 record だけ(2 回以上の回転をまたいだ参加 record
  には送らない)。参加 record の再送ごとに grant の行を積む(ACK で消える)。
- プロフィールの手元の author bucket は、手元に namespace がある bucket だけを読む。他人の author bucket は手元に無い
  ことが多く(remote の読取りは namespace を取り込まない)、手元の旧 `author::` の行が limit 件以上あると、他人の切替後の
  投稿は provider から読まれず 1 ページ目に出ない(remote は手元が埋まらないときだけの既存の形)。
- Existing-gap: Dome の移動の記録と Preset は、この PR より前から owner の公開の制御領域にある(private の context の
  Dome でも公開)。今回は直さない。
- 他人の Dome の一覧は、一覧の表示の中で owner の制御領域を provider から読む(Preset と同じ。一覧の上限と 30 秒の
  期限で有界)。

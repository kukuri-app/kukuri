# #1221 通信作業のinventory

基準: `984a491a3430f1da105bfe8ff49a877cbab4690f`。設計: [ADR 0055](../adr/0055-demand-owned-network-work.md)。
状態: **P2調査中**。以下はコードを読んで確認したgroup。NET-AC-1の全件列挙完了とはまだ扱わない。
`.codegraph/` は存在するが `codegraph explore` は「indexなし」を返したため、索引を作成せずrgとsource読みに切り替えた。
全件検索の途中にある `#[cfg(test)]` でファイル後半を捨てない。productionの後にtestがある場合も、
testの後にproductionが続く場合もあるため、module境界とcallerを確認する。

## 確認したgroup

pathの `service/` は `crates/app-api/src/service/`、`cn-runtime/` は
`crates/desktop-runtime/src/community_node/` を指す。全memberの逆引きはP3差分を入れる前に固定する。
intervalは現行値であり新設計の推奨値ではない。

| ID / memberと場所 | trigger・間隔・対象 | 現行の停止 / 連鎖 | 総数への依存 / 配置 | guard・検証 |
| --- | --- | --- | --- | --- |
| N01 `PeerAddrBook::{ranked_peers,set_seed_peers,record_learned_peer}`、`transport/src/peers.rs` | ticket/learn/seed/取得。account DBの索引窓 | learnedはaccount別30日/64MiBで回収。docs側はreapplyへ進む | 1選択4peer、source各4件。旧全件`merged_peers`を撤去。legacy sync内のpeer保持はR5-H | scope別候補、NW-5/6 |
| N02 `RemoteFetchRetryState::{begin,finish}`、`iroh-node/src/remote_fetch.rs` の取得入口 | local miss、retry。全候補。30秒は実行時、接続5秒/転送15秒 | 通常walkは待機者cancel後も継続。in-flight task→permit待機→接続 | 実行8の外に無制限の待機とcooldown走査。P3受付 | NW-2/3/4、LocalOnly |
| N03 `TopicWarmupCoordinator::warmup_peers_once/warmup_peer`、`transport/src/iroh/topics.rs` | topic join/peer追加。direct 250ms〜5秒、relay 1〜10秒 | deadline/neighbor成立。移動する4候補窓だけを走査し、同時2future・共有2dial枠を超える試行は待機せず延期 | peerごとのspawn/permit待機を撤去。topicごとのretry taskと初回bootstrap全件materializeはN04残件 | NW-2/5/6 |
| N04 `ensure_hint_topic/extend_active_topic_peers/remove_topic_state`、同上 | hint subscribe/publish、peer変化。選択窓だけをtopicへ追加 | 初回warmupはreceiverのDrop guard、更新warmupはtopic stateの単一handleが所有。topic解除/置換/shutdownで先に全taskをabortし、終了を待つ。世代closed/通知と登録前lockで旧task復活を防ぐ | 初回bootstrapは最大4候補のcursor読取り。topic数に比例するreceiver/retry taskとSDK内部peer保持は後続条件 | NW-5/7 |
| N05 docs `reapply_sync_peers` とopen/start/subscribe、`docs-sync/src/iroh_sync.rs` | seed/学習/restore。cached replica集合 | local-onlyはsync昇格しない。registry guard内で再適用 | replica×peer、同期requested全体。P3差分/P4bucket | NW-3/5、LIFE契約 |
| N06 `close_replica_owned/close_replica_under_guard`、`iroh_sync_lifecycle.rs`、`iroh_local_source.rs` | close/revoke/既存source読取り。最大32所有task | caller cancel後も所有。leave失敗隔離、停止task完了 | P1で有界な停止基盤。再監査し直さずP3接続 | NW-7、P1監査/CI |
| N07 `ensure_topic_subscription/spawn_topic_subscription/maybe_restart_*`、`service/timeline_subscription_support.rs` | timeline操作、empty/recovery、欠損本文 | subscription registryのtask終了/明示再起動。docs/blob/withdrawal取得を起動 | topic登録、期限map、背景spawnの累積。P3 | NW-1/3/4/5 |
| N08 `spawn_subscription_task/ensure_joined_private_channel_subscriptions`、`service/private_channels_support.rs` | private参加/表示/restart。recovery tick 1秒 | audience/epoch確認、event/hint/復旧。channel退出で停止 | 全joined/各channelの周期/期限。P3/P4 | NW-3/7/9 |
| N09 `ensure_author_subscriptions_for_rows/spawn_author_subscription`、`service/social_runtime_support.rs` | row表示、author操作、1秒catchup、bootstrap recovery | docs/hint receiverとbootstrap task。著者購読registry | 著者登録累積/各author周期。P3/P4 | NW-1/5、author署名/権限 |
| N10 `rebuild_author_relationships/current_mutual_direct_message_peers/schedule_direct_message_reconcile`、`social_runtime_support.rs/social_helpers.rs` | social変更/startup/DM操作 | 全following/followerとFoF確認→DM reconcile | graph全体の列挙と全subscription差分。P3対象別索引 | NW-8、mutual未確認時拒否 |
| N11 `reconcile_direct_message_subscriptions/spawn_direct_message_subscription/direct_message_topic_snapshot`、`service/direct_messages_subscription_support.rs` | mutual peer単位、旧pairwise hint受信 | 受信taskはregistryで停止。peer別2秒outbox timerはN69で撤去し、再送をaccount共通ownerへ移管 | N64のpeer別pageはDM statusにも利用。DM相手数×旧pairwise受信task、起動時の全件読取りと全peers snapshotはP3残件 | NW-8/9、ACK/tombstone |
| N12 `session_projection.rs`、`session_display.rs`、UI `SessionVisibility/useSessionDisplay` | 表示observer、window/columnの非表示 | 対象64/observer64、表示futureをcancel、保存前再guard | 既存の有界需要をP3へ接続。参加sessionの寿命とは別 | NW-4/7、既存session manifest tests |
| N13 `run_community_node_session_maintenance_once/start_community_node_session_scheduler`、`cn-runtime/scheduler_support.rs` | runtime起動、15秒、設定node全体 | `MaintenanceTasks`がjobごとのfutureを所有、Skip、shutdown停止 | 所有は既存。node全体再列挙とstatusからselfheal。P3due索引 | NW-6、mixed認証/同意 |
| N14 `maybe_self_heal_community_node_connectivity/repair_community_node_connectivity`、`cn-runtime/reconnect_support.rs` | sync status不健全、期限付きbackoff | reconnect成功でreset、seed/購読再適用へ連鎖 | 全active/全ready nodeへ波及。P3差分と観測 | NW-5/6/7 |
| N15 `observe_sync_status_once/start_sync_status_observer`、`runtime/sync_status_observer.rs` | 3秒、全体snapshot | runtime shutdown。変更snapshotだけemit | 全peers/topic/nodeの再集計。P3増減集計 | NW-1/6 |
| N16 `commands/background_notifications.rs::spawn` とdispatch | 受信event + 60秒fallback、trayでも継続 | runtime/アプリ寿命。新規通知sequenceの64件ページ→OS dispatch | P3で全件読取りと同timestamp ID集合を撤去。各ページ後にaccount切替guardを解放し、local inbox/UI一覧は別経路 | NW-8、OS設定/二重toast |
| N17 `cn-indexer/src/participant.rs/scheduler.rs` | scope設定、docs/hint event、再評価 | post job台帳1,024、lease dropでcancel記録 | 現行の有界schedulerを再利用。scope境界/差分はP4 | CN-AC-1〜4、NW-9/10 |
| N18 UI `shell/data/timelineMerge.ts`、`slices/timeline.ts`、`viewModels/useTimelineViewModels.ts` | refresh/event/遡り/列切替 | UI observer寿命、再取得をapp-apiへ渡す | 累積窓のmap/merge/setState・cacheをP3で索引/窓化 | UI-AC群、100/1k/10k、focus/scroll/draft |
| N19 `join_live_session/stop_live_presence_task`、`live.rs/service/live_game_support.rs` | 参加中liveごと10秒、TTL30秒 | ended判定またはleave/shutdownでtask停止、hint送信とlocal presence更新 | 参加数に比例。P3の参加leaseへ登録、hiddenでは停止しない | NW-4/7、live終了契約 |
| N20 `spawn_owner_dome_heartbeat_task`、`dome_hosting.rs` | host sessionごと5秒、Delay | session不在/ID変更/署名失敗で終了。handleのownerなし | host session数、送信失敗時continue。P3 task所有 | NW-6/7、Dome lease/draining |
| N21 `dome_connection_support.rs` のconnection終了/blocked再評価 | 所有者の終了操作、drain期限待ち | caller future内sleep後に状態再読込み。全host runtimeへdrain | 全joined context/connection走査。P3索引差分、P4state配置 | 世代/owner guard、drain中cancel |
| N22 `wait_for_private_channel_epoch_snapshot`、`object_persistence_support.rs` | epoch参加/rotation、50ms間隔・全体10秒 | caller取消/期限で停止。metadata/policy/participantsを再取得 | participant全体の反復。P4個別owner/recipient確認 | NW-9、旧epoch grant経路 |
| N23 `spawn_reply_target_reflection`、`reply_target_support.rs` | 未反映の返信先、台帳受付後 | spawn後にpermit待機、所有handleなし、docs LocalOnly→本文remote | 台帳は既存、task所有とscope別取得はP3統合 | NW-2/4/9、署名/LocalOnly維持 |
| N24 `MissingBodyLedger`、`hydration_limits.rs` | 欠損本文、5秒/30秒/120秒/600秒、最大8試行 | RAIIで失敗記録、最大4実行/4,096記録 | 台帳は有界。上限は共通予算へ統合し履歴をリセットしない | NW-2/4、既存missing-body tests |
| N25 `runtime/mod.rs` の通知event転送、`host/mod.rs::replace_event_task/restore_desired_subscriptions` | account起動/restart、notify event | host転送taskは置換/shutdownでabort。runtime通知転送もruntimeが所有しshutdown完了待ち・Drop中止 | desired購読全件復元はP3残件。旧runtime task寿命はP3 ownerへ接続 | NW-7/8/10 |
| N26 `iroh-node/src/node.rs::apply_relay_config/shutdown`、`transport/src/discovery.rs/iroh/discovery.rs` | 起動/relay設定、endpoint.online待機 | node shutdownは所有付き。online待機はdetached | endpoint世代単位。P3で監視taskを所有し重複設定をno-op | NW-6/7、既存node終了契約 |
| N27 `cn-runtime/requests_support.rs/session_runtime_support.rs` | auth/consent要求、token/heartbeat/rendezvous/metadataの期限、設定変更 | HTTP timeoutと401再認証。ready node/seed集合を再適用 | 全ready node/購読/seedの再合成。P3差分/due索引 | NW-5/6、mixed node auth/consent |
| N28 `cn-core/src/rendezvous.rs::heartbeat`、`cn-user-api/handlers/bootstrap.rs::topic_rendezvous_heartbeat` | 認証・同意後のjoins/refreshes/leaves | request期限。15秒のtopic窓4件とtopic-peer TTL45秒、窓key TTL60秒 | P3でSMEMBERS全件を撤去。各窓16件を標本し最大64件だけ検証、候補返却は最大8件。旧SETはTTLで自然退役 | NW-2/5/8、auth/endpoint binding |
| N35 Tauri `LinkPreviewState::request/spawn_fetch`、`commands/link_preview.rs` | URL表示/操作。cache成功10分・失敗1分、connect3秒/request6秒 | 32 in-flight/4実行、cache128件/16MiB。caller取消後も取得継続、task handleは未所有 | 件数/bytesは既存で有界。P3の共通容量・task寿命へ接続し、SSRF/redirect/本文上限を維持 | NW-2/4、ADR0051 |
| N36 UI `useDesktopShellDataEffects/useAuthorTrustGateLookup/useTimelineContentAdvisoryLookup/useRuntimeEventBridge` | 表示/DM3秒、通知60秒、trust sweep30秒・debounce300ms。advisory300ms、500件batch | effect cleanupでtimer解除。表示/DMはhidden時抑止。mediaは最短retry時刻のtimer | 読込済み行/author/URLの全体処理とeventごとの通知読取りをP3で窓・差分・coalesce | UI-AC群、NW-1/8 |
| N37 `IndexerWorker::{spawn,run,full_pass,spawn_subscription}`、`cn-indexer/src/worker.rs` | 全scope再確認300秒、event debounce2秒、backoff5〜300秒 | watch停止、shutdown待ち10秒。timeout後のJoinHandle放棄は実停止を保証しない | unbounded event channel、scope/task/backoff全体、full pass。P4で有界queue/due cursor/owned停止 | CN-AC群、NW-2/7/10 |
| N38 `spawn_status_server/StatusServerHandle::shutdown`、`cn-indexer/src/status.rs` | 設定時の単一HTTP listener、status GET | graceful shutdownを5秒待つ。timeoutは強制終了ではない | status snapshotのscope集合はP4で有界な診断窓へ。外部への復旧要求は起動しない | CN診断、停止/未完了の区別 |
| N39 `timeline.rs` の投稿後hint送信 | 保存/反映後に3回、250/500ms待機 | caller futureの終了で残送信も終了 | 1投稿あたり固定回数。P3へ同じ送信目的を登録し、enqueueのOkを接続回復と解釈しない | NW-4/6、投稿保存維持 |
| N40 `private_channels.rs` のleave hint | private退出、全体peer有無判定後2秒timeout | hint失敗でもcapability/参加状態の除去へ進む | 全peers snapshotをP3の対象別観測へ置換。P1 close/revokeを維持 | NW-7/9、private leave |

検索候補のうち、次の入口は通信復旧の周期処理と区別する。

- `app_update.rs::{download_app_update,install_checked}` と `useAppUpdateScheduler`はhost単位の更新処理。
  確認は30分間隔、download/installは`try_lock`で一つのsessionに限定する。accountのpeer台帳を持たず、
  署名確認と既存のapp lifecycle guardを維持する。
- `commands/{device_backup,identity,os_notification,os_notification_windows,external_url}`、`file_dialog`の
  spawn_blocking/OS threadは明示的なlocal/OS操作。peer再接続timerとして数えない。
  backup/restoreやdesktop lifecycleからのruntime復帰はN25/27の入口として追跡する。
- `MetaverseScene`、focus/scrollのrequestAnimationFrame、draft保存debounceは描画/local処理。
  live/Domeの通信heartbeatはN19/20、room/connectionの取得はそのapp-api shared helperからN02/21へ追跡する。

## 上流APIで確認した前提不足

lockの対象はiroh 1.0.3、iroh-gossip 0.101.0、iroh-blobs 0.103.0、
iroh-docs 0.101.0（pin `e7233d14853cb4db9966e30050bac1e689cdeec8`）。

| ID | source / 動作 | P3で満たす必要がある契約 |
| --- | --- | --- |
| U01 | iroh-docs `engine/live.rs::start_sync` は保存済sync peerを引数peerへappendし、`join_peers`で各peerの同期を起動 | 保存peerは5件で有界だが選択外を再開し得る。公開 `net::connect_and_sync` の1回実行では保存peerを追加しないことを実証。native高水準startを経由しないowner構成を採る |
| U02 | 同 `leave` はsync停止とgossip quitを行う。進行中syncのJoinSetとneighbor経由のsync受付は別経路 | 停止応答後の旧世代のnetwork/保存とneighborによる選択外開始を制御。P1 closeの再検証ではなくowner統合のdelta |
| U03 | iroh-gossip `api.rs` は `join_peers` を公開し個別peer削除APIなし。sender/receiverの両方dropでtopic leave | 影響topicの全handle/task所有と解放。別topicを巻き込まず、残存sender cloneを残さない |
| U04 | iroh-gossip `proto/state.rs` はpeer切断を全稼働topicへ通知 | 対象topic数を有界にし、休止topicを内部stateへ残さない。意味上の全登録topicとは分ける |
| U05 | irohのendpoint IDは `iroh-node/src/node.rs` が永続鍵から復元する。DHTはendpoint IDを解決 | 既存endpointは署名bindingを再利用可能。鍵再生成時は旧bindingを有効扱いしない |
| U06 | iroh `remote_state.rs` のactorは接続/処理がない状態60秒で終了し、`remote_map.rs::cleanup`がsenderを除去する。mapped address表は別 | actor終了だけでaddress台帳の回収も済むとは扱わない。内部接続数とmapped addressの上限/解放を確認 |
| U07 | gossip hyparviewの既定active viewは5、passive viewは30、topicごとの有界集合 | per-topic上限を既存成果として再利用。全topicの合計/共有接続はowner予算と整合させる |
| U08 | iroh-docs `on_replica_event -> start_download` はremote entryから独自downloader/task/hash provider台帳へ進む。app-apiのblob受付を通らない | docsのdownload policyとownerへの取得委譲を設計する。neighbor content-ready経路と未実行hash台帳も確認し、app-api側8枠だけで全取得有界としない |
| U09 | iroh-gossipの既定message上限は4,096bytes。private replica IDの最大形はこれを超え得る | 最大長locatorをhintへ丸ごと入れない。固定サイズの暗号化参照から、署名provider/epoch制限付きで有界payloadを取得するwireを先に検証 |
| U10 | 公開 `actor::SyncHandle` と `net::{connect_and_sync,handle_connection}` はstorage actor、接続先1件、namespace受信callbackを組み合わせられる | `explicit_docs_sync_ignores_cached_peer_and_rejects_other_namespace` / `explicit_docs_sync_cancel_closes_remote_connection` が成功。production nodeも高水準`DocsApi`と同一Engineの`SyncHandle`を保持し、memory/persistentのstore layoutと再openを維持する。選択peer/受信拒否/cancelのための依存forkは不要。全caller移行、通知変換、内部address台帳は未完了 |

### U06/U07の回収漏れ修正

採用候補を固定して再現・回帰を確認した。irohは`adf5b0e0a5f36f73f11934bcbf2f4ce04ccd4155`
（上流#4447）、gossipは`c42f40a1346200ae2b18d41596a2a8a02047e600`（#161を含む#162）。
rootとstandalone Tauriの5packageを同じsourceへ揃え、型の二重化を避ける。
package version、MSRV、wire、永続形式を変更せず、両lockの無関係な依存edgeも維持する。

- mapped address: 100件の退役履歴で101件が残る失敗を再現。修正後は100/1,000件とも64件。
  active peerのmapping、relay逆引きの整理、再利用時の新mappingも`harness/upstream`のcontractで確認。
- gossip: topic lease終了後の実QUIC closeをweak handleで確認。元コードは12秒でも未終了、候補は約5秒で終了。
  別topic継続、同時dial、pending joinのquitと再joinも関連contractで確認する。
- これは協調peerの退役漏れの修正。64は全active actor込みの絶対上限ではなく、actor idleの60秒も別にある。
  relay mapのretain走査、remoteがstream/headerを終わらせない場合、pending joinの容量は共通ownerに残す。
  現行で未使用のcustom transportを、この修正で対応済みとは扱わない。

`TransportAddrUsage::Active`を接続生存の代用にした最初の試験は判定方法が不適切だったため、
before/afterの根拠に使わない。path cacheを観測しないweak connection終了通知へ修正してから、
旧gossip `2ce78afe`でも同じ失敗を再確認した。

irohのmapped address表は `mapped_addrs.rs::AddrMap` の正引き/逆引きHashMapであり、actorの終了と別の寿命を持つ。
内部接続とmapped addressの容量・停止、docsの進行中sync取消、blobs downloader内部の受付は残確認。
外側のSemaphoreや時間bucketだけでこれらも有界と主張しない。

## Sensitive sinkの逆引き

### 追加したbinding交換（P2実証、常設runtime未登録）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N29 | `fetch_receive_endpoint_binding` → endpoint connect/open_bi/read → `verify_for` | 候補1件、受付deadline、1,024byte、QUIC remote ID照合。完了/失敗/cancelでclose | `receive_binding_exchange_uses_authenticated_endpoint_without_cn`、`receive_binding_replay_from_another_endpoint_is_rejected`、`receive_binding_cancel_closes_the_connection` |
| N30 | Router → `ReceiveBindingProtocol::accept/serve` → binding write | try_acquireで2要求、2秒、1byte request。失効検証、scope取得なし | `receive_binding_full_server_rejects_instead_of_waiting` |
| N31 | `ReceiveBindingProtocol::new/replace` → binding更新 | account/endpoint/署名/時刻/単調更新。鍵は保持しない | `receive_binding_replacement_cannot_switch_account_or_endpoint`、core binding tests |

この3行は§4.1の限定実装範囲。署名bindingを取り交わすだけでは、account routeのgossip配信、
private capsule、旧DM outboxの移行を達成したとは扱わない。

### P3の本番binding登録（account route配送は未接続）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N53 | `IrohDocsNode::spawn` → Routerの`ReceiveBindingSlot` → QUIC binding応答 | node生成時は鍵なしで即拒否。2要求・2秒・1byte request・1,024byte応答。要求時にのみ300秒binding署名 | `receive_binding_slot_rejects_until_account_is_installed_and_signs_at_request_time` |
| N54 | runtime identity load → `SharedIrohStack::use_account_receive_binding` → node slot導入 | docs author設定後、同一accountのみ。別account導入失敗でも元の鍵を保持。未導入時は外部応答0 | `desktop_runtime_installs_binding_for_loaded_account`、`account_binding_is_live_only_after_identity_load_and_survives_stack_rebuild`、slotの別account test |
| N55 | stack rebuild / node shutdown → 新slot再導入 / 旧slot停止 | 切替前に新nodeへ同じ鍵を導入。旧nodeは停止開始で新要求を拒否、既存要求の終了を待ち鍵を解除 | 同じ実node rebuild test、slot clear test |

N53〜55は公開accountと接続endpointの対応だけを提供する。recipientのscope/DM mutual/private能力、
gossip offerと出典の照合、旧listenerの撤去やoutbox輸送先変更はまだ行わない。slotの署名鍵は
account runtime/node寿命内に保持し、binding応答に秘密値・参照本文を含めない。

### N25のlocal通知event転送task所有（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N58 | `DesktopRuntime::new` → `notification_inserted_notify`待機task → `RuntimeEvent::NotificationStatusChanged` | 同一account runtimeに1task。`shutdown_checked`でabort/完了待ち、Dropでもabort。旧accountのNotify保持からevent broadcastしない | `notification_event_forwarder_stops_when_runtime_shuts_down`、`notification_event_forwarder_stops_when_runtime_is_dropped`、lock分類contract |

N58のsinkはlocal runtime broadcastのみ。通知row生成、OS toast、network送信、hostのdesired購読復元には
触れない。host側の既存task所有とruntime側の世代を混同しない。

### 追加した暗号化参照（P2実証、I/O未接続）

| ID | 入口 → helper → sink | guard / 上限 | 対応contract |
| --- | --- | --- | --- |
| N32 | `seal_receive_offer` → 署名/ECDH/HKDF/AEAD → wire生成 | recipient/参照field/寿命を検証、平文880byte/wire2,048byte。network/storeなし | core `receive_offer_*`、実gossip `account_receive_offer_crosses_real_gossip_with_one_recipient_route` |
| N33 | `SealedReceiveOfferV1::decode/open` → AEAD復号/署名検証 → VerifiedReceiveOffer | decode前サイズ、版、nonce/cipher、署名sender/recipient/参照/期限。Verified型はscope許可ではない | wrong-recipient、tamper、reencryption forgery、payload上限 |
| N34 | `seal_private_receive_payload/PrivateReceivePayloadV1::decode/open` → epoch用途分離/AEAD → manifest | channel/epoch/secret一致、平文16,384byte/wire65,536byte。全epochの試行復号なし | private epoch/secret違い、relabel、最大入力、用途/連結の分離 |

N32〜34のsensitive sinkは暗号化/復号と返却値だけ。providerへのblob取得と永続mutationを追加する際は、
N29のbinding照合およびscope/mutual/失効guardを先行させ、禁止I/Oのtestを追加する。

| sink | 既知の入口group | 必須の支配guard / 禁止副作用 | 差分前の残確認 |
| --- | --- | --- | --- |
| endpoint connect / gossip join,publish | N01/03/04、N07〜11、N13/14 | owner受付、protocol/scope候補、同意、endpoint世代。無関係/休止topicのI/O 0 | upstream内部の接続保持/再試行、各public trait caller |
| docs open/start_sync/query/leave | N05〜09/17、CN source reader | LocalOnlyをsyncへ昇格しない。private能力/世代。閉鎖中の再open禁止 | writer/controllerとlegacy migration caller |
| blob remote fetch / persistence | N02/07/08/11/12/17 | mode/bytes/scope/取得gate、完了時再guard。private hashを無許可peerへ出さない | media、live/game/Dome、reply targetの全caller |
| CN HTTP / token / seed適用 | N13/14、requests/session support | node別auth/consent、401時停止、自己修復で別nodeを昇格しない | request helper全caller、明示設定setter/restore |
| notification/DM/outbox DB mutation | N10/11/16、新受信入口 | 署名、mutual/audience、dedupe/既読、ACK、tombstone | notification candidateから保存まで、backup migration |
| namespace/blob/projection GC | N06、P4回収 | 保護objectと依存参照、cursor、部分失敗の冪等再開 | GC/backup/restore/export/import全caller |

## 未分類を解消する範囲

次はgrep候補を拾っただけで、停止・連鎖までの確認が残る。全件inventory完了と主張しない。
一つずつ別タスクを増やさずP2でgroupへ統合し、既存の固定ACに関連しないものは理由付きで対象外へ分類する。

1. `app-api` の `dome_connections/dome_hosting/live/private_channels/timeline`、
   `service/{dome_connection_support,live_game_support,object_persistence_support,reply_target_support,mod}`。
   active sessionのheartbeat/終了、bounded hydration、共有read helperの再取得連鎖。
2. `desktop-runtime` の `host/{accounts,mod}`、`runtime/{mod,private_channels_game_api}`、`stack`、
   `community_node/{requests_support,session_runtime_support,session_state_support,http_client_support}`。
   account切替、stack probe/rebuild、認証/metadata/rendezvousの期限・全setter。
3. `iroh-node/src/node.rs`、`transport/src/{discovery,iroh/discovery,iroh/endpoint}`、
   pin済みiroh/iroh-docs/iroh-gossip/iroh-blobsの内部task・retry・peer cache。
4. CN participant/workerの登録点からingest/media/sourceまで、bucket writer/read/GCの全入口。
   [replica read inventory](replica-read-inventory.md)を再利用し、重複した全件調査をしない。
5. Tauriの `desktop_lifecycle/state/lib`、`commands/{link_preview,device_backup,os_notification}`、
   UIのpoll/subscribe/object URLを含む登録点。OS thread/単発blocking処理とnetwork retryを分ける。

`FakeTransport`、各tests/fixture内のsleep/spawn、エラーDTOの`retry_after_seconds`フィールドは
production schedulerとして数えない。ただしproductionと同一ファイルの場合はmodule境界で除外する。
updater、ファイルdialog、identity export、外部URL起動はそれだけで本Issueの新要件にしない。

## 終了条件

P2終了時に各groupのmember・caller・停止・連鎖と上流APIの実現可能性を確認し、未分類を0へ更新する。
固定AC/INVARとNW transitionへの対応を独立監査する。P3/P4では差分を入れたgroupだけを更新し、
同一headの成功監査と無関係な全suiteを繰り返さない。詳細な作業状態は#1221本文へ記録する。

## 共通受付の状態機械（P2/P3、I/O adapter未接続）

| ID | 入口 → helper → sink | guard / 上限 / 停止 | 対応contract |
| --- | --- | --- | --- |
| N41 | `NetworkWorkOwner::register_scope/revoke_scope` → scope逆引き → 有限scope/停止指示 | 認証済みscopeのtokenを発行。owner別・再登録別の一意世代、上限64。失効は対象scopeだけに適用 | ADMIT-4/5、revoked_scope / foreign_owner / tenfold_history tests |
| N42 | `admit/release_waiter` → key索引とwaiter集合 → 受付台帳 | 要求256、64waiters/要求、payload計4MiB。固定長key、scope現在性、LocalOnly拒否、同一metadataのみ合流。通常の実行は最後のwaiter離脱でも所有 | ADMIT-1/2/4、bounds / unchanged_demand / display_cancel tests |
| N43 | `start_next/expire/next_deadline/next_cancellation/complete` → lane/deadline索引 → 実行許可・停止・完了判定 | 実行8にStoppingも計数。4:2:1、期限に待機時間を含む。取消・期限切れ・失効の結果はDiscard。I/Oと保存guardはadapterが所有 | ADMIT-3/4/5、weighted_lanes / queued_deadline / running_expiry tests |

この3行のproduction callerはまだなく、追加したcontractから全public入口を通す段階。全caller検索は `rg -n 'NetworkWorkOwner|WorkAdmission|WorkCompletion' crates`。メモリ台帳以外のsensitive sinkは追加しない。4MiBはpayload計であり、固定長key・索引・waiterの管理領域は件数上限で別に有界にする。dispatch後のpayloadもcompleteまで予算へ計上する。executorによるI/O停止とguard直下の保存を後続で結合し、この部品の成功を通信全体の上限達成と混同しない。

## gossip I/Oの公開API実証（D9）

N01/U03/U04/U07のadapter前提として、`iroh::tests::controlled_gossip`の2 contractでnative peerとの双方向wireと未完結header中の取消を確認した。`proto::topic::State`/`topic::Message`を使い、membership本体は再実装しない。`proto::State`の外側Messageはtopicも含む内部配送型で、実wireはheader後にtopic Messageだけを書く点を区別する。所有connectionのDropでcloseし、nativeの内部RecvLoopへ取消を委ねない。

このfixtureは1topic/1peerの短い往復でtimerを発火しない。production入口は増えず、timer容量・複数topicの共有接続・再試行・受信/送信workerの上限は残る。`EndpointHooks`は送信前拒否とhandshake後観測、`RouterBuilder::incoming_filter`は受信spawn前選別に使用可能だが、接続試行futureの失敗/cancel精算はownerが持つ。

## N11のACK保存sink（共通route前の保護）

`spawn_direct_message_subscription`の受信 → topic照合 → `handle_direct_message_hint` → ACK署名/sender/recipient/導出dm_id一致 → `set_direct_message_acked_at/remove_direct_message_outbox`。このhelperのproduction callerは上記subscriptionだけ。新routeへの再利用時もこの会話境界を迂回しない。ACK-1/2のtestは他会話のoutbox・本文・送信状態が不変で、正しい相手のACKだけ削除することを確認する。修正前は会話ID照合がなく、正しい署名を持つ別相手のACKでoutboxが削除される失敗を再現した。

## 表示用取得の本番接続（P3）

| ID | 入口 → helper → sink | guard / 上限 / 停止 | 対応contract |
| --- | --- | --- | --- |
| N44 | app-api `SessionProjectionRegistry::schedule` → BlobService trait / stack proxy → `IrohBlobService::prepare_display_fetch` → `remote_fetch::prepare_display_fetch` → `NetworkWorkRuntime::acquire` | LocalOnlyの既存fast pathは別。remoteだけnode共通64lease/8枠、deadlineは受付から30秒、待機spawnなし、hash32byte。既存walk枠も準備成功前に取得 | DISPLAY-1/4、`display_admission_wait_is_included_in_total_budget`（修正前FAIL）、`display_admission_is_shared_across_services_using_one_node` |
| N45 | 準備済み表示future → `lease.cancelled`と実fetchのselect → 検証済み一時bytes返却 | deadline/closeを優先、finishで世代/期限判定。caller dropでleaseと取得future/旧permitを解放。app-api保存前のscope/token guardを維持 | DISPLAY-2/3、既存caller取消と新node終了の実QUIC tests、adapter queue/close tests |
| N46 | node `shutdown/shutdown_owned/Drop` → `NetworkWorkRuntime::close` → active scope失効と通知 | 受付を先に閉じ、queued/準備済み/実行中を取消。停止応答までは枠を保持し別nodeは独立。nodeの既存Router/endpoint停止は維持 | DISPLAY-3、`node_shutdown_cancels_display_fetch_without_returning_or_caching_bytes` / `display_node_close_does_not_stop_another_nodes_work` |

N44の全callerは `rg -n 'prepare_display_fetch' crates`、型の実装とstackのproxyを含む。remote helperへのproduction callerはIrohBlobServiceだけ。新台帳に秘密値/本文を保持せず、bytesの保存sinkは従来の`cache_and_project_displayed_manifest`でscope/tokenを再確認する。通常fetch・docs・gossipはこのadapterへ未移行なので、全networkの合計8枠達成とは扱わない。

## 明示的な通常取得の共通受付（P3）

| ID | 入口 → helper → sink | guard / 上限 / 停止 | 対応contract |
| --- | --- | --- | --- |
| N47 | BlobService/DocsSyncのremote helper（通常/一時/上限付き一時）→ `run_single_flight` → retry guard → `NetworkWorkRuntime::submit_fetch` | 非再利用のservice世代＋flight key、persistence/byte limit一致、最初のdeadline、64scope/64waiters。LocalOnly fast pathは入らない | FETCH-1/2、既存singleflight/result/取消/保存方針tests、waiter上限/cooldown-join test |
| N48 | 単一driverの期限/ready/task完了 → 実行枠取得済みfutureだけspawn → 成否callback → flight退役/結果通知 | 表示と通常の合計8。待機時spawn0、期限/cancel/panic/closeの精算、callbackとfuture Dropはlock外、結果反映は現在世代だけ | FETCH-1〜3、`ordinary_fetch_waits_for_the_same_capacity_as_display_work`、node close/panic/reentrant Drop tests |
| N49 | `RemoteFetchRetryState::finish/is_cooling_down` → retry_after/期限索引 | 3秒cache、1,024件、key256byte、容量時は近い期限から回収。全件retainなし。未送信outboxは対象外 | FETCH-4、`remote_fetch_failure_history_has_a_fixed_capacity`（修正前10,240件FAIL）、既存cooldown tests |

通常helperの全callerは `rg -n 'fetch_bytes_.*cooldown|run_single_flight|submit_fetch' crates`。runtimeのsubmit callerはremote_fetchだけで、ほかはtests。旧`RemoteFetchBegin::begin`の予約APIは互換/tests向けに残るが、productionの通常取得は使用しない。表示の既存walk permitは追加制約として残す。SDK内部の自動downloader/peer走査・全protocolのscope世代接続はU08等の残作業。

## blob peer healthとbounded fetch候補（P3）

| ID | 入口 → helper → sink | guard / 上限 / 停止 | 対応contract |
| --- | --- | --- | --- |
| N50 | account DB → docs/blob/gossipの独立候補scope → protocol別`BlobPeerHealth` | learnedは30日/64MiB、ticket/seedは保持。docsは実sync、blobは実転送、gossipは実neighbor成立のみ成功。別protocolのhealthで候補を追加しない | R2-A、account再open/候補cursor/実neighbor tests |
| N51 | remote blobのhash → `ranked_peers`のsource別cursorとrecent/success → 4peerのconnect候補 | source各4 ID、recent4/success2、address materialize最大12、選択最大4。新manual ticketも選択。`connect_candidates`のdirect/relay順は既存 | PEER-4、100/1,000履歴の読取り数・fresh ticketの連続取得・foreign health除外 |
| N52 | QUIC connect / blob転送 → 型付き欠損・stream応答・local故障 → health/cache | blob ALPNだけ。health/rate各1,024件、稼働attempt pinと世代照合、request window満杯は延期。型付き欠損/ERR_INTERNAL(3)/local errorを接続不良へ混ぜない | PEER-1/3、実Iroh欠損3mode・local store故障、2,048履歴/1,024 pin/rate tests |

N50〜52のsensitive sinkは選択された既存候補へのblob hash送信と一時観測cache。新候補はsource台帳に存在するIDへ限定し、scope/capabilityの検証をhealthや直近成功で代替しない。旧`merged_peers`は撤去、`available_peer_ids`は4件の窓に限定した。account再構築でdocs/blob/gossipの全候補snapshotを作らない。SDK内部のaddress/watch集合と旧docs syncはR5-Hで扱い、R2-Aだけで総通信の上限達成とはしない。

## N16のlocal OS通知dispatch差分（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N56 | `put_notification_if_absent` → SQLite insert trigger / memory index → 単調dispatch sequence | duplicateは番号を消費しない。既存rowはmigration時NULLのまま、64件固定の索引ページのみ読む。再起動後もheadを保持 | `dispatch_pages_follow_insertion_order_for_tied_timestamps`、`dispatch_migration_excludes_existing_inbox_rows_without_rewriting_them` |
| N57 | runtime通知event / 60秒fallback → Tauri `drain_pending/poll_once` → OS toast | 初回/account切替/restoreは現在headだけをbaselineに保存。1ページ後にaccount guardを解放、cursorはscalarで永続化。quiet/read/self/種類設定・成人向けpreview gateを通る通知のみOS送信 | `dispatch_cursor_storage_does_not_grow_with_same_timestamp_history`、`cursor_tracks_insertion_order_without_timestamp_tie_state` |

N56の保存sinkは通知rowと同一INSERT transaction内のsequence/索引だけで、本文や既存inboxを複製しない。
N57はlocal OS通知のみを送る。新しいnetwork I/O、private参照の外部送信、通知の履歴backfillは行わない。

## N28のCN rendezvous候補窓（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N59 | auth/consent済heartbeat → `TopicRendezvousStore::heartbeat` → Valkey topic窓/member/peer keyと候補JSON | 入力正規化後にValkey `TIME`で共通時刻。接続/応答は各2秒。opaque topic keyのみ。15秒×直近4窓、窓TTL60秒・topic-peer/peer TTL45秒。SADD+EXPIREは同じMULTI/EXEC。窓ごと16件のdistinct標本、最大64候補のmembership/peerを検証し8件だけ返す。leaveは直近4窓とmemberを除去 | `rendezvous_candidates_do_not_grow_with_topic_membership`、`another_topic_cannot_extend_an_expired_membership`、`rendezvous_bucket_and_membership_have_finite_ttls`、`bucket_insert_and_ttl_are_one_valkey_transaction`、`invalid_topic_is_rejected_before_valkey_io`、既存CN API auth/consent/privacy tests |

N59はtopic presenceのephemeral stateだけを増減する。auth/consentとendpoint bindingの確認は
`cn-user-api`の既存handlerを通し、rendezvous応答をaccountの証明にしない。旧`topic:` SETは
新経路から参照せず従来TTLで消える。候補の完全列挙は目標にしない。

## Account受信routeのtransport境界（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N60 | `HintTransport::{subscribe_receive_offers,publish_receive_offer}` → account別gossip topic → sealed offer受信stream | recipientからroute導出。送信前にofferの2,048byte上限を検証。受信は同時1accountだけ、旧account切替/明示解除/shutdown/Dropでtaskと旧streamを停止。送信後のtopic保持は30秒・最大32件。bootstrap候補は3source各4件だけを読む。taskは管理lock取得後に生成し同じ非await区間で登録。shutdownは先に受付を閉じ、join待ちを通知で止め、全holdをabortしてから待つ。account routeは通常topic診断へ混ぜない。Fakeも同じ世代終了を行う | `account_receive_offer_crosses_real_gossip_with_one_recipient_route`、`account_receive_route_replaces_the_previous_account_subscription`、`oversized_receive_offer_is_rejected_before_joining_a_route`、`account_route_bootstrap_window_is_independent_of_imported_history`、`cancelling_offer_subscribe_before_registration_leaves_no_receiver_task`、`cancelling_offer_publish_before_registration_leaves_no_hold_task`、`cancelled_offer_shutdown_aborts_every_detached_hold_before_waiting`、`offer_publish_waiting_for_registration_cannot_revive_after_shutdown`、`offer_subscribe_waiting_for_registration_cannot_revive_after_shutdown`、`offer_join_wait_stops_when_transport_shuts_down`、`fake_account_switch_stops_delivery_to_the_old_offer_stream` |

N60単独のsinkは暗号化offerの一時配送のみ。`source_peer`はgossip経路の観測であり署名senderやproviderの証明ではない。受信後のAEAD/署名、binding、scope・mutual・private epoch、blob取得、永続反映、再送所有、D2全受信範囲への接続はこの段階で未完了。既存hint経路は残す。

## 署名provider限定のoffer payload取得（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N61 | `BlobService::fetch_verified_receive_offer_payload` → node共通受付 → receive-binding ALPN → 同じendpointのblob ALPN → memory bytes | `VerifiedReceiveOffer`の署名済みprovider IDと候補ID一致、offer期限をI/O前に検査。受付待機後とblob要求前にもoffer/binding期限を確認。QUIC相手とsender accountのbindingを確認してからblobを要求。宣言byte数最大65,536をstream中に制限し、完了時に実長/BLAKE3/両期限を再確認。保存せず、接続は結果・取消で閉じる。共通8実行枠と受付時から30秒期限を使用 | `signed_provider_offer_fetches_only_its_bounded_manifest_without_storing_it`、`offer_fetch_rejects_wrong_endpoint_missing_binding_and_declared_length`、`retained_expired_offer_is_rejected_before_provider_io`、`bounded_offer_blob_work_shares_the_display_slot_and_stops_on_close` |

N61はprovider bindingを公開accountへ結ぶだけで、public source参加・DM mutual・private epoch/capabilityの許可を証明しない。アプリ側でこれらをI/O前と反映直前に確認し、失効時に要求futureを中止するまで受信経路として有効化しない。

## Gossip warmupのpeer窓（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N62 | topic join/retry → `TopicWarmupCoordinator::warmup_peers_once` → gossip ALPN dial | peer履歴100/1,000でも4候補だけを巡回してclone。`for_each_concurrent(2)`はtaskをspawnせず、共有dial slot2件が満杯ならin-flight台帳を増やさず即時延期。既存direct優先とrelay fallbackはwarmup先のaddress構築を維持 | `warmup_samples_a_moving_four_peer_window_from_large_history`、`warmup_does_not_queue_peer_state_when_shared_dial_slots_are_full`、`transport_import_ticket_updates_existing_topic_subscription`、`transport_seed_update_updates_existing_topic_subscription` |

N62はper-peer warmupの増幅だけを除く。`ensure_hint_topic`の初回bootstrap合成とtopic全体のretry task、`extend_active_topic_peers`の全topic更新、SDK内部のgossip viewはN04/U07のまま残る。D2の現在の受信対象を減らすために購読を切り捨てない。

## Gossip topic warmupのtask所有（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N63 | 初回join/peer追加 → receiverまたはtopic stateのwarmup task → gossip dial | 初回warmupはreceiver取消時にDropでabort。更新は同一topic世代で最大1taskを登録し、旧taskをabort/awaitしてから置換。解除/shutdownではclosed通知と全task abortを先に発行し、終了を待つ。join待ちの旧世代は通知で取消し、古いtimeout判定はsnapshot世代が現stateと一致するときだけ置換する。shutdown後のsubscribe登録を拒否 | `unsubscribing_during_initial_join_stops_its_warmup_task`、`unsubscribing_stops_a_registered_peer_update_warmup`、`hint_subscribe_waiting_for_registration_cannot_revive_after_shutdown`、`cancelled_hint_shutdown_aborts_all_topic_tasks_before_waiting`、`stale_rejoin_decision_cannot_remove_a_new_topic_generation`、既存ticket/seed更新・timed-out再購読 |

N63はwarmup taskの停止所有を対象とし、topic全体やpeer全体の走査量を削減したと主張しない。`ensure_hint_topic`のbootstrap全件materializeと `extend_active_topic_peers` の全topic更新は残る。D2の現在の受信対象を維持するため、active topic数を暗黙に切り捨てない。

## DM保護outboxのpeer別再送ページ（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N64 | DM送信/相手別2秒tick → `DirectMessageStore::list_direct_message_outbox_for_peer_page` → 既存署名frame hashのpairwise hint | SQLiteの(peer,created_at,message_id,dm_id)索引を使い64行＋続き1行だけ読む。Memoryも同順索引を更新。1巡の開始時にpeerの末尾keyを固定し、tickごとにその終点までcursorを進めて先頭へ戻る。途中の新規DMは巡回を延ばさず、保存した1rowを直接送信する。mutual、ACK、tombstoneと保護rowは従来どおり | `direct_message_outbox_peer_pages_ignore_other_peer_history`、`new_outbox_rows_cannot_starve_older_unacked_rows`、`direct_message_outbox_peer_page_uses_the_sqlite_cursor_index`、`dm_outbox_retry_reads_only_one_peer_page_per_tick`、既存DM delivery/restart・migration roundtrip/backend parity |

N64は再送tickと新規送信時のoutbox読取りだけを有限化する。`resume_direct_message_state`と`direct_message_status_view`はまだ全outboxを読み、DM peer/topicごとの常時taskと旧pairwise配送も残る。outboxは保護データであり、容量のために削除・成功扱いしない。新索引は端末内に留まり、外部送信は既存hintだけ。

## DM状態表示のpeer別上限窓（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N65 | DM状態/会話表示 → `direct_message_status_view` → `list_direct_message_outbox_for_peer_page` | peer別索引の先頭64行だけを読み、続きがある場合は`pending_outbox_has_more`を立てて画面で`64+`と表示。0〜64件は正確な値を維持。CLIの厳格出力schemaにもflagを登録。保護outboxを削除せず、送信・ACK判定へ計数を流用しない | `dm_status_uses_a_bounded_peer_outbox_window`、`dm_status_and_conversation_outputs_accept_the_bounded_count_flag`、既存DM状態・送信待ち/restart・IPC型/表示契約 |

N65は個別状態表示の全outbox走査だけを除く。会話一覧全件、起動時の全outbox/会話/相互peer走査、peerごとの常時taskは残る。N64の「未完了」記録は当時の状態として残し、この欄で差分を示す。

## Account受信routeのアプリDM入口（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N66 | runtime identity load → `start_account_receive_offers` → sealed offer復号/署名確認 → scope/DM mutual → provider限定manifest一時取得 → 既存DM frame取込 | accountごと1route、同時最大4future。未移行scopeはprovider I/O前に拒否し旧受信routeを維持。DM mutualをmanifest・frame・各添付fetch後とplaintext保存前に再確認。route解除はprocess一意leaseで旧世代を弾き、shutdown取消後もleaseを保持して再開する。旧streamのsupersede時は実行中futureを取消して停止。stack再構築時だけ新transportの空routeを条件付きで取得して復旧する。予期しないapp owner dropは処理taskをabortし、route自体はtransport寿命が所有する | `account_receive_offer_rejects_unmutual_and_other_scopes_before_provider_io`、`account_route_ingests_verified_mutual_dm_and_stops_on_shutdown`、`account_offer_rechecks_mutual_after_provider_io_before_reflection`、`revoked_mutual_during_attachment_fetch_never_persists_plaintext`、`old_account_owner_shutdown_cannot_stop_new_same_account_receiver`、`superseding_account_owner_cancels_old_in_flight_provider_fetch`、`cancelled_shutdown_retries_the_account_route_lease_cleanup`、`stale_same_account_lease_cannot_unsubscribe_a_new_receiver`、`app_account_listener_reclaims_vacant_route_after_stack_rebuild`、停止/Drop、実Iroh `real_account_route_fetches_bound_provider_manifest_and_reflects_dm`、既存DM delivery/restart、runtime binding |

N66のDM manifestは既存`GossipHint::DirectMessageFrame`のJSONをprovider限定65,536byte以内の一時参照として使い、復号済みsenderと導出DM topicが一致した場合だけ既存frame検証・通知へ渡す。新送信側のaccount→endpoint binding解決とoffer発行、ACKのaccount route化、公開通知/private epochのscope guardと反映は残件。旧pairwise受信・outboxは撤去せず、D2の受信範囲と保護データを維持する。

## 受信宛先の署名binding照合とDM輸送移行（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N67 | 送信側のaccount宛先要求 → `resolve_receive_destination` → configured/bootstrap/importedのaccount別cursor → 実QUICで署名binding照合 → 検証済み`EndpointAddr` | 1試行最大4候補・12選択step、同時2照合、候補2秒。cache最大1,024account・署名期限内/最長10秒。shutdown後は返却/次probeを止め、進行中接続を取消。失効は旧結果の世代を止め、別endpointの成功cacheは保持。未解決は`None` | `destination_window_rotates_through_large_peer_history_in_four_candidate_steps`、`destination_cursor_reaches_old_peer_during_new_inserts_and_deletes`、`destination_requires_live_binding_for_the_exact_account_and_invalidates_cache`、shutdown/cache/旧probeの負例 |
| N68 | peer別DM outboxの64行ページ → N67宛先照合1回 → frame hash/message IDをrecipient account sealed offerへinline記録。recipientは実QUICのprovider binding照合後に既存暗号frameを取得し、provider endpointへ署名ACKをinline offerで返す。sender account ACK受信 → outbox解除 | mutualと同一outbox rowを宛先照合後・offer前に再確認。新規manifest blob書込み0。offer/ACK offerは各2秒・account runtime同時4件で取消/延期し、待機列を作らない。未解決・失敗で保護rowは残す。ACKは署名sender/recipient/conversation/messageを確認し、重複時は最初の配達時刻を保持。旧pairwiseを維持し、追加の周期timer/taskは作らない | `dm_outbox_page_sends_sealed_account_offer_without_consuming_protected_row`、`revoked_mutual_after_destination_lookup_sends_no_account_dm_offer`、`inline_dm_frame_requires_sender_bound_provider_before_blob_io`、`signed_account_route_ack_clears_only_matching_dm_outbox`、`direct_message_acked_at_keeps_first_signed_ack`、実Iroh `real_account_route_fetches_bound_provider_manifest_and_reflects_dm` |

N67の既知peer窓はCN/著者制御stateからのaccount候補探索をまだ代替しない。N68時点のpeer別tickはN69で置換するが、旧pairwise購読を残すためNET-AC-2/6やD2の全達成ではない。旧経路撤去、全相手数に比例する購読task/起動処理の除去、公開通知/private epochの送受信は後続。

## DM保護outboxのaccount共通due owner（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N69 | identity復元/関係再構築 → account単一ownerの2秒tick → `list_due_direct_message_outbox` → 最大4行のpairwise hint/account offer | 未試行3件と期限到来済み再試行1件を別indexから読む。各行の試行時刻を更新し、mutual/row現在性を再確認してから送信。旧pairwise hint・account offerは各2秒で取消し、遅いpeerが他laneを無期限に塞がない。送信失敗/失効でも保護row維持。新ownerは重複起動せず、shutdown/dropで実行中futureを取消。pairwise受信streamからtimerを除去 | `due_dm_outbox_lanes_keep_new_and_old_work_bounded`、`due_direct_message_outbox_uses_both_sqlite_lane_indexes`、`all_generations_have_paired_down`、`dm_due_owner_processes_bounded_new_and_retry_lanes`、`blocked_pairwise_publish_cannot_stop_other_peer_or_account_offer`、`account_dm_retry_owner_is_single_and_shutdown_cancels_active_lookup`、既存DM restart/mutual tests |

N69は周期再送の総peer数依存だけを減らす。起動時の全conversation/outboxと全mutual graph読取り、旧pairwiseの相手別受信task、既知peer外の宛先発見は未移行。

## Account受信宛先のCN候補窓（P3）

| ID | 入口 → helper → sink | guard / 停止 | 対応contract |
| --- | --- | --- | --- |
| N70 | CN session期限/heartbeat/metadata refresh → 各CNの期限到来時にnode別cursorを進める → `list_direct_message_outbox_candidate_page`の作成順固定終端・1回最大4行とnode別cursor → own route＋当該CNの最大4送信先routeを別heartbeatで更新 → source別候補窓 → `resolve_receive_destination`で実QUIC署名binding照合 | CNの既存auth・同意を使い、応答後に設定・同意・sessionを再確認。HTTP前にtransport instance/clear epochのfenceを取り、候補登録lock内で比較して失効後の復活を拒否。sourceはaccount最大8件・各8候補・45秒、account最大1,024、1試行4候補/同時2probe。1node解除ではそのsourceだけと専有cacheを消す。request最大5route、response 65,536byte/5route/各8候補/4relay URL、addr_hintは数値IP:portのみ。保護outboxはACKまで維持 | `account_candidate_pages_advance_independently_of_retry_attempts`、`account_candidate_page_uses_stable_sqlite_cursor_index`、`revoking_source_clears_cache_even_after_its_candidate_list_changes`、`account_rendezvous_queries_one_due_recipient_without_public_topic_snapshot`、`untrusted_rendezvous_candidate_requires_live_account_binding`、既存CN session/DM tests |

N70は旧全購読topic snapshotから自account受信routeを除き、own routeの更新は各CNに維持する一方、送信先routeの照会は各CNの期限到来に合わせ、node別cursorで独立に進める。旧head監査で見つかった応答後の失効race、複数CN上書き、疎なdue peekの飢餓をそれぞれfence、source所有、独立cursorで塞ぐ。401/同意要求は既存session再認証・再同意へ返し、他の失敗は5秒以上待つ。CN不使用時の署名済み著者制御state個別取得、旧pairwise受信task、起動時全件走査、既存CN topic refresh自体の総購読数依存、明示leaveを伴う差分登録は残件。

## N71 著者制御endpoint locator（2026-09-24失効）

当時のN71は、account-onlyの宛先発見を著者replicaの署名付きlocatorで実装する計画だった。
#1221の[現行Scope revision](https://github.com/kukuri-app/kukuri/issues/1221)ではCNなしの未知endpoint発見を要求せず、
既存のticket/seed/既知peerとCN候補を実QUIC bindingで検証する。#1333の本番未使用locatorページAPIと
停止中の制御recordは不採用とし、旧計画を残件に数えない。前節の「著者制御state」は当時の残件記録である。

# #1176 無操作時のCN・ピア接続維持

## 対象と承認

- 基準commit: `361791d3e120f315f3f3b7fdebafe009f0dd6183`。Scope revision `1176-plan-v2`。リスクC。
- 2026-09-20: ユーザーが実装・Issue作業・commit・PR・CI成功後のmergeまで承認。独立監査は別工程とする。
- relay容量不足を[#1206](https://github.com/kukuri-app/kukuri/issues/1206)、blob反復取得・「取得に失敗しました」/更新ボタン・cache仕様を[#1207](https://github.com/kukuri-app/kukuri/issues/1207)として起票した。両Issueは#1176の完了条件ではない。
- 追加CI testは原因を特定してから作り、実時間sleep・長時間timeout待ちを追加しない。
- 固定AC/INVARと元の報告は[#1176](https://github.com/kukuri-app/kukuri/issues/1176)。ローカルの詳細分析・計画は `.codex/plans/2026-09-20-issue-1176-idle-connectivity.md`、恒久的な実装・証跡は本記録へ集約する。

## 根拠と原因

添付ログの原本SHA-256は `8665808476b2574cc26f00d522336535c8f8167befa7a0a2adf19247e5b7cd3b`。2000件中、remote fetch関連1393件、relay容量不足489件、docs actor送信失敗21件。先頭以前は破棄済みで、報告ビルド、格納時刻、sleepの有無は記録されていない。以下の再現は現行コードの具体的な不具合を証明するもので、原本の最初の障害原因やOS suspendを証明するものではない。

| 問題 | 変更前の制御フロー・再現 | 対策 |
| --- | --- | --- |
| CN応答待ちの上限なし | 共通HTTP clientのtimeout既定値がNone。header/bodyを途中で止め、仮想時間31秒を進めてもfutureがPending | connect 5秒・body込み10秒の上限 |
| 1nodeの待機が全nodeへ伝播 | `ensure_community_node_session_with_mode`の全node共通mutex。Aのlock保持中にBのlockをpollしてもPending | node別session lock。利用が終わったlockはWeak参照で回収 |
| tick全体の完了待ち | 全node→観測送信→self-healをawaitし、その後15秒sleep | node別session・観測・self-healを独立したfutureとしてpoll。各lane1件、停止で全futureを破棄 |
| 正常actorの不要な停止 | peer不在のself-healでも`force_rebuild_runtime_connectivity_assist`を無条件実行 | 正常なlocal actorは維持し、discovery/購読を再適用。local docs probe失敗時だけ再構築 |
| 再構築失敗後に復旧不能 | `SharedIrohStack::rebuild`が旧stackをtake・shutdown後、次の生成に失敗するとcurrent=None。2回目が`missing active iroh stack during rebuild` | 次stackが完成するまで旧参照とpeer stateを保持。再構築/停止を直列化 |
| actor復旧後に旧購読が残り得る | 入力memoが同一なら、actor世代が変わってもseed再適用を省略 | actorの健康を確認してmemoを利用。世代変更でseed memoを無効化し、購読とprivate capabilityを復元 |
| 自己修復がcanonical storeを作り直し得る | 再構築が起動時の破損store退避・再生成を共有していた。破損を注入した変更前testは再生成後のfile lockで失敗 | runtime再構築は`IrohDocsNode::reopen_with_discovery_config`を使い、既存docs/default-author/endpoint secretを要求する。open失敗でstoreを退避・初期化しない。起動時の既存回復契約は変更しない |

並行化に伴う回帰防止として、metadata応答は最新configの対象nodeだけへmergeし、HTTP前の古い全node snapshotを書き戻さない。削除済みnodeのmetadataを復活させない。relay/seedのglobal applyは専用mutex内で最新の検証済みnode集合から組み立てる。

## Surface inventoryと状態遷移

| ID | 実入口→helper→sink | guard / 検証するtransition |
| --- | --- | --- |
| INV-1 | `ClientHost::{from_runtime,replace_runtime_locked}` → `start_community_node_session_scheduler*` → `MaintenanceTasks`、`shutdown_checked` | runtime毎のtask slot、future所有、TR-1維持 / TR-7停止・切替 / TR-8再起動 |
| INV-2 | scheduler、config setter、手動refresh、index/trust/advisory → `ensure_community_node_session_with_mode` → policy/auth/consent/bootstrap/rendezvous HTTP・token | node別lock、既存preflight/401/retry/admission、TR-2待機 / TR-4認証 / TR-5混在同意 |
| INV-3 | scheduler / sharing enable → `flush_community_node_trust_observations_once` → HTTP/outbox | 独立lane、既存共有同意/撤回guard、TR-2/4/5/7 |
| INV-4 | metadata/apply_ready/self-heal/設定 → `apply_runtime_connectivity_assist_with_mode` / `apply_effective_seed_peers_with_mode` / force rebuild → SharedIrohStack/AppService | global apply排他、最新のnode別policy照合証拠、TR-2/3/4/5/7 |
| INV-5 | periodic hydrate・subscription・書込み → reloadable docs/replica restart、stack apply/rebuild → `IrohDocsNode::reopen_with_discovery_config` → docs actor/store/stream | local read-only probe、bounded probe timeout、旧参照保持、再open失敗で初期化しない、世代変更時の再購読、TR-3回復 / TR-6actor失敗 / TR-7/8 |
| INV-7 | 上記状態遷移 → `kukuri_connectivity` → 既定filter/既存buffer/export | token/鍵/DM本文なし、上限変更なし、TR-1～8 |

INV-6/TR-9（blob取得）は#1207へ移管。shared HTTP clientのcallerは `requests_support`、`manifest_support`、`dome_hosting_support`、`tester_feedback_support`、`indexing_request_support`、`indexing_status_support`、`content_advisory_lookup_support`、`index_query_support`、`trust_observation_support`、`trust_relation_support`。通報専用clientのredirect禁止契約は変更しない。

探索はCodeGraph node/callersを先に使い、`rg`でtrait forwarding、設定/手動refresh、scheduler起動停止、全HTTP client callerを補完した。公開IPC/route、wire/schema、永続保存形式の追加はない。

## AC / INVAR → test / evidence

| 条件 | 証跡 |
| --- | --- |
| AC-1 / INVAR-4 | 既存scheduler keepalive・token更新・rendezvous独立更新・getter read-only test、`blocked_lanes_do_not_stop_other_nodes_or_duplicate_work` |
| AC-2 / INVAR-1/4 | `stalled_cn_headers_have_a_deadline_without_wall_clock_wait`、`stalled_cn_body_has_a_deadline_without_wall_clock_wait`、`stalled_node_does_not_hold_another_nodes_session_lock`、multi-node metadata/同意test |
| AC-3 / INVAR-2/3 | `idle_peer_repair_preserves_healthy_docs_actor`、`failed_stack_rebuild_can_retry_without_losing_local_docs`、`idle_repair_of_closed_actor_restores_private_capability_with_unchanged_seeds`、`idle_actor_repair_does_not_replace_an_unreadable_canonical_store`、既存connectivity scenario |
| AC-5 / INVAR-5 | 限定targetのscheduler/session/stack世代ログ、`default_filter_keeps_connectivity_transitions_without_verbose_runtime_logs`、既存buffer上限・filter test |
| INVAR-1 / TR-4/5 | 既存community_nodeのsession/admission/metadata/trust観測test、`idle_maintenance_merges_node_metadata_and_retains_consent_boundaries`（未同意nodeへのHTTP 0、tokenなし） |
| INVAR-3/4 / TR-7 | maintenance future drop時のlock解放と重複lane抑止、既存shutdown/account/restore test |

## 検証記録

- 変更前: HTTP deadlineの2件がFAILED（実行0.01秒）。node lockの1件がFAILED（0.00秒）。actor維持・再構築再試行の2件がFAILED（0.19秒）。実時間sleepは追加していない。
- 中間確認: community_node関連166件成功（42.52秒）、追加のidle/multi-node 3件成功（0.18秒）。その後のactor世代/復元の追加差分は最終検証で確認する。
- 最終 `cargo xtask rust-check`: 成功（fmt、全対象clippy `-D warnings`）。
- 最終 `cargo xtask rust-test`: **1048件成功、既存4件skip**（実行106.850秒）、doctest成功。新規CI testに実時間sleepは追加していない。
- 追加idle系5件: 成功、0.33秒。actor故障・修復後のprivate capability・identity・canonical bytes保全を含む。
- 最終 `cargo xtask tauri-check`: 成功。
- Tauri `tracing::tests`: 9件成功、0.01秒。通常のtest exeはWindowsのCommon Controls v6 manifest不足で `STATUS_ENTRYPOINT_NOT_FOUND`（`TaskDialogIndirect`）となったため、Windows SDK `mt.exe`で**ローカル生成test exeだけ**へv6 dependency manifestを埋め込み、同じテストを実行した。製品ソースやOS設定の回避変更はしていない。tracingの対象ソースはその後不変。
- 最終 `cargo xtask e2e-smoke`: `desktop_smoke_post_persist` 6step成功。FakeNetworkの永続確認であり実接続の代用ではない。
- 最終connectivity scenario: `community_node_public_connectivity` 15step（28.2秒）、`community_node_multi_device_connectivity` 11step（9.1秒）、`private_channel_invite_connectivity` 11step（11.7秒）、すべて成功。前2本はローカルPostgres/Valkeyを起動して実行し、終了時にxtaskが後片付けした。
- 通信経路: 既存static-peer testとdirect優先の候補順test、topic rendezvous testを維持。`relay_only_seed_peers`等の既存testはRelay Fallbackの検証として区別し、通常P2Pの代用としていない。
- 最終 `cargo xtask oversized-files` / `git diff --check`: 成功。baseline増量なし。
- 補足: Tauri全体の `cargo fmt --manifest-path ... --check` は未変更ファイルの既存format差で失敗する。今回変更した `tracing.rs` は単独rustfmt check成功。無関係な整形は混ぜていない。途中の並行xtask再buildはWindowsの実行中exe lockで失敗したため、最終検証は同一featureの逐次実行で完走した。
- 独立監査は実装者とは別コンテキストで行い、正式判定を対象PR head付きのPRコメントへ残す。コード差分の監査とIssue全体の実機完了判定は区別する。

## 未確認の範囲

原本ログの報告版・最初の障害原因は特定できていない。Windowsで実runtime/実peerを使った上記自動検証は完了したが、GUIを長時間トレイへ格納する観測、画面ロック、OS suspend/resume、Linux実機観測は未実施。これらを成功扱いせず、**今回の修正PRをマージしてもIssue #1176はCloseしない**。残りは既知のpeerを使った非表示・復帰時のCN期限とpost/reply/blob到達の確認、および報告ログとの照合に限定する。

## Linux CIで見つかった終了処理の不足と追加修正

初回head `5cf10b8e` のPR #1208では12チェックが成功したが、Linux Rustの1072件中1071件完了後、`failed_stack_rebuild_can_retry_without_losing_local_docs`だけが1380秒以上停止した。認証済みブラウザで実行中ログを確認し、run `35462151565`を中断した。旧headの独立監査PASSは当時の判定として保存し、この新しい証拠を含むマージ判断には使わない。

WSL Ubuntu 22.04 / Rust 1.92で同じtestを30秒の外側watchdog付きで再現した。一時的な段階ログにより、最初のinvalid relayによるrebuildはErrを返し、2回目も旧stackのshutdownは戻るが、次stackのopenが戻らないことを確認した。段階ログは診断後に除去した。

`load_persistent`はFsStoreを開いた後でnodeを組み立てる。relay parse/bind等の早期Errでは、nodeがまだ生成されていないためnode自身のDrop cleanupを使えず、FsStoreの非同期終了完了前に次のopenへ進めていた。Err時に`store.shutdown().await`を完了させてから元のstartup errorを返すように修正した。既にdocs失敗側がshutdownした場合のclosed errorは元エラーを上書きしない。依存ライブラリ内部のどのlockで停止したかまで実証したとは扱わず、この失敗cleanupと再openの境界を修正対象とする。

- 同一Linux再現: 修正後0.13秒で次stack open、保存docs読込み、shutdownまで成功。
- Windows delta: `cargo xtask rust-check`成功、iroh-node全件＋idle recovery群の15件成功（0.518秒）。
- WSL Linux: `cargo xtask rust-check`成功、`cargo xtask rust-test`で1072件成功・既存4件skip（169.550秒）、doctest成功。停止していたtestは全体実行でも0.173秒で成功した。
- `.config/nextest.toml`で、このidle recovery群だけにprocess-levelの`30s × 2`上限を設定した。既存4並列groupを維持し、通常成功にsleepを加えず、非協調的な停止をCI job全体の上限まで待たせない。
- Linux全体の最終結果とdeltaの独立監査は、追加commitのheadに対応するPRコメントへ記録する。CIの再実行を調査手段にはせず、ローカルLinuxで原因・修正を確認してからpushする。

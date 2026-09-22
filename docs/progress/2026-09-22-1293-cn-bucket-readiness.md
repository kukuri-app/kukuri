# #1293: CNのbucket追従

## 現在状態

- 親: #1243、対象: #1293、Scope revision `2026-09-22-v1`、区分C。
- 基準: `dba0881da72f8bca974f3e9d76c4d13c434894e0`（基盤PR #1297のmerge）。
- In progress。publicの読取り計画とscopeの寿命を準備中。private epoch、取り下げのbucket越え、移行の完成は未達。
- runtimeの既定はLegacyのまま。`PublicReplicaReadMode`は先行検証のためのbuilderで、運用環境からの切替入口はまだ追加していない。

## 今回の差分

- 論理scope(kind/id)と物理replicaを分け、対象namespaceが変わっても同じscopeの索引を消さない。
- publicの旧形式/移行中/現在と直前のbucketという読取り計画を追加。privateをpublicの設定で変更しない。
- 1 passの時刻を固定してdesired/openで共有する。窓から外れたreplicaは同期だけ止め、停止失敗時は次のpassで再試行する。
- supported setから外したpublic replicaもcloseする。保存entryは削除しない。
- ingestのscope/replica照合はopenより前。新bucketの投稿は署名済みenvelopeからheaderを復元し、署名済み時刻・bucket・未来許容幅を照合してから本文/scanへ進む。
- 新bucketのjob revision/scan fingerprintは署名ID。未署名markerやJSON表記の変更でallow判定を失効させない。現物envelopeは署名と配置を再確認し、docs authorがあれば1件指定で読む。
- 処理台帳は実行中を含め1,024件。完了/取消/再試行待ちの最古を回収し、全枠実行中なら新規を延期する。
  leaseを実行futureが所有し、cancel時は同世代の実行中記録だけをCancelledへ移す。完了と新世代は変更しない。
- 変更key/1bucketの欠落をlogical scope全体の欠落としてjobをcancelする処理を削除。台帳の保持上限で回収する。
- CN E2Eのquery障害wrapperがcloseとauthor指定読取りを転送する。停止にquery障害を混ぜない。

## この準備差分の固定inventory

親子Issueの全ACの完了監査ではなく、runtimeをLegacyに保った先行差分を対象とする。

| ID | 入口・member | helper / sensitive sink | guard・維持条件 | 遷移・証跡 |
| --- | --- | --- | --- | --- |
| CR-1 | desired_scopes(_at)、restore_scopes(_at)、ingest_all_supported、worker full_pass | PublicReplicaReadMode::scopes → namespace open/secret登録 | public新旧選択、privateは従来、pass内時刻固定 | 旧/移行/新形式、10倍履歴: replica_plan、bucket_scale |
| CR-2 | worker full_pass、stop_and_deindex_scope、revoke_channel_and_deindex | stop_replica → close/revoke、entries/projection削除 | physical終了でlogical索引を消さない、停止失敗を再試行 | 旧→bucket、mixed scope、空bucket停止失敗: lifecycle/bucket_scale |
| CR-3 | ingest_scope、ingest_changed_keys（worker/fullpass/event/手動） | validate_scope_replica → open/query | logical scope一致がopenを支配 | mismatchでopen0、legacy回帰 |
| CR-4 | scope_context、ingest_scheduled_record、ingest_object_record | canonical_post → body/blob、safety provider、索引upsert/remove | v1署名/配置を確認、invalidコピーは他source削除の根拠にしない | wrong bucket、偽deleted、破損marker、provider不可とreuse |
| CR-5 | ReferenceGuard::verify/check → scanとupsert直前 | docs読取り、supported/legal/withdrawal確認 | legacy検証を維持、v1現物署名とscope、途中変化はtransient | 署名IDによるreuse、既存transient/withdrawal/reference contracts |
| CR-6 | enqueue/start/finish、lease Drop、snapshot | job台帳/permit | 1,024件、実行中はcapacity退避しない、cancel/旧世代安全 | scheduler容量、待機中/取得中cancel、stale completion |
| CR-7 | cn-e2e FaultInjectingDocsSyncのDocsSync実装 | 実Iroh close、author指定query | query障害以外を透過転送、LocalOnlyを保持 | 実Iroh query_fault_wrapper_forwards_replica_close |

逆引き: `open_replica`はparticipant restoreとingest両入口、close/revokeはstop_replicaへ集約。
ingestのprovider/body/blobとupsert/removeはCR-4/5の共通経路。schedulerのproduction enqueueはingest_scheduled_recordのみ。
`reconcile_scope`は唯一callerとともに削除。公開API/routeや環境設定で新形式writer/read modeを有効化する入口は追加しない。

## 修正前後の証跡

| test | 変更前 | 変更後 |
| --- | --- | --- |
| public_scope_removal_stops_replica_events_and_preserves_local_entries | scope除外後もsubscriptionが閉じずtimeout | 成功、保存値も維持 |
| moving_to_bucket_replicas_does_not_deindex_the_still_supported_logical_scope | 旧replica idがdesiredに無いだけで旧投稿の索引を削除 | 実Postgresで成功。旧投稿を維持し新bucketの投稿も索引 |
| mismatched_scope_is_rejected_before_opening_any_replica | 不一致namespaceを3回openしようとする | open 0回で拒否 |
| a_post_signed_for_another_bucket_is_not_indexed | 別bucketの投稿を1件index | index 0件 |
| a_misplaced_copy_cannot_remove_an_already_indexed_post | 別bucketのコピーが元の正しい索引を削除 | 元の索引を保持、偽deletedも同様 |
| changing_only_the_bucket_marker_reuses_the_signed_post_scan | marker変更でfresh scan 1回（期待0） | fresh0/reused1。providerは2回目からUnavailableとなる構成でcalls1・索引維持 |
| completed_job_history_does_not_grow_with_rotated_buckets | 完了履歴2048件を保持 | 1024件以内 |
| cancelled_waiting_and_fetching_jobs_release_their_records | future cancel後Cancelled0件（期待1） | permit待ち/取得中ともCancelled1、後続開始可 |
| query_fault_wrapper_forwards_replica_close | trait既定のunsupported error | 実Irohのclose成功 |

- `cargo test -p kukuri-cn-indexer --lib replica_plan -- --nocapture`: 2成功。
- `cargo test -p kukuri-cn-indexer --lib -- --nocapture`: 56成功。capacity/cancel/旧世代のcontractを含む。
- `KUKURI_CN_RUN_INTEGRATION_TESTS=1` と実Postgresを用意したcontract実行: 8件すべて実行して成功。
  過去bucket投稿10→100でも新形式のopen回数4・読取り回数/entry数が同じで、現在/直前以外へアクセスしない。
  その後、空bucketのclose失敗を再試行し他scopeを止めない第9contractを追加し、全体検証へ含めた。
  テスト専用compose project `kukuri-1243-cn-test`、Postgres port15443 / Valkey port16390を使用。
  同名container/volumeが無いことを確認して作成し、終了時にそのprojectだけを削除した。
- 未実装のprivate epoch/取り下げ/移行をこの結果で完了扱いにしない。PR headの独立監査は未完了。
- 最初の `cargo xtask cn-check` は成功。その後、new bucketの署名によるheader復元と候補選別を追加したため、更新差分を再検証する。
- 最初の `cargo xtask cn-test` は、レビュー指摘に対するred testと修正を先に確定するためcompile途中で中断した。
  成功証跡には使わない。専用compose project `kukuri-1243-cn-suite` のcontainer/volumeを削除して再実行した。
- 再実行した `cargo xtask cn-test`: 成功（報告上732 passed、0 failed）。Postgres/Valkeyのintegration gate有効。
  lifecycle 9件すべてとschedulerの容量/cancel、実Iroh wrapperを含む。任意のCN E2E gateは無効であり、
  ArcadeDBを含むCN E2E全体の達成証跡ではない。専用projectは終了時に自動削除済み。
- cn-checkで指摘された`bool::then`のclosureを`then_some`へ変更（値の意味は同じ）。Windowsで実行中の
  xtask.exeの置換が拒否された再実行は検証成功に数えず、cn-test終了後に改めて実行する。
- 最終差分の `cargo xtask cn-check` は成功。test helperのMutex失敗には`expect`で理由を付けた。
  その後の実Postgresによるlifecycle 9件もすべて成功。`cargo xtask oversized-files`と`git diff --check`も成功。
  上限緩和・baseline増加は行っていない。
- 独立予備reviewの署名ID・wrapper所見を修正。容量導入後のcancel残留所見もred/greenで修正した。
  PR headの正式監査は別工程として行う。

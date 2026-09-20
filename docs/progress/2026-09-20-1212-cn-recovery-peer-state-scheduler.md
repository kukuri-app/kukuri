# Issue #1212: CN復旧時のpeer状態管理と投稿取得scheduler

## Current status

- 判定: Audit pending
- Scope revision: `2026-09-20-peer-state-scheduler-v1`
- 基準commit: `b302f942decd25ae834ab9c295edee516fa96e02`
- 実装commit: `97f9d589`
- リスク区分: C
- Blocker: 0件

## 目的と再現

本番復旧時の初回巡回3633.280秒のうち、接続・転送timeoutが3505秒（96.47%）を占めた。
peer/candidateと投稿を直列処理し、成功実績を次の取得へ反映しなかったため、健全な投稿も待たされた。
再現と集計は [取り込み遅延調査](2026-09-20-cn-recovery-ingest-latency-investigation.md) を正本とする。

## 実装

### peer状態管理

- `kukuri-transport::PeerAddrBook`へ接続世代・状態、成功/失敗件数、連続失敗、平滑化転送時間、
  peer backoff、型付きrequest frequency ledgerを追加。
- 接続状態は5分、成功優先は10分で失効。古い世代の接続通知は無視する。
- remote fetchは状態からpeerを順位付けし、結果を台帳へ戻す。peer/candidate個別timeoutに加えて
  取得全体を30秒に制限する。
- HTTP IP / P2P endpoint / relay clientを別型とし、HTTPのtower-governorとrelay upstream byte limiterを
  enforcement adapterとして維持する。P2P fetchはendpoint単位16要求/秒のledgerを使用する。

### 投稿取得scheduler

- `PostFetchScheduler`を追加し、`scope kind + scope id + object id + source revision`ごとのleaseと
  queued / fetching / processing / retry_wait / completed / suppressed / cancelledを保持。
- 同じrevisionのactive jobを重複実行せず、新revisionが旧leaseを失効させる。`ReferenceGuard`が
  scheduler leaseも再確認し、stale jobは索引mutationへ進まない。
- scope内の投稿を既定4件の上限付きworker setで処理し、完了した枠へ次の投稿を補充する。
  `COMMUNITY_NODE_INDEXER_MAX_CONCURRENT_POSTS`で正整数を指定できる。
- 再起動時はraw bytesや実行中leaseを復元せず、authoritative replicaの全件照合からjobを再構築する。
- `/v1/status`へ識別子を含まない`post_scheduler`集計を追加。旧status JSONはdefault値で読める。

## AC / INVAR evidence

| 条件 | 実装・contract |
| --- | --- |
| AC-1 接続状態と世代 | `PeerConnectionStatus` / `record_connection_state`; `stale_disconnect_does_not_replace_newer_connection_generation`; `stale_connection_observation_expires_to_unknown` |
| AC-2 取得実績と順位 | `record_fetch_success/failure` / `ranked_peers`; `successful_peer_is_ranked_before_recently_timed_out_peer` |
| AC-3 request frequency | `RequestRateSubject/Class/Policy/Ledger`; `request_frequency_keeps_http_peer_and_relay_subjects_separate`; HTTP/relay adapter既存test |
| AC-4 投稿状態 | `PostFetchScheduler` / `/v1/status.post_scheduler`; scheduler/state/status test |
| AC-5 並列・重複・stale | `FuturesUnordered` worker set、lease再確認; scheduler tests、`independent_posts_are_ingested_concurrently_within_the_configured_bound` |
| AC-6 再起動reconcile | `ingest_scope`がreplica stateからenqueue/reconcile; `worker_restart_restores_supported_scopes`、`worker_restart_projection_rebuild_and_provider_recovery` |
| AC-7 遅い取得の分離 | peer順位contract、30秒total budget、投稿並列contract。実production時間の再計測はrollout後 |
| AC-8 観測 | `IndexerStateSnapshot.post_scheduler`、legacy JSON互換test |
| INVAR-1/2 fail-closed/guard | scheduler leaseを`ReferenceGuard`へ追加。cn-indexer transient/withdrawal/prevention/source contracts、cn-e2e failure paths |
| INVAR-3 通信経路 | candidate生成順は変更せずpeer順だけを実績で更新。`community_node_public_connectivity` |
| INVAR-4 上限・lease | scheduler semaphore、30秒budget、request ledger。permitはRAII、stale lease test |
| INVAR-5 非残留 | BlobService ephemeral境界を維持。runtime integrationのlocal miss確認を維持 |
| INVAR-6 既存制限 | user-api rate-limit tests、relay config tests。trusted proxy/upstream limiterは置換しない |

## validation

- `cargo test -p kukuri-transport --lib`: PASS（56 tests）
- `cargo test -p kukuri-iroh-node`: PASS（11 tests。30秒total budgetのtime-paused contractを含む）
- `cargo test -p kukuri-cn-indexer`: PASS（unit 51 + integration/contract、最終targeted retryを含む）
- `cargo xtask rust-test`: PASS（1052 tests、4 skipped + doctest）
- `cargo xtask cn-check`: PASS（最終差分）
- `cargo xtask cn-test`: PASS（最終差分、Postgres/Valkey integrationを含む）
- `cargo xtask scenario community_node_public_connectivity`: PASS（15 steps）
- `cargo xtask oversized-files`: PASS（既存warningのみ、追加baseline超過なし）
- Windows linkerが一度だけ`LNK1104: msvcrt.lib`を返したが、競合のない同一package再実行はPASS。

最終差分後にpath別validationを再実行し、PR headの独立監査結果とCIを追記する。

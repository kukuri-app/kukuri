# #1281 stack rebuild の shutdown 印を取消に強くする（2026-09-22）

- 対象 Issue: #1281（区分 C、Scope revision `2026-09-22-v1`、基準 commit `b39d3b606d835aaae9580b13b85d3742e78cb2b6`）。#1239 / PR #1279 の独立監査で残した non-blocker。
- Goal: rebuild が失敗・取消されても停止中または停止済みの stack を probe せず再構築へ進み、正常な rebuild 後は probe を再開する。
- Non-goals: rebuild 全体の transaction 化、retry policy・通信優先順位・保存形式の変更、現在存在しない timeout / select caller の追加。

## 固定条件

| ID | 条件 |
| --- | --- |
| AC-1 | 古い stack の shutdown 開始前に印を立て、shutdown 境界で rebuild future が drop されても印が残る |
| AC-2 | `current == None` なら、印の値にかかわらず `Err("missing active iroh stack")` を返す |
| AC-3 | rebuild 失敗後、docs actor を probe せず `local_docs_available()` が即座に `Ok(false)` を返す |
| AC-4 | rebuild 成功後は印が下り、replacement の docs actor を probe する |
| INVAR-1 | rebuild 失敗後の再試行で復旧でき、保存済み docs を失わない |
| INVAR-2 | 健全な stack を不要に作り直さず、通常の probe timeout はエラーのまま維持する |
| INVAR-3 | 全件走査、再同期、新しい外部通信を追加しない |

## inventory と状態遷移

| ID | 入口 | helper / sink | guard / invariant | transition / evidence |
| --- | --- | --- | --- | --- |
| INV-1 | runtime connectivity apply | `apply_runtime_connectivity` → probe / rebuild | 停止済みなら probe しない | TR-2 / failed-rebuild test |
| INV-2 | unchanged connectivity apply | `apply_runtime_connectivity_assist_with_mode` → early return / apply | 停止済みを正常扱いしない | TR-2、TR-3 / retry test |
| INV-3 | idle repair | `repair_community_node_connectivity` → apply / forced rebuild | 健全・停止済みを区別する | TR-1、TR-2 / existing tests |
| INV-4 | rebuild / forced rebuild | `rebuild` → shutdown / reopen / replace | shutdown 前に印、commit 後に解除 | TR-2、TR-3 / cancellation・success tests |
| INV-5 | runtime shutdown | `shutdown_checked` → `current.take()` / shutdown | current 不在を先に返す | TR-4 / missing-stack test |

| ID | 事前状態 | event | 期待結果 | 禁止する副作用 | test |
| --- | --- | --- | --- | --- | --- |
| TR-1 | active / healthy | health probe | `Ok(true)` | 不要な rebuild | `idle_peer_repair_preserves_healthy_docs_actor` |
| TR-2 | active | rebuild failure または shutdown 境界で cancellation | 印あり、`Ok(false)` | stopped actor probe | `cancelled_stack_rebuild_marks_the_current_stack_unavailable_before_shutdown`、`failed_stack_rebuild_can_retry_without_losing_local_docs` |
| TR-3 | marked stopped | rebuild retry succeeds | 印なし、probe succeeds | old stack の再利用 | `failed_stack_rebuild_can_retry_without_losing_local_docs` |
| TR-4 | current missing | `local_docs_available` | missing-stack error | actor probe | `shutdown_after_a_failed_rebuild_reports_the_missing_active_stack` |

caller の列挙は `rg -n "local_docs_available|\.rebuild\(" crates/desktop-runtime/src` で確認した。`local_docs_available` の caller は `apply_runtime_connectivity`、`repair_community_node_connectivity`、`apply_runtime_connectivity_assist_with_mode`。`rebuild` の製品 caller は `apply_runtime_connectivity` と `force_rebuild_runtime_connectivity`。sensitive sink は古い stack の shutdown、新しい node の reopen、reloadable service の差し替えであり、追加・削除はない。

## 修正前の再現

shutdown 直前に同期点を置き、rebuild future をそこで drop する `cancelled_stack_rebuild_marks_the_current_stack_unavailable_before_shutdown` を追加した。修正前の順序へ戻す mutation では、健全な旧 stack の probe が `Ok(true)` を返して `cancelling rebuild at the shutdown boundary must leave the unavailable marker set` で失敗した。時刻や actor の終了タイミングに依存せず、印が shutdown の後にある順序を再現する。

## 実装

- `SharedIrohStack::rebuild` は `current_shut_down` を、古い stack の shutdown とその最初の await より前に立てる。replacement の構築・peer state・docs author の復元・service の差し替えがすべて終わった後にだけ下ろす。
- `local_docs_available` は `current.as_ref()` を印より先に評価する。明示的な shutdown 後は、失敗した rebuild の印が残っていても missing-stack error を返す。
- test 専用の oneshot gate は shutdown 直前の cancellation point を決定的に作る。本番 build の型・経路には含まれない。
- 入口、sink、inventory の件数と分類は変更なし。処理は AtomicBool の定数時間判定のままで、全件処理や外部通信を加えていない。

## AC / INVAR の証跡

| 条件 | test / evidence |
| --- | --- |
| AC-1、AC-3 | `cancelled_stack_rebuild_marks_the_current_stack_unavailable_before_shutdown`、`failed_stack_rebuild_can_retry_without_losing_local_docs` |
| AC-2 | `shutdown_after_a_failed_rebuild_reports_the_missing_active_stack` |
| AC-4、INVAR-1 | `failed_stack_rebuild_can_retry_without_losing_local_docs`（retry 後の probe と保存済み docs） |
| INVAR-2 | `idle_peer_repair_preserves_healthy_docs_actor`、既存の probe timeout 契約を変更していない差分 |
| INVAR-3 | `rebuild` / `local_docs_available` の差分と caller / sink inventory |

## Validation

- 修正前 mutation: cancellation test は旧 stack を利用可能と判定して assertion で失敗。
- 修正後の targeted test: cancellation、failed rebuild retry、failed rebuild 後の shutdown の 3 件が成功。
- `cargo xtask rust-test`: 1,324 tests passed、5 skipped。doctest 成功。
- `cargo xtask scenario community_node_public_connectivity`: `status=pass`、15 steps、`connected=true`、`peer_count=1`。
- `cargo xtask e2e-smoke`: `desktop_smoke_post_persist` 成功（6 steps）。
- `cargo xtask oversized-files`: 成功。変更後の `stack.rs` は 986 行で新規 oversized file はない。
- `git diff --check`: 成功。
- PR head の独立監査と CI は後続の結果を追記する。

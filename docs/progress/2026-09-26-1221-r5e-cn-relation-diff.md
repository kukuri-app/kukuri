# #1221 R5-E: CN の手動取込と関係解析を差分・cursor へ移す

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R5-E と、2026-09-26 のユーザー決定「R5-E 関係解析の定義」。
区分 C、Scope revision `2026-09-24-outcome-v5-r2a-retention-1`、基準は #1369 merge `5f23e3df`。依存の R5-D は完了済み。

- 手動専用の全件取込（`ingest_all_supported`・`restore_scopes`・`desired_scopes`・`IngestPipeline::ingest_scope`、撤回の
  prefix 全件読み）を撤去する。topic の追加と索引申請の承認は、R5-D の受付（`last_index_demand_at`）へ需要として登録し、
  bucket reader の優先枠が現在の窓だけを読む。
- 関係の観測を投稿の共起から「2 者間のアクション」へ置き換える。アクションは返信・repost・引用・リアクション・フォロー。
  取込みで観測したアクションを行として保存し、行の追加・削除を trigger でペアの計数へ差分反映する。
- ペアの edge は双方向（方向ごとに種類は異なってよい）に成立したときだけ作る。値は既存の key のまま、
  `shared_topics`＝その 2 者間のアクションがあった public topic の数、`co_participation_events`＝2 者間のアクションの件数。
  成立しなくなったペアの edge と、public 参加が 0 になった author の cluster は削除する。全体の上位 limit 件の選択と
  dominant topic の先頭 1 万人の制限は撤去する。
- フォローは、相手側の方向のアクションを観測したときだけ、その author の replica の `graph/follows/<相手>` を 1 key 読む。
- 解析は変化したペアと author の印だけを古い順に上限つきで処理し、途中で止まっても印から続ける。
- 解析の実行記録は直近の上限件数だけを残す。

対象外: author replica の購読・同期、private channel の関係、proximity の計算式と重み、`neighbors` の読み方。

## 受入条件

| ID | 対象・期待結果 | 判定 |
| --- | --- | --- |
| AC-1 | 手動の全件取込の経路が無い。topic の追加と申請の承認で需要が登録され、bucket reader の優先枠に入る | 撤去した関数の呼出し 0、追加・承認後の `last_index_demand_at` |
| AC-2 | 返信・repost・引用・リアクション・フォローを 2 者間のアクションとして保存し、元の投稿の索引が消えると行も消える。自分自身へのアクションと private は保存しない | 各アクションの行、撤回・送信防止・scope 解除で行 0 |
| AC-3 | 双方向に成立したペアだけ edge を作り、値は topic 数と件数。片方向・不成立になったペアの edge と、参加 0 の author の cluster は削除する | oracle と一致、片方向で edge 0、撤回後に edge 削除 |
| AC-4 | 解析の 1 step は変化した印だけを上限つきで読み、履歴 10 倍でも読む行数が同じ。途中で止めても印から続け、変化の無いペアを書かない | 1 倍と 10 倍で同じ読取り、再開後の一致、変化 1 件で書込みがそのペアと author だけ |
| AC-5 | フォローは相手側のアクションを観測したときだけ 1 key 読み、有効なら行を置き、取り消しなら消す | 点読の回数、follow だけでは読まない |

維持する条件: INVAR-1 private channel 由来のアクションを関係に入れない。INVAR-2 relation graph は node-local の派生であり、
social graph と index 真実源へ書かない。INVAR-3 索引の fail-closed（allow の投稿だけ）を変えない。INVAR-4 proximity の
計算式・重み・API の key を変えない。

## 入口と副作用

| ID | 対応 | 入口 → helper | 期待結果 / 禁止する副作用 |
| --- | --- | --- | --- |
| T1 | AC-1 | `add_supported_topic` / `approve_indexing_request` → `supported_topics.last_index_demand_at` | 追加・承認で需要を登録。全 scope・全投稿の取込みは無い |
| T2 | AC-2 | 取込み `ingest_object_record`（Indexed、public）→ `observe_post_actions` → `record_relation_action` | 返信先は index の点読、repost・引用は元の著者。自分と private は保存しない |
| T3 | AC-2 | `ingest_object_ids` → `observe_reactions`（窓: 1 投稿あたり reaction の key 32 件まで、未保存の id だけ）/ `ingest_changed_keys` の reaction key → 同（読み直し） | 署名・reaction id・topic を確かめる。取り消しは行を消す |
| T4 | AC-5 | 新しく保存したアクション → `observe_follow` | remote の reader からだけ `graph/follows/<actor>` と署名つき envelope を読む。既存のアクションでは読まない |
| T5 | AC-2 | index の行の削除（撤回・送信防止・scope 解除）→ trigger | 起点のアクションを消す |
| T6 | AC-3/4 | `cn-cli relation analyze` → `analyze_relations` → `dirty_relation_pairs` / `dirty_relation_authors` | 印の付いた行だけを古い順に上限つきで読み、反映した印だけを外す |

## 実装の照合（監査前）

- 撤去: `IndexerParticipant::{desired_scopes, desired_scopes_at, restore_scopes, restore_scopes_at, ingest_scope,
  ingest_all_supported}`、`IngestPipeline::ingest_scope`、撤回の prefix 全件読み、`cn_core::co_participation`
  （Pg・memory の全体集計）と memory の解析 test。
- 追加: migration `202609260001_relation_actions`（アクション・ペアの計数・共有 topic・author の参加数・印・trigger・
  参加数の初期投入）、`cn_core::relation_actions`、`cn-indexer` の `ingest/relation.rs`、`RelationStore::{remove_edge,
  clear_cluster}`（memory・ArcadeDB と共有 contract）。ArcadeDB は新しい型へ移し、旧型を `ensure_schema` で削除。
- 解析の実行記録は直近 100 件と最新の成功だけを残す（`record_relation_analyze_run`）。
- 運用者向けの説明文（capability・docs・policy descriptor）を 2 者間のアクションへ更新。ADR 0026 §9 に新しい判断と
  旧案の失効を記録。

## 検証（局所）

- 変更前に失敗する条件: 旧実装は全体集計の上位 limit 件を書き、edge・cluster を消さない。片方向（投稿の共起だけ）でも edge を
  作る。追加・承認は需要を登録しない。
- Postgres（`KUKURI_CN_RUN_INTEGRATION_TESTS=1`）: `relation_contracts`（双方向でだけ edge、値は topic 数と件数、フォローで
  片方向が成立、索引の削除とフォローの削除で edge 削除、参加 0 で cluster 解除、private だけの author に cluster 無し。
  乱数 300 操作を上限 2 の解析で少しずつ反映しても全行からの期待値と一致。20 ペアと 200 ペアの履歴で、1 件のフォロー
  追加の印の数・反映結果が同じ）、`relation_ingest_contracts`（実 pipeline で返信・引用・リアクション・フォローを保存、
  自分への返信と private の返信は保存しない、author replica の読取りは新しいアクション 3 件で 4 回・再取込みで増えない、
  リアクションの取り消し・返信の索引削除・フォローの取り消しで行が消える）。判定を片方向で edge を作る形に変えると 2 件が
  失敗することを確認。
- ArcadeDB（`KUKURI_CN_RUN_ARCADEDB_TESTS=1`、使い捨てコンテナ）: 旧型に 1 件の edge を置いた状態から `ensure_schema` で
  旧型が消え、共有 contract（edge 削除・cluster 解除を含む）が成功。2 回目の `ensure_schema` も成功。
- 追加・承認の需要登録: `index_scope`（追加・承認の後に `last_index_demand_at` がある）。公平な巡回と「読取りが需要を作らない」を
  確かめる既存 test（`worker_contracts`・`scope_admission`・cn-user-api `index_query`）は、追加による需要を外してから確かめる。
- cn-core・cn-indexer・cn-user-api・cn-cli・cn-trust・cn-operator の test を Postgres つきで実行し、Valkey を要する
  rendezvous の 5 件（ローカルに Valkey 無し、変更と無関係）以外が成功。全体の手動取込の撤去に合わせ、test の投稿作成に
  timeline 索引の key を足した。`cargo clippy --all-targets -D warnings`・`cargo fmt --check`・`oversized-files`（増加なし）。
  cn-e2e（実 ArcadeDB を含む stack）は PR の CI。

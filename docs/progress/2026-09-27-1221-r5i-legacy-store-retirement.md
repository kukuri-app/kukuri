# #1221 R5-I: 閉じた旧保存領域の回収(旧 iroh store の退役)

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R5-I と、2026-09-27 のユーザー決定「R5-I 旧保存領域の回収方式」、
2026-09-26 の「R5-F CNの受入下限と保持」で R5-I へ移した CN の旧 store。基準は #1375 merge `0d17c370`。依存の R5-H は完了済み。

- client の node を新しい store(`<db>.iroh-store`)で動かし、旧 `iroh-data` は読むだけの別の instance として開く。
- 本人の書込みを書いたときに保護所有先へ入れる。60 秒ごとに旧領域を写す保護移行の常駐は、旧 store の退役で止まる。
- 旧 store の本人の entry・Dome の pin・最近の他人の内容を、新しい store と cache へ 1 回 128 対象以内で移す。
- 移し終えたら旧 store を名前を変えてから file を 1 回 128 件以内で消す。旧 store を前提にした読取りを撤去する。
- CN の旧 store を endpoint ID を保って退役させる。

対象外: R2-B・R2-D・R6-A、新しい store の中の新形式の定常の回収(R5-A の予算の回収は既存のまま)。

## 受入条件と状態

| ID | 対象・期待結果 | 状態・試験 |
| --- | --- | --- |
| AC-1 | 新しい store と endpoint ID の維持。新規の account は旧 store を作らない | iroh-node `adopting_the_legacy_endpoint_secret_keeps_the_endpoint_id`。desktop-runtime `a_legacy_store_moves_in_bounded_steps_and_is_retired`(退役の前後で endpoint ID が同じ) |
| AC-2 | 本人の書込みを書いたときに保護所有先へ。ACK で送信待ちの保護が外れる。他人の author の領域へ置いた行は入れない | desktop-runtime `own_writes_go_to_the_protected_owner_when_written`・`a_pinned_asset_is_protected_when_pinned`。app-api(`iroh-integration-tests`)の全件 |
| AC-3 | 旧 store からの有界な移行(永続 cursor、再起動からの再開)、課金の外の projection 行と旧保護の成人向けの hash | iroh-node `own_entries_move_in_bounded_pages_and_resume_from_the_position`・`pin_tags_move_with_their_blobs`。store `legacy_projections_move_to_the_ledger_or_are_reclaimed_in_pages`・`retirement_waits_for_the_migration_after_the_switch`。desktop-runtime の退役の試験 |
| AC-4 | 旧 store の退役(名前を変えてから 1 回 128 件以内で消す)と、旧 store を前提にした読取りの撤去 | iroh-node `a_retired_directory_is_removed_in_bounded_steps`。desktop-runtime の退役の試験(1 ステップで消す file が 128 以内、旧 store の file が 0) |
| AC-5 | CN の旧 store の退役 | cn-indexer(Postgres)`a_retired_legacy_store_keeps_the_endpoint_and_the_bucket_reader` |
| AC-6 | 検証 | desktop-runtime `a_legacy_store_moves_in_bounded_steps_and_is_retired`(下記) |
| AC-7 | 文書 | ADR 0048 §3・§7.1、ADR 0054 §5・§6、ADR 0055 §5、`docs/runbooks/community-node-production-rollout.md` §5.8・§8、本書 |

AC-6 の試験の内容: 更新前の端末の保存状態(旧 store に本人投稿・bookmark・private channel・未 ACK の DM と添付・avatar・
custom reaction asset と bookmark・live・Dome の pin、他人の旧投稿の行 201 件)を作り、1 ステップで止めて再起動し、保存した
位置から退役まで進める。退役の後に、旧 store の file が 0、endpoint ID が同じ、envelope と送信待ちの行数が変わらない(受入済みの
旧領域を再発行しない)、本人投稿の本文と添付・4 保護種別・DM の添付・reaction・avatar・Dome pin(pin の状態を含む)を再表示できる、
`author::` の制御領域を新しい store から読める、最近の他人の投稿は cache から表示でき古いものは消える、他人の旧投稿の成人向けの
hash は消えて本人の分は残る、退役の後は参加状態が変わっても保護移行の台帳を読み直さない、を確かめる。実 Iroh で、別の node が
本人の投稿の record(exact)と旧 replica のページ(keys)を読み、本文の blob を `RemoteBlobProtocol` から取得する。backup → restore の
後も添付・Dome pin・reaction・DM の添付と送信待ちの行が戻る。

## 入口と副作用

| 入口 | helper | 結果 |
| --- | --- | --- |
| client の起動 | `adopt_endpoint_secret` → `SharedIrohStack::new(<db>.iroh-store)`、`LegacyStore::open(<db>.iroh-data)` | 旧 root の secret を一時 file 経由の rename で写す。中身(`docs.redb`)の無い旧 root は `.retiring` へ回す |
| 本人の blob の書込み | `BlobService::put_owned_blob`(`put_blob` は `own_blob:<hash>`) | 保護参照を先に置いてから保護所有先へ置き、iroh の store にも置く。DM の frame・暗号化添付と epoch の制御 frame は `dm_outbox:`、平文と復号した添付は `dm_message:` |
| 本人の record の書込み | `IrohDocsSync::apply_doc_op` → `protect_own_record` → `SqliteStore::put_owned_record` | `own_docs` の保護参照。`author::<他人>` とその author bucket へ置く行は置かない |
| pin | `IrohBlobService::pin_blob` | `dome_pin:<hash>` で保護所有先へ置く |
| 背景 task | `DesktopRuntime::start_legacy_store_retirement` → `legacy_store_step` | R5-G の 1 ステップと R5-I の各 kind の 1 ページ。続きがあれば 100ms、退役の条件を待つ間は 60 秒、旧 store が無くなれば止まる |
| 退役 | `SqliteStore::legacy_store_retirable` → `LegacyStore::close` → rename → `remove_dir_step` | ADR 0048 §7.1 の条件で旧 store を閉じ、名前を変え、1 回 128 件以内で消す |
| 旧 store の無い account | `protected_migration_step` → `settle_without_legacy_store` | 保護移行を済んだものとして writer を切り替える(台帳のページを読まない) |
| CN の起動 | `retire_legacy_layout` → node(`iroh-store/`)、`IndexMaintenance::with_legacy_store` | data dir の直下の旧 store を `legacy.retiring/` へ移し、巡回ごとに 128 件以内で消す |

## 撤去したもの・残したもの

- 撤去: `IrohDocsNode::export_local_blob`・`list_local_tags`・`read_offline_blob`(旧 store の読取りは `LegacyStore` だけ)、avatar の
  `iroh-data` と手元の node の fallback、`put_remote_blob`・`put_remote_blob_file` の「iroh の store にあれば cache へ入れない」、
  pin の世代(`pin_generation`)と R5-G の pin の読み直し、Dome の `legacy_dome_replica`(instance・hosting・owner の記録の旧 context
  replica の fallback)、`start_protected_migration`(退役の背景 task へ統合)。
- 残した: R5-G の保護移行の kind(旧 store の分を読み、退役で止まる)、`fetch_local_blob` の iroh の store の読取り(新しい store の
  docs の内容と pin)、プロフィールの `author::<pubkey>` の読取り(新形式の制御領域でもあり、本人の分は新しい store へ移る)、
  Dome の接続の記録の旧 context replica の読取り(切替前の書き先で、本人の分は新しい store へ移る)。

## 検証(局所)

- iroh-node 50 件、store 171 件(schema golden を再生成、世代数 44)、docs-sync 67 件、blob-service 12 件成功。
- desktop-runtime lib 321 件成功。app-api(`iroh-integration-tests`)lib 509 件成功。
- cn-indexer(Postgres)全件成功(`bucket_reader` 5 件を含む)。
- `cargo xtask cn-e2e` 成功。scenario: `desktop_device_backup_restore`(endpoint secret の確認を `iroh-store` へ直した)・
  `desktop_smoke_bookmark_workflow`・`community_node_public_connectivity`・`community_node_index_query_client`・`e2e-smoke` 成功。
- kukuri-cli(Windows)全件成功、harness(`RUST_MIN_STACK=67108864`、`--test-threads=1`)23 件成功、Linux(Docker `rust:1.92.0-bookworm`)の
  `crates/kukuri-cli/tests/process_e2e.rs` 5 件成功。
- mutation check(外すと失敗することを確認して戻した): 本人の record の保護を外す・他人の author の領域も保護する・本人の blob を
  保護所有先へ入れない(`own_writes_go_to_the_protected_owner_when_written`)、pin の保護を外す(`a_pinned_asset_is_protected_when_pinned`)、
  本人の entry を写さない(iroh-node と退役の試験)、1 回の上限を外す・file の削除の上限を外す(iroh-node)、名前を変えずに退役する
  (退役の試験)、最近の他人の行を台帳へ移さない(store と退役の試験)、参照の残る成人向けの hash も回収する・切替の後の終端を待たない
  (store)、CN の旧 store を移さない・保守の巡回で消さない(cn-indexer)。14 通りすべてで失敗した。
- Tauri crate: worktree では workspace 解決のため、一時的に `[workspace]` を足して `cargo check --locked` が成功(manifest は戻した)。
- 未実行: frontend の test(IPC 型は変えていない)、全体の CI。
- `cargo clippy --all-targets -D warnings`(store・iroh-node・docs-sync・blob-service・app-api・desktop-runtime・cn-indexer・harness・
  kukuri-cli)、`cargo fmt --check`、`cargo xtask oversized-files`(違反 0。baseline は `dome_hosting.rs` の縮小を反映)成功。

## 既知の制約

- 本人の blob は保護所有先と新しい iroh の store の両方に置く(SQLite を失ったときに docs から戻す既存の復元のため)。pin した asset も
  保護所有先と iroh の store の両方にある。
- 本人の record の保護(`own_docs`)は外さない。他人の Dome の接続の記録を手元の anchor へ置いたもの(`apply_doc_op` で置く)も保護する。
- 旧 store の本人の entry は、新しい store へ書込みの時刻で書き直す(新しい store に同じ key があれば写さない)。この端末の docs author
  以外の entry(同期で入った他人の entry)は写さない。参加中 channel の現 epoch の replica の他人の record(参加者から見た owner の
  metadata・policy)は新しい store へ写さず、R5-G が保護所有先へ写した record を capability つきの key 指定の読み出しで読む。
- 他人の旧投稿のうち予算に入らないもの・最近でないものは手元から消える(ユーザー判断)。対象は投稿の projection 行と成人向けの hash。
  他の派生表(reaction・live/game の cache など)の旧同期の行は対象外。
- 退役の条件は writer の切替の後の保護移行の終端なので、切替から最短で 1 回の待ち(60 秒)の後に退役する。
- account 一覧の avatar は保護所有先だけから読む。更新の後に一度も開いていない account(R5-G の移行が済んでいない)の avatar は、
  その account を開いて移すまで一覧に出ない。
- CN は safety provider を構成しない起動では node と保守の巡回を作らないため、`legacy.retiring/` は構成した起動まで残る。

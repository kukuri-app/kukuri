# #1221 R5-G: 保護データの移行と backup/restore 入力の切替

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R5-G と、2026-09-26 のユーザー決定「R5-G 保護データの範囲とbackup」。
基準は #1371 merge `185febd5`。依存の R5-A・R5-B・R5-C は完了済み。

- 旧 `iroh-data` にしか無い本人のデータを、R5-A の remote cache と保護参照(大きい blob は `kukuri.remote-blobs/` の file)へ移す。
  新しい store は作らない。対象は 4 保護種別(本人投稿・bookmark・private 参加状態・未送信 outbox)と依存 record/blob、DM 履歴の
  添付(平文)、custom reaction bookmark の asset、本人の profile avatar、自作の custom reaction asset、自作 Dome の pin 済み asset
  (preset manifest と asset_refs)、自分が署名した live/game の manifest。private は現 epoch の記録(metadata・policy・自分の
  participant・自分宛 grant)。
- 対象索引を 1 回 128 件以内の cursor で歩き、コピー照合と移行状態を `protected_migration` に保存する。
- backup は旧 `iroh-data` を含めず、保護所有先(`kukuri.db` 一式と保護された `kukuri.remote-blobs/` の file)を含める
  component version 2。旧形式(1)の復元は拒否する。backup 作成時に移行が済んでいなければ、同じ操作の中で残りを 128 件ずつ移してから作る。
- `AppService::list_bookmarked_posts` 等の全件 API を撤去し、CLI の bookmark 一覧を cursor つきのページ出力にする。
  `list_bookmarked_custom_reactions` は新しい順の 200 件までにする。
- 閉じた旧領域を削除する前提を確定する(削除そのものは R5-I)。

対象外: owner の参加者名簿(R5-H の参加 record 配送)、writer の切替と旧同期の撤去(R5-H)、旧領域の回収(R5-I)、
会話全体の手元削除での参照の解放、private channel 退出での参照の解放、Dome の unpin での参照の解放。

## 受入条件

| ID | 対象・期待結果 | 判定 |
| --- | --- | --- |
| AC-1 | 各 kind の 1 ページは索引 128 行以内で、位置を保存した後に止めても、開き直すと続きから読む | store: 300 行で各ページ ≤128・再接続後に続き・重複 0 |
| AC-2 | 4 保護種別と追加の kind、共有 blob を旧領域から写し、SDK の bytes と hash が一致し、保護される | 実 Iroh: 1 ステップ→shutdown→再起動で完了、各 hash の bytes・BLAKE3・`is_protected` |
| AC-3 | 共有 blob(本人投稿＋bookmark＋非保護 cache 行)は 1 行だけで、bookmark を外しても保護が残り、保護行は回収と 1GiB 計数の外 | store: 行数 1、参照の残り、`used_bytes`、回収 0 |
| AC-4 | ACK で outbox 行を消す transaction の中で `dm_outbox:` の保護を外す。index 行が消えた後の移行は参照を付けない | store: ACK 後の参照 0・非保護、競合時の `false` |
| AC-5 | backup→restore で、archive に `iroh-data` が無く、本人投稿の本文・添付 bytes、bookmark、参加 channel、未 ACK の outbox 行と frame、avatar が一致する。旧形式(1)は拒否する | 実 Iroh の backup/restore、core の版の拒否 |
| AC-6 | 移行が終端へ達していない account の backup は作らない | desktop-runtime: 1 kind を戻すと作成が失敗し archive 無し |
| AC-7 | 全件 API を撤去し、CLI の bookmark 一覧はページ出力、custom reaction bookmark は上限つき | CLI の schema と対応表、store の上限 |
| AC-8 | 1 ページの読みは表の件数に依存しない | VM 命令数: 400 行と 2,000 行で 1.25 倍以内(4 kind) |

維持する条件: INVAR-1 旧領域を読み取るだけで書き換えない(削除は R5-I)。INVAR-2 private の cache の record は capability が
無ければ読めない。INVAR-3 保護データを回収して容量を達成しない(R5-A)。INVAR-4 全件の走査・一括再読込をしない。
INVAR-5 旧領域から何も読めなかった参照(復元後・旧領域の回収後)は置き換えず、既にある保護を減らさない。

参照の名前: `own:<envelope id>`(本人投稿・自作 custom reaction asset)、`bookmark:<object id>`、`reaction_bookmark:<asset id>`、
`dm_outbox:<dm id>/<message id>`、`dm_message:<dm id>/<message id>`、`live:<session id>`、`game:<room id>`、`avatar:<pubkey>`、
`private:<topic>/<channel>`(現 epoch の記録で置き換える)、`dome_pin:<hash>`。

索引と位置: `envelopes`・bookmark・custom reaction bookmark・DM 履歴は rowid（最大の行が消えた直後の追加で rowid が再利用されても読み飛ばさないよう、移行済みの位置より前の rowid で行が入ったら trigger が位置を戻す）、送信待ち DM は `(created_at, message_id, dm_id)`、
live/game は更新で rowid が変わらないため `(derived_at, id)`(索引を追加)、avatar は写した hash、private は capability の一覧の key
(一覧は起動時に全件読み込み済みのため、起動時と変更時に先頭から読み直す)、Dome は SDK の pin tag の名前。

## 入口と副作用

| ID | 対応 | 入口 → helper | 期待結果 / 禁止する副作用 |
| --- | --- | --- | --- |
| T1 | AC-1/2/8 | `ClientHost` の起動・切替 → `DesktopRuntime::start_protected_migration` → `protected_migration_step` → `SqliteStore::protected_migration_page` / capability 一覧 / `IrohDocsNode::list_local_tags` | kind ごとに 128 件以内、満杯なら 100ms・追いついたら 60 秒。shutdown で abort |
| T2 | AC-2 | `protect` → `set_protected_refs` → `copy_legacy_blob`(`export_local_blob` で BLAKE3 照合)/ `put_remote_record` | 参照を先に付けてから写す。旧領域に無いものは写さない。旧領域を書き換えない |
| T3 | AC-4 | `remove_direct_message_outbox`・`delete_direct_message_message_local`・`remove_bookmarked_custom_reaction` → `set_refs_in(.., [])` | 行の削除と同じ transaction で参照を外す |
| T4 | AC-5/6 | tauri/CLI の backup → `finish_protected_migration` → shutdown → `create_device_backup` → `SqliteStore::read_protected_backup_files` | 停止後に 1 本の接続で完了を確かめ、保護 file だけを列挙。`iroh-data` を列挙しない |
| T5 | AC-5 | private の key 指定の読み出し → `IrohDocsSync::with_private_cache` | capability の確認(replica を開く)の後だけ cache を足す |
| T6 | AC-5 | `ClientHost::account_display` → `read_profile` | avatar は保護所有先を先に読み、旧領域は移行前の fallback |
| T7 | AC-7 | CLI `list_bookmarked_posts` → `list_bookmarked_posts_page` | cursor の後ろの 1 ページ |

## 実装の照合

- store: migration `20260926010000_protected_migration`(`protected_migration` 表と、live/game の `(derived_at, id)` 索引)。
  `protected_migration.rs`(kind ごとの page、位置の保存、`protected_migration_caught_up_at`、index 行の存在を確かめる
  `set_protected_refs`、backup 用の `read_protected_backup_files`)。保護参照の変更を行の変更と同じ transaction にする
  `begin_protected_ref_update` / `set_refs_in` / `commit_protected_ref_update` へ bookmark の既存処理を寄せ、ACK・DM の手元削除・
  custom reaction bookmark の解除で参照を外す。`list_bookmarked_custom_reactions` は 200 件まで。
- core: `open_sent_direct_message_frame`(送信者として frame を開く)。`DEVICE_BACKUP_COMPONENT_VERSION = 2`。
- iroh-node: `export_local_blob`(旧 blob store から file へ写して BLAKE3 を照合)、`list_local_tags`(pin tag を名前順に上限つき)。
- docs-sync: `read_legacy_records`(namespace を import せず、登録済みの capability で旧領域の 1 key を読む)。private の key 指定の
  読み出しに record cache を足す(`with_private_cache`)。
- app-api: `list_bookmarked_posts` を撤去、`joined_private_channel_replicas` を追加。
- desktop-runtime: `runtime/protected_migration.rs`(移行の 1 ステップ、drain、背景 task)、shutdown で停止、capability の変更で
  private の位置を先頭へ戻す。backup の入力から `iroh-data` を外し、保護 file を含める。account 一覧の avatar。
- CLI: `list_bookmarked_posts` をページ出力へ。`command-parity.json` の R1-A の記録と scope revision を改める。harness も page API。
- 文書: ADR 0048 §3/§4/§7 と Consequences、`docs/architecture/blob-cache.md`、`docs/legal/device-backup-data-classification.md`。

旧領域を削除できる前提(ADR 0048 §7): R5-H の writer 切替を永続化した後に、全 kind の `caught_up_at` がその時刻より後であること。

## 検証(局所)

- 変更前に失敗する条件: backup は `iroh-data` の全 tree を毎回含め、旧領域が無い account では本人データを復元できない。
  bookmark 以外の本人データは cache に保護参照が無く、ACK 後も送信待ちの保護が外れない経路が無い。CLI は bookmark を全件で返す。
- store `cargo test -p kukuri-store --lib` 166 件成功(schema golden を再生成、世代数 41)。`protected_migration` の 4 件
  (AC-1/3/4/8。VM 命令数は own_envelope・bookmark・dm_outbox・live_session の途中の 1 ページで 400 行と 2,000 行を比較)。
- core 145 件成功(送信者側の frame、component version 1 と未知版の拒否)。iroh-node 49 件、blob-service 12 件成功。
- docs-sync 70 件成功(1 件は既存の ignored)。private の cache の record は capability を外すと読めない(INVAR-2)。
- app-api lib 504 件成功。CLI `command_parity` 5 件・lib 32 件成功。
- desktop-runtime lib 313 件成功(実 persistent Iroh の `legacy_protected_data_moves_in_pages_and_restores_without_the_legacy_tree`:
  1 ステップ後は 2 ページ目の本人投稿の record が未移行、再起動後に完了、10 種の hash の bytes・BLAKE3・保護、1MiB 超は file、
  追いついた後の新しい channel、archive に `iroh-data` が無く保護 file がある、復元先で投稿・添付 bytes・bookmark・channel・
  未 ACK の outbox と frame・avatar、復元後の読み直しで保護が減らない。`private_plan` の空の判定を外すと最後の確認が失敗する
  ことを確認)。IdentityStorage を取る 65 件(device_backup・identity_restart・protected_migration・receive_binding・
  runtime_events・account)を 3 回続けて実行し、すべて成功。
- harness: `cargo xtask scenario desktop_device_backup_restore`・`desktop_smoke_bookmark_workflow` 成功。
- `cargo clippy --all-targets -D warnings`(store・core・iroh-node・docs-sync・blob-service・app-api・desktop-runtime・cli・harness)、
  `cargo fmt --check`、`cargo xtask oversized-files`(違反 0。`joined_private_channel_replicas` は `private_channel_rendezvous.rs`、
  harness の一覧は `ScenarioRuntime::bookmarks`、docs-sync の helper と test は `iroh_local_source.rs`・`tests/iroh_sync.rs` に置いた)。
- Tauri crate: worktree では workspace 解決のため、一時的に `[workspace]` を足して `cargo check --locked` が成功(manifest は戻した)。
- 未実行: frontend の test(mock の文字列 1 件だけ)、全体の CI。IPC 型は変えていない。
- rowid の再利用: `a_reused_rowid_after_the_cursor_is_read_again`（最大の bookmark を消した直後の追加が、移行済みの位置の手前から読み直される）。
- 監査の指摘（固定 head `98f9a1b4`）への修正: (1) Dome pin の tag は hash の順に並ぶため、追いついた後に pin した asset を
  読み飛ばしていた。blob service が pin の世代を持ち、起動時と pin したときに dome_pin の位置を先頭へ戻す
  （`assets_pinned_after_catch_up_are_moved_whatever_their_hash`、読み直しを外すと失敗することを確認）。(2) 本人の envelope 行は
  docs の record より先に入るため、並行時に record を取り残して位置を進めていた。依存 record（state、公開投稿はプロフィールの行、
  custom reaction asset は asset の record）がまだ無い作成 10 分以内の行の手前で位置を止め、次のステップで読み直す
  （`own_envelope_waits_for_its_docs_records`）。backup 前の drain は、読み直しの間 100ms 待つ。

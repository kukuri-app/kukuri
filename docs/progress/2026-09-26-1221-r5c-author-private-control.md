# #1221 R5-C: author と private 制御参照を対象別に解決する

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R5-C。区分 C、Scope revision
`2026-09-24-outcome-v5-r2a-retention-1`、基準は #1366 merge `9cdec8ab`。依存の R5-B は完了済み。
R5-B と同じ有界な docs 読取り（QUIC の key ページと exact 読取り）を、author と private channel の
制御参照へ接続する。remote 読取りのために namespace を import/open/start_sync しない。

- author の現在値（`profile/latest`、`graph/follows/<相手>`、`graph/blocks/<相手>`、Dome preset/move の state）は
  `author::<pubkey>` の制御領域から key 指定で読む。履歴（プロフィールの投稿・repost の索引）は
  author bucket（cursor の bucket、隣接 bucket、現在 bucket）と移行中の旧 `author::<pubkey>` から読む。
- private の参加（招待・friend-only grant・friend-plus share の取込み、rotation の handoff grant の受取り）は、
  epoch の metadata・policy・owner と自分の参加 record・自分宛 grant だけを key 指定で読む。
- 手元（`LocalOnly`）に無い key だけを remote から読む。provider は、書き手（author 本人、channel owner、
  token の発行者）の検証済み宛先（R4-A の account 宛先解決）を先頭に、既存の有限候補
  （公開は全体の候補窓、private は当該 channel の gossip scope）で埋め、最大 4 件。1 操作 30 秒、
  局所照合 200 行、応答 1MiB・record 64KiB（R5-B の上限）。private の要求は capability の証明つきで、
  channel の参加者（発行者・owner・gossip scope）だけへ送る。

対象外: author/topic/channel の購読 task の寿命と旧定常 sync（R2-C/R5-H）、自分の関係の再計算
`rebuild_author_relationships` と DM 購読の再構築（R4-D の「mutual関係更新」）、owner の rotation で
全参加者へ grant を配る列挙と、参加者数の表示（`private_channel_diagnostics`。対象参照ではなく
audience 全体の操作）、新 writer（R5-H）。グラフの全件復元と、他の著者の follow/block の窓（各 512 件）の
remote 復元はしない（手元の record だけを読む）。

## 受入条件

| ID | 対象・期待結果 | 判定 |
| --- | --- | --- |
| AC-1 | author 購読の起動・docs event・追いつき、author の social view で、手元に無い現在値の key（profile と自分を指す follow/block、event の key）を有界な provider から読み、署名と author を確かめてから保存する。起動時に 5 秒で打ち切る `LocalThenRemote` の全体 hydration と、読めないときの購読の再起動を撤去する | 実 Iroh で provider の author 履歴を 10 倍にしても読む key 数が同じ。client は namespace を import しない。provider が無くても表示が返る |
| AC-2 | プロフィールのタイムラインで、手元のページが埋まらないときだけ remote のページを読む。author bucket と旧 replica の索引を cursor から読み、署名・author・bucket の時刻を確かめた行だけを返す。複数の source を合わせても、次のページが行を飛ばさない | 実 Iroh で bucket と旧形式の投稿を表示し、件数を増やしても読む行が 200 行以内。source の分け方を変えた乱数の操作列で、全行が 1 回ずつ新しい順に出る |
| AC-3 | Dome preset/move の state と署名 envelope を、手元の次に有界な provider から読む | 手元に無い preset を remote から読み、namespace を import しない |
| AC-4 | private の取込みと handoff grant の受取りで、epoch の metadata・policy・owner と自分の参加 record・自分宛 grant だけを読む。全参加者の読取りの 50ms 間隔の反復と、grant を待つための sync の再開を撤去する。friend-plus の共有・退出・Dome の入場判定も参加 record を 1 件だけ読む | 実 Iroh で発行者の端末から取込み、参加者を 10 倍にしても読む key 数が同じ。epoch・署名・audience が合わない record では参加しない。退出後は要求しない |

維持する条件:

- INVAR-1: 署名、author と key の一致、epoch・audience・共有状態・mutual の判定を変えない。
- INVAR-2: private の要求は channel の参加者だけへ capability の証明つきで送り、公開の候補窓へ送らない。
- INVAR-3: `LocalOnly` の読取りは remote I/O をしない。自分の replica（自分のプロフィール、custom reaction の一覧）の読取りは変えない。
- INVAR-4: 通信の優先度 Direct P2P → Relay Supported P2P → Relay Fallback は既存の transport のまま。

## 入口と副作用

| ID | 対応 | 入口 → helper | 期待結果 / 禁止する副作用 |
| --- | --- | --- | --- |
| T1 | AC-1 | `spawn_author_subscription` の起動 / docs event / 追いつき → `hydrate_author_state`・`hydrate_author_key`・`catch_up_author_state` → `hydrate_author_record` → `put_envelope` | 手元の次に author の provider から固定 key。署名済み envelope だけを保存。`LocalThenRemote` の namespace sync を読取りに使わない |
| T2 | AC-1 | `get_author_social_view`・`list_profile_timeline` | 購読の再起動（`maybe_restart_author_subscription`）をしない |
| T3 | AC-2 | `list_profile_timeline` → `profile_timeline_page` → 手元のページ / remote の source ページ → 合流 | 1 操作 200 行・30 秒。bucket の外の時刻の行と別 author の行を返さない |
| T4 | AC-3 | Dome の hosting・管理・移動 → `fetch_dome_preset_manifest`・`fetch_dome_move_record` | 手元の次に author の provider。署名 envelope の照合を維持 |
| T5 | AC-4 | 招待・grant・share の取込み → `load_private_epoch_snapshot`（発行者と owner の宛先） | metadata・policy・owner/自分の参加 record。失敗時は secret の登録を戻す（既存） |
| T6 | AC-4 | 書込み・一覧の前の `maybe_redeem_epoch_handoff_grants_for_channel`、`private_channel_rotation_is_pending` | 手元の policy が rotation を示すときだけ grant を remote で読む。sync の再開をしない |
| T7 | AC-4 | friend-plus の共有、退出、Dome の入場判定 | 参加 record 1 件の key 読取り |

## 検証

局所では変更に関係する test だけを実行し、全体は PR の CI で確認する。固定 head の独立監査で
blocker が 0 なら終了する。

## 実装の照合（監査前）

- `crates/iroh-node` / `crates/docs-sync`: QUIC の docs 読取りが `author::<pubkey>` と author bucket を公開 replica として
  受け、key の一覧に docs author の指定を持つ（他の名義の entry はページを埋めない）。`RemoteDocsSource` は
  docs author を指定した key の一覧と、応答が prefix・名義の外の key を含まないことの確認を持つ。
- `remote_read_support.rs`: `writer_readers` が書き手の検証済み宛先（R4-A）を先頭に最大 4 provider を選び、
  `read_local_then_remote` が手元の次に provider を 30 秒以内で順に読む。source は所有して渡し、呼び出し側の
  future を `Send` のまま spawn できる形にした。
- author（AC-1〜3）: `AuthorKeyReader` が手元に無い現在値の key だけを remote から読む。起動時の 5 秒の
  `LocalThenRemote` の全体 hydration と、読めないときの購読の再起動（`maybe_restart_author_subscription`）を撤去。
  プロフィールは手元のページが埋まらないときだけ、旧 replica と author bucket の索引を provider ごとに読み、
  `assemble_profile_page` がどの source の読み残しより新しい行だけを返す。Dome preset/move も同じ reader。
- private（AC-4）: `load_private_epoch_snapshot` が metadata・policy・owner と自分の参加 record だけを読み、
  50ms 間隔の全参加者の読取りと `restart_replica_sync` による待機を撤去した。取込みは token の発行者と owner の
  宛先だけへ、参加中は owner と channel の gossip scope へ capability の証明つきで要求する。handoff grant は手元の
  policy が rotation を示すときだけ remote で読む。friend-plus の共有・退出・Dome の入場判定は参加 record 1 件。
  到達しなくなった `OwnerInactive` と取込みの `refetch_participants`・`check_owner_active` を撤去した。

## 検証（局所）

- 失敗する変更前の条件: 手元が空の client で、プロフィール・author の現在値・Dome move・private の取込みが
  provider から読めること（旧実装は namespace の sync に依存し、手元が空だと空・取込みの時間切れ）。
  空のプロフィールで購読を再起動しないこと（旧 test `list_profile_timeline_restarts_author_subscription_...` を置換）。
- 単体: `author_remote_reads`（edge 60/600・投稿 100/1,000 で provider の読取り量が同じ、全行が 1 回ずつ新しい順、
  自分のプロフィールは provider を読まない、Dome move）、`private_epoch_reads`（参加者 5/50 で読取り量が同じ、
  参加者一覧の prefix 読取り 0）、`profile_merge`（source の分け方・読む件数・ページの大きさを乱数で変えた 300 通りで
  全行が 1 回ずつ新しい順）。app-api の lib 全 510 件成功。
- 実 Iroh: `real_iroh_author_profile_is_read_from_the_author_device_without_sync`（author bucket と旧 replica の
  26 件を 2 ページで表示、profile と自分を指す follow を反映、client の docs へ取り込まない）、private channel の
  invite・friend-only・friend-plus・leave（テストの端末に desktop と同じく account の受信 binding を設定）、R5-B の
  reader の結合 test。friend-plus の取込み直後の新 epoch 投稿は、取込みが epoch の全体 sync を待たなくなったため、
  他の可視性の確認と同じく期限内の表示を待つ形にした。
- iroh-node の `author_replica_keys_are_read_by_the_requested_docs_author`、docs-sync の remote source。
- 変更 crate の `cargo clippy --all-targets -D warnings`（app-api は `iroh-integration-tests` つき）、
  `cargo fmt --check`、`cargo xtask oversized-files`。`dome_connections.rs` は取得方針を明示する 3 行で
  baseline を 1025→1028 に更新し、縮んだ 3 file は下げた。desktop-runtime・cli・harness の `cargo check --all-targets`。
- 全体の suite と Tauri の check は PR の CI で確認する。

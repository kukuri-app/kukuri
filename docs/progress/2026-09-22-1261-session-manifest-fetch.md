# #1261 session manifest の取得前 hash 検証

## 範囲と判断

- Issue: https://github.com/kukuri-app/kukuri/issues/1261
- リスク区分: C。Scope revision: 2026-09-22。
- 基準 commit: `e652c3df46718d81318ea2462869cbed79863145`。
- ユーザーが計画、Issue 作業、commit、PR、CI 後の merge を承認。PR head の独立監査は別工程で行う。
- AC-1: 署名された manifest と一致しない hash は BlobService を呼ばず拒否する。
- AC-2: 旧 Dome の remote 取得は互換例外として残し、取得前に state の key / scope / owner と Dome ID の整合を必須にする。
- AC-3: object ごとの docs 読出しと blob 取得は定数上限を維持する。
- AC-4: ADR 0052 §2 に方式と例外を記録する。
- INVAR-1〜3: 正常な live / ScoreGame / Dome と後着 blob、#1252 の検証、現行 record と旧 Dome の読出しを維持する。

local のみへの制限は、別端末で旧 Dome を初めて読む動作を失うため採用しない。旧 Dome の ID は所有者の認証ではない。
例外では未署名 hash の取得が残る。署名つき分岐で hash が一致しない場合は、旧形式へ fallback しない。
投稿本文 / 添付、Dome Preset / Instance の取得、wire / DB schema、同期全体の再設計は対象外。

署名検証済み envelope.content の UTF-8 bytes を hash 化する。型への deserialize / 再 serialize は未知 field や field 順序を失うため使わない。
現行 writer は `sign_envelope_json` の `serde_json::to_string` と `store_manifest_blob` の `serde_json::to_vec` で同じ JSON bytes を出力する。
取得後の manifest 比較、blob が無いときの未反映、既存の retry と revision の順序を維持する。

## 作業と対応

| Task | 対応 | 成果物・検証 | 依存 |
| --- | --- | --- | --- |
| T1 | AC-1 / TR-1 | hash 改変の失敗 test を修正前に実行 | なし |
| T2 | AC-1・3 / INVAR-1〜3 | session_integrity の取得前 hash guard | T1 |
| T3 | AC-2 / INVAR-1・3 | 旧 Dome の取得前 ID / scope guard | T2 |
| T4 | AC-3 / 全 INVAR | 入口、後着、再読出し、混在、件数非依存の tests | T3 |
| T5 | AC-4 / 全 AC | ADR と本記録、path 別ローカル検証 | T4 |
| T6 | 全 AC / INVAR | 固定 PR head の独立監査、CI、merge 後確認 | T5 |

## 固定 inventory

| ID | 入口・trigger | shared helper | 副作用 | guard | transition / evidence |
| --- | --- | --- | --- | --- | --- |
| INV-1a | catch_up_sessions（起動・一覧・窓の追いつき） | hydrate_live_session_from_record / hydrate_game_room_from_record → verify_*_record | blob 取得、cache status / 行更新 | 署名・既存検証・hash 一致、旧 Dome 例外 | TR-1・2・4・6 / session_catch_up、取得境界 tests |
| INV-1b | hydrate_subscription_event / hydrate_subscription_doc_event / hydrate_subscription_hint | hydrate_*_from_key_with_retry → load_verified_live_session または hydrate_game_room_from_record | 同上 | 同上、既存 retry 上限 | TR-1・2・4・5 / hint_rehydration、取得境界 tests |
| INV-2a | live の終了・参加・退出、game の更新・asset / event 操作、Dome 移動・layout 操作 | fetch_*_state_and_manifest → load_verified_* → verify_*_record | blob 取得、検証済み値を操作へ返す | 同上、既存 revision 選択 | TR-1・4・5 / hydration_integrity_sessions_contract、取得境界 tests |
| INV-2b | hosting_instance の他 owner 読出し（hosting / management / private channel） | load_verified_game_room → verify_game_room_record | blob 取得、後続 Instance 検証へ owner を渡す | 同上、既存 Instance 検証は変更しない | TR-1・3・4 / dome_listing、取得境界 tests |
| INV-3 | 署名済み manifest が得られない dome- state | verify_game_room_record の互換分岐 | 未署名 hash の blob 取得 | key、replica scope、state から導出した Dome ID、取得後の既存 metaverse 検証 | TR-3・4 / 旧 Dome 境界 test |

列挙方法: `rg -n 'fetch_manifest_blob|verify_live_session_record|verify_game_room_record|load_verified_live_session|load_verified_game_room' crates/app-api/src/service` と
`rg -n 'fetch_live_session_state_and_manifest|fetch_game_room_state_and_manifest|hosting_instance\(' crates/app-api/src`。
INV-2a の全 member は上記参照の `live.rs`、`game.rs`、`dome_move.rs`、`dome_hosting.rs`、
INV-2b は `dome_hosting.rs`、`dome_management.rs`、`private_channels.rs`。
署名検証 loader は 2 caller、live verifier は 2 caller、game verifier は 2 caller、live loader は 2 caller、game loader は 2 caller。
manifest 取得 sink は対象 verifier 内の 3 callsite。Preset caller は対象外として分類する。
write 側の Verified*::verify は blob 保存後の値を検証する入口であり、未信頼 state を契機にした fetch は行わない。

Sensitive sink は `fetch_manifest_blob` → `BlobService::fetch_blob`。拒否時は cache status / projection の更新も禁止する。
CodeGraph は .codegraph の存在検査後に実行したが CLI が索引なしと報告したため、索引を作らず rg と source の確認へ切り替えた。

## 状態遷移

| ID | 事前状態・event | 期待状態 | 許可 I/O | 禁止副作用 |
| --- | --- | --- | --- | --- |
| TR-1 | 正常 record の hash だけを変更 | 行不変、対象を拒否 | bounded docs | 全 BlobService 呼出し、行更新 |
| TR-2 | state / envelope が先着、blob 欠落・後着 | 後着後に反映 | 一致する hash の取得、既存 retry | 不一致 hash の取得 |
| TR-3 | 旧 Dome、blob local 無 / 後着 | 互換条件内で反映 | ID / scope 一致時の取得 | 例外条件外の取得 |
| TR-4 | 不正署名 / owner / revision / scope、署名つき hash 不一致 | 拒否 | bounded docs、旧 Dome は明記した例外のみ | 署名つき hash 不一致から旧形式への fallback |
| TR-5 | 反映後の改変、viewer 再作成・再読出し、正常不正混在 | 同じ guard、既存行・revision 保持 | 正常 object の取得 | 不正 object の取得 / 行更新 |
| TR-6 | 無関係な record の増加 | 対象 object の件数不変 | bounded exact read | prefix 全件読み追加、上限拡大 |

## 検証状況

修正前に `cargo test -p kukuri-app-api session_manifest_fetch --lib` を実行し、以下の 5 test が禁止取得で失敗した。
fixture の ScoreGame は参加者 2 人を必要とするため、fixture の初回誤りを直してからこの再現を記録した（build / fixture 失敗は再現に数えない）。

- `live_changed_hash_never_reaches_blob_service`
- `score_changed_hash_never_reaches_blob_service`
- `signed_dome_changed_hash_never_reaches_blob_service`
- `legacy_dome_allows_late_fetch_only_for_matching_identity_and_scope`
- `rejected_hash_preserves_projection_and_operations_across_viewer_restart`

修正前は正常系 3 passed / 境界 5 failed、修正後は同じ 8 test が passed。
その後、取得失敗からの復旧、hint / catch-up / 他 owner hosting の拒否確認も同じ tests に加えた。

| 条件 | 主な evidence |
| --- | --- |
| AC-1 / TR-1・4 | 3 種の changed_hash test、rejected_hash_preserves_projection_and_operations_across_viewer_restart（全 BlobService 呼出し 0、既存行不変） |
| AC-2 / TR-3 | legacy_dome_allows_late_fetch_only_for_matching_identity_and_scope（不正 ID / owner / topic / channel / key は取得 0、正常な旧 Dome は後着後に取得・反映） |
| AC-3 / TR-6 | object_reads_and_fetches_do_not_grow_with_unrelated_records（無関係 0 / 1000 record、各 object の返却 docs 2・blob 取得 1、全 query が Exact） |
| AC-4 | ADR 0052 §2 の取得前 guard・互換例外 |
| INVAR-1 / TR-2 | valid_sessions_accept_late_blobs_without_changing_the_requested_hash（取得エラー・欠落・到着）、既存 topic_session_hints_retry_until_manifest_blob_is_available |
| INVAR-2 / TR-4・5 | hydration_integrity_sessions / hydration_integrity_sessions_contract、既存の mixed record・revision 巻戻し防止 tests |
| INVAR-3 | signed_content_bytes_are_used_without_reserializing_the_manifest（JSON field 順序・未知 field を保持）、正常 writer を使う fixture、旧 Dome test |

`cargo xtask check` は成功（Rust fmt / clippy、operator-neutrality、Tauri compile、frontend lint / typecheck）。
`cargo test -p kukuri-app-api --lib` は 432 passed / 1 failed。失敗は既存の
`reply_reads_a_constant_number_of_docs_records`（reply の計測で 3 / 329 record）で、同じ binary の対象 test 単独再実行は成功。
対象 session test はすべて成功。`cargo xtask test` は成功（nextest: 1366 passed / 5 skipped、doctest 成功、frontend: 249 files / 2001 tests passed）。
通常 nextest では上記 reply の test も成功した。`cargo xtask app-api-slow-test` も成功（実 Iroh を含む 463 passed、doctest 成功）。

`git diff --check` 成功。必須ローカル検証は完了。
独立コード監査は 5 group / 適合 5 / 不適合 0 / 未分類 0、blocker 0。
固定 PR head の最終監査、CI、merge 後確認は PR / Issue に記録する（この commit 時点では未実施）。

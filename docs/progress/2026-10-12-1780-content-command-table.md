# #1780: CLI content_socialの宣言統合

## 対象と成果

- 条件の正本: [#1780](https://github.com/kukuri-app/kukuri/issues/1780)、親 [#1777](https://github.com/kukuri-app/kukuri/issues/1777)。`AC-CLI / PR-CLI`、Scope revision `2026-10-12-refactor-r1`、区分C。
- 比較起点: `1f77dd4e3252b37fc944446544b901435c04063f`。
- `content_social.rs`の38 commandを、名前・effect・実行式を持つ一つのmodule内宣言表へ統合した。Operation enum、executeのmatch、registrationsをその表から生成し、3箇所の手書き対応を撤去した。

既存のasync_call/async_call0はDTO decode・encodeを共有するが、登録・enumを作る責務は持たない。そのため既存helperを維持した局所宣言macroを選んだ。新しい共有framework、crate、dependency、別の登録表は作っていない。

入口は従来の`commands::registrations → CommandRegistry → Dispatcher → Handler::execute → domain runtime`。HostReady guard、runtime取得→入力decode→実行→error redaction→encodeの順序、媒体/file適応と同期methodは元の実行式を維持する。Tauri/Web dispatchへ直接接続しない。

## metadataの変更前後比較

一時probeで`CommandRegistry::builtin().page`から公開catalogを64件ずつ読み、変更前と変更後の157 commandのmetadataをUTF-8 JSONで比較した。対象38件は名前・effect・guard・secret/streaming flag・入出力schemaを含めて一致し、catalog全体のbytesも一致した。

- before/after SHA-256: `1b2f10e81c5c92d06e331af9b1a24378cbfd3224da202c18b0d7698abf519fda`
- probeはcatalogの保存だけを行い、profile/runtimeを起動しない。秘密値や本人データを読んでいない。
- 初回probeはprivateなfor_sessionを呼んでcompile前提を誤り、test実行0で失敗した。publicなbuiltinへ訂正して変更前後とも成功した。
- probe sourceは計測後に撤去し、恒久snapshot/frameworkを追加していない。既存のcontract testは保持する。

## 局所検証

Windows/Rust1.98.1、`CARGO_TARGET_DIR=C:\Users\kgm11\kukuri\target`で変更前後に実行した。

| command | before / after |
| --- | --- |
| `cargo test --locked -p kukuri-cli --test command_parity` | 5 PASS / 5 PASS |
| 同 `--test single_execution` | 1 PASS / 1 PASS |
| 同 `--lib commands::` | 9 PASS / 9 PASS |
| 同 `--test content_live public_inputs_create_distinct_posts_once_each_and_reply_uses_domain_runtime` | 1 PASS / 1 PASS |
| 同 `--test content_live media_round_trip_uses_explicit_files_without_base64_json_or_overwrite` | 1 PASS / 1 PASS |
| `cargo check --locked -p kukuri-cli` | 変更後PASS |
| `cargo clippy --locked -p kukuri-cli --lib -- -D warnings` | 変更後PASS |
| `git diff --check`、対象rustfmt | PASS |

parity/単回実行、private入力を診断へ出さないこと、明示fileの媒体往復と既存file上書き拒否を維持した。test/binaryの既存linker_messages warningは前後共通であり、抑制していない。固定head独立監査と実必須CIの結果はIssue/PRへ集約する。

## コード量・終了

同じ`content_social.rs`の物理行を基準commitと完成sourceで比較し、**353→243（110行減）**。既存test・補助実装への変更/追加は0で、同じ責務の総量も110行減。計測probeの撤去を基準のコード削減に算入していない。証跡文書の追加は別計測する。

公開名、flat payload/schema、guard、error/redaction、単回実行、file入出力を変更していない。38の手書き正本は一つで、旧enum/match/登録表の手書き併存はない。差し戻しは本PR単独revert（永続移行なし）。監査・CI・merge後一致が揃うまでCompleteとしない。

# #1194 AC-1 Android の build 基盤（2026-10-09）

#1194 の AC-1（PR-A1-1）の記録。Scope revision `2026-10-09-r5`、基準 commit `41fdb34f3`（`integration/android-1193` の作成基点）。前提の判断は #1193 の D1・D3・D5・D7（2026-10-09 確定）。

## 採用した toolchain・SDK・ABI

| 項目 | 値 | 置き場所・根拠 |
| --- | --- | --- |
| applicationId・表示名 | `app.kukuri.android`・kukuri | `apps/desktop/src-tauri/tauri.android.conf.json`（D5）。desktop の `app.kukuri.desktop` と保存先は変えない |
| minSdk・targetSdk・compileSdk | 29・36・36 | 同上と `gen/android/app/build.gradle.kts`（D7、Google Play の target API） |
| ABI | 配布は arm64-v8a、検証は x86_64（emulator） | D7。32bit は対象外 |
| Rust | 1.98.1、target `aarch64-linux-android`・`x86_64-linux-android` | `rust-toolchain.toml` |
| Tauri | crate 2.12.0、CLI 2.12.0 | `src-tauri/Cargo.toml`、`apps/desktop/package.json` |
| Android project | `tauri android init` の出力。Gradle 9.6.1（wrapper）、Android Gradle Plugin 9.3.1、Kotlin Gradle plugin 2.2.10 | `apps/desktop/src-tauri/gen/android` |
| JDK | 17 以上。手元は Android Studio 付属の JBR 21.0.8 | Gradle 9 の要件 |
| SDK の部品 | platform android-36、build-tools 36.0.0（無ければ Android Gradle Plugin が取得する） | 初回の build の log |
| NDK | 手元は r29（29.0.14206865）。CI の check は runner の image の NDK | `cargo xtask android-check` が `ANDROID_NDK_HOME`・`NDK_HOME` から探す |

`tauri android init` の出力から直したもの: CLI の呼び方（init を実行した端末の node の絶対 path を埋め込んでいたため、PATH の node で `apps/desktop/scripts/tauri-cli.mjs` を呼ぶ）、compileSdk・targetSdk（雛形の 37 を 36 へ）、Android TV の宣言（D7 の対象外）、未使用の雛形（layout・色・night の theme）と既定のアイコン。ランチャーアイコンは `src-tauri/icons/android` を Gradle の res として直接読み、権利は既存の `docs/ASSET_MANIFEST.json` のまま管理する。

## Android 向けの依存の解決

- main `41fdb34f3` の `apps/desktop/src-tauri` の lib を `cargo check --target aarch64-linux-android`（NDK r29 の clang、API 29）にかけると、依存 crate（iroh・iroh-docs・iroh-blobs・iroh-gossip の fork、str0m、sqlx の SQLite、`kukuri-desktop-runtime` など）はすべて通った。C を含む依存（ring、libsqlite3-sys、secp256k1-sys、blake3 など）は NDK の clang で build される。
- 失敗は src-tauri の desktop だけの処理の 5 件だった: `lib.rs` の `tauri::menu`・`tauri::tray` と `unminimize`、`commands/os_notification.rs` の `show_platform_notification`（Android の実装なし）と `unminimize`。

## desktop と Android の境界

| 対象 | Android での扱い | 実装 |
| --- | --- | --- |
| tray・menu、tray からのウィンドウの復元 | build から外す | `lib.rs` の `cfg(desktop)` |
| 多重起動の制御 | 依存ごと外す | `Cargo.toml` の `cfg(not(target_os = "android"))` 節 |
| updater | plugin を登録しない。更新の command は plugin に触れる前に `update_managed_by_google_play` で断る | `lib.rs`、`app_update.rs`（D3。Play での表示と配布判定の一元化は #1199 AC-3） |
| 終了の横取りと signal による終了 | 使わない（process の終了は OS に任せる。lifecycle は #1196） | `lib.rs` の `RunEvent` の処理、`desktop_lifecycle.rs` |
| OS 通知 | 表示は `os_notification_unavailable`、権限は `unavailable`（許可済みと表示しない） | `commands/os_notification.rs`。#1197 AC-4 で置き換える |
| 開発版の兄弟 dir（`<identifier>.dev`、#1105） | 使わず、OS の app data dir（`/data/user/0/app.kukuri.android`）をそのまま使う。sandbox の外へは書けず、開発版と配布版は署名が違うため同じ端末に並ばない | `state.rs` の `base_app_data_dir` |
| 秘密の保存 | 未接続（Keystore の adapter は #1195 AC-1） | — |

frontend は変えていない。Tauri の IPC と既存の dispatch・起動の関門（`invoke_gate.rs`）は Android でも同じ経路を通る。

## 検証

| 対象 | command | 結果 |
| --- | --- | --- |
| Android（arm64-v8a）の compile | `cargo xtask android-check` | 成功、警告 0 |
| Windows の compile | `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml` | 成功、警告 0 |
| Android project の生成と設定 | `tauri android build --debug --apk --target x86_64`（`CARGO_PROFILE_DEV_DEBUG=line-tables-only`） | 成功。universal の debug APK 約 477 MB（Rust の共有 library 460 MB）。既定の debug 情報では 1.36 GB |
| 開発の入口 | `tauri android dev`（emulator） | 成功。Windows の Tauri CLI は devUrl の host を端末の LAN の address に置き換え `TAURI_DEV_HOST` を渡すため、Android の設定の開発 server は host を固定せず `vite.config.ts` がその値で待ち受ける。修正前は開発 server が 127.0.0.1 だけで待ち受け、emulator から届かなかった |
| desktop の変更箇所の単体 test | `cargo xtask tauri-test -- app_update os_notification desktop_lifecycle state::`（Windows） | 32 passed |
| CLI ラッパー・Vite の設定 | `pnpm typecheck`、`eslint vite.config.ts`、`vitest run scripts/tauri-cli.test.mjs` | 成功（7 passed） |

Linux の compile・配布の contract（`tauri-test --package-build`）と MSIX は PR CI（`Kukuri Fast`・`Kukuri Linux Package`・`Kukuri Windows Store Package`）で確かめる。

参考（AC-2 の証跡ではない）: 上の APK を emulator（AVD の Medium Phone、API 36.1、x86_64、page size 4 KiB）へ入れて起動すると、共有の React 画面が Tauri の IPC で起動状態を受け取り、同意画面（利用規約 v10・18 歳以上の確認）を表示した。log は `app-level legal consent required; deferring runtime startup` で、app data dir には profile の lock（`.kukuri-profile.json`・`.kukuri-profile.lock`）だけがあり DB は作られていない。修正前は開発版の兄弟 dir `/data/user/0/app.kukuri.android.dev` を作れず（Permission denied）、起動失敗の画面になっていた。画面の上端が status bar と重なる（edge-to-edge の insets）のは #1198 AC-3 で扱う。

未実施（担当）: 実機での起動・再起動と同意前の禁止 I/O の観測、release の AAB、#1195 の保存の接続（#1194 AC-2）、16KB ページ（#1194 AC-3）、署名・versionCode・配布（#1199）。

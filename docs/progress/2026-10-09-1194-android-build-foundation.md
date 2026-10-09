# #1194 Android の build 基盤と実機での起動（2026-10-09）

#1194 の AC-1（PR-A1-1）と AC-2（PR-A1-2、末尾の節）の記録。AC-1 は Scope revision `2026-10-09-r5`、基準 commit `41fdb34f3`（`integration/android-1193` の作成基点）。前提の判断は #1193 の D1・D3・D5・D7（2026-10-09 確定）。

## 採用した toolchain・SDK・ABI

| 項目 | 値 | 置き場所・根拠 |
| --- | --- | --- |
| applicationId・表示名 | `app.kukuri.android`・kukuri | `apps/desktop/src-tauri/tauri.android.conf.json`（D5）。desktop の `app.kukuri.desktop` と保存先は変えない |
| minSdk・targetSdk・compileSdk | 29・36・36 | 同上と `gen/android/app/build.gradle.kts`（D7、Google Play の target API） |
| ABI | 配布は arm64-v8a、検証は x86_64（emulator） | D7。32bit は対象外 |
| Rust | 1.98.1（`rust-toolchain.toml`）。Android の target `aarch64-linux-android`・`x86_64-linux-android` は `rustup target add` で入れる | `docs/runbooks/dev.md` の「Android 前提」 |
| Tauri | crate 2.12.0、CLI 2.12.0 | `src-tauri/Cargo.toml`、`apps/desktop/package.json` |
| Android project | `tauri android init` の出力。Gradle 9.6.1（wrapper）、Android Gradle Plugin 9.3.1、Kotlin Gradle plugin 2.2.10 | `apps/desktop/src-tauri/gen/android` |
| JDK | 17 以上。手元は Android Studio 付属の JBR 21.0.8 | Gradle 9 の要件 |
| SDK の部品 | platform android-36、build-tools 36.0.0（無ければ Android Gradle Plugin が取得する） | 初回の build の log |
| NDK | 手元は r29（29.0.14206865）。CI の check は runner の image の NDK | `cargo xtask android-check` が `ANDROID_NDK_HOME`・`NDK_HOME` から探す |

`tauri android init` の出力から直したもの: CLI の呼び方（init を実行した端末の node の絶対 path を埋め込んでいたため、PATH の node で `apps/desktop/scripts/tauri-cli.mjs` を呼ぶ）、compileSdk・targetSdk（雛形の 37 を 36 へ）、Android TV の宣言（D7 の対象外）、未使用の雛形（layout・色・night の theme）と既定のアイコン。ランチャーアイコンは `src-tauri/icons/android` を Gradle の res として直接読み、権利は既存の `docs/ASSET_MANIFEST.json` のまま管理する。

Tauri の Android library と plugin の Gradle project（`:tauri-android` など）は cargo の registry の中にある。雛形のままではその build の出力も registry の中に書かれ、tauri-plugin の build script（`tauri-plugin` 2.7.0 の `copy_folder`。Android では `build/` を除かずに directory を丸ごと写し、全 file を再実行の条件にする）が Gradle の書込み中に写して、clean な状態からの build が `failed to copy tauri-api to the plugin project` で落ちた。root の `build.gradle.kts` で app 以外の project の出力を `gen/android/build/<project>` へ移した。修正前の build が registry に残した `build/` がある環境でも、Gradle はそこへ書かず build は成功する（古い出力を registry に置いて確認した）。deep-link の plugin が build のたびに manifest へ入れる目印（`DEEP LINK PLUGIN. AUTO-GENERATED`）は、生成と同じ内容で追跡する（内容が同じなら書き換えない）。

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

frontend の画面と API は変えていない（`vite.config.ts` は `tauri android dev` の開発 server の host だけ。desktop では CLI ラッパーが `KUKURI_TAURI_DEV_HOST` を必ず渡すので変わらない）。Tauri の IPC と既存の dispatch・起動の関門（`invoke_gate.rs`）は Android でも同じ経路を通る。

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

未実施（担当）: 16KB ページ（#1194 AC-3）、署名・versionCode・配布（#1199）。

## AC-2: release の成果物と実機での起動

基準 commit `0fe3a7a26`（#1195 AC-1 の merge 後の `integration/android-1193`）。toolchain と lockfile は上の表のとおり。

| 成果物 | command | 結果 |
| --- | --- | --- |
| release の AAB と APK（arm64-v8a、R8 有効、未署名） | `tauri android build --apk --aab --target aarch64` | 成功（約 16.5 分）。AAB 50,911,543 bytes（SHA-256 `bd7aab46725206f8…`）、APK 132,910,455 bytes（`068058286e3a3dc5…`） |
| debug の APK（arm64-v8a） | `tauri android build --debug --apk --target aarch64`（`CARGO_PROFILE_DEV_DEBUG=line-tables-only`） | 成功。485,236,830 bytes |

- APK の native library は `libkukuri_desktop_tauri_lib.so` の 1 つで、依存は OS の library（libandroid・libdl・liblog・libm・libc）だけ。ELF の LOAD segment は 4 つとも 16 KiB 境界（`p_align` 0x4000）で、`zipalign -c -P 16 -v 4` も通る（16 KiB の端末での起動は AC-3）。
- 実機で release を動かすときは、APK に手元の Android SDK の debug 鍵で `apksigner` の署名をした（配布には使わない。upload 鍵での署名は #1199）。

実機（Pixel 8 Pro、Android 17、arm64-v8a）での結果:

- uninstall の後に release を入れた cold start（213 ms）で、同意画面（利用規約 v10・18 歳以上の確認）を表示した。log は `app-level legal consent required; deferring runtime startup` だけで、強制終了の後の起動（167 ms）も同じ。
- 同意と復元の関門は desktop と同じ起動の経路（`lib.rs` の `restore_startup_action`）を通る（復元そのものの確認は #1195 AC-2）。
- 同意前の app data dir（debug を上書きで入れ `run-as` で確認）には、profile の lock と WebView・ProfileInstaller の設定だけがあり、DB・鍵の保存先・account は無い。desktop の command は起動の関門で止まる（`check_app_update` は `requires Ready startup state; current state is ConsentRequired`）。
- 同意前の通信: app の uid の socket（`/proc/net/{tcp,tcp6,udp,udp6}` を端末の中で約 0.1〜0.2 秒ごと）と、uid ごとの通信量の累計（`dumpsys netstats` の `mAppUidStatsMap`、interface・tag 別の履歴）を見た。cold start、強制終了の後の起動、3 分の待機、同意画面の操作（規約のスクロール、言語の選択、18 歳以上のチェック、文字の長押し）、debug と release の上書き install を挟む 7 回の起動のどれでも、app の socket と通信量は 0 だった。
- 対照: 「同意して続行」の約 1.3 秒後に runtime が起動し（`initialized kukuri desktop runtime`）、UDP の bind（iroh）・DNS・443 への TCP・80 への TCP（証明書の失効の確認）が現れ、60 秒で受信 238 KB・送信 154 KB になった。上の観測の方法は runtime の通信を即座に捉える。
- 未特定の観測: 上とは別の 2 回で、同意前に app の uid へ少量の通信が計上された（Wi-Fi で受信 10,061 B・送信 5,194 B、受信 7,745 B・送信 6,225 B）。2 回目は全量が TrafficStats の tag `0x4001804` で、この tag は runtime の通信（tag なし、DNS は `0xffffff82`）にも WebView の画面からの fetch にも付かず、app の code（dex）は tag を設定しない。どちらの時間帯にも app の uid の socket は観測されていない（1 回目は 1 秒ごとの観測。2 回目は観測の外）。他の process（WebView・Google Play 開発者サービス等）が app の代わりに行った通信の計上と見ているが、相手は特定できていない。runtime は起動しておらず、runtime・P2P・CN の通信ではない。同意画面の「同意いただくまで、kukuri はネットワーク接続（IP アドレスを伴う通信）を開始しません」との関係は #1203 AC-1（Android のデータの流れの突合）へ引き継ぐ。
- 同意の後（release、R8 有効）: runtime が起動して community node の案内を表示した。Rust の panic と Java の class の欠落は無く、証明書の検証（rustls-platform-verifier の Kotlin の部品）と Keystore への保存が R8 の後も動く。community node の案内から CN（`https://api.kukuri.app`）の規約 6 件を HTTPS で取得して表示した（同意はしていない）。依存の jni の警告 `Dropping a GlobalRef in a detached thread` が runtime の起動ごとに 1 回出る（失敗ではない）。
- desktop だけの処理: updater の plugin は登録せず、多重起動の制御は依存ごと外している（AC-1）。同意後の debug では、起動状態が `ready` で、`check_app_update` は `update_managed_by_google_play` を返した。終了の横取りは無く、強制終了の後の起動も通常どおり。

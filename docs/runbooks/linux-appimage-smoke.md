# Linux AppImageの生成・実機確認

## 状態と対象

#889の開発・検証手順。生成機能と実機確認は実装中で、配布開始を意味しない。
検証済み範囲は [作業記録](../progress/2026-09-05-issue-889-linux-appimage.md)、署名とデータ分類は [ADR 0049](../adr/0049-linux-gui-cli-control-plane.md) を参照する。公開Releaseへの集約と利用者向け導線は#890が所有する。

- 生成: Ubuntu 22.04、x86_64、repositoryのRust／pnpm／Tauri固定版。
- 実機確認の元の対象: Ubuntu 22.04／Debian 12 × X11／XWayland。2026-09-06のユーザー承認により、追加で用意できないOS環境とXWaylandの今回の実行は延期し、既存Ubuntu 24.04とWindowsで検証する。延期は検証済みの意味ではない。native Wayland-onlyやaarch64 GUIの動作保証は対象外。
- 署名: Tauri updater署名のみ。埋込みGPG署名は使用しない。

### #889の現行検証方針（2026-09-07、v5）

成功済みUbuntu／Windowsの実機証跡を採用し、追加の網羅的なComputer Use検証は行わない。変更した入口・影響先の不足を自動tests、隔離D-Bus、制御時計、必要な実filesystem／verifier／process／保存fixtureで補う。実OS自動検証・mock・実desktop表示の証跡は区別する。未変更の全mic／camera／GPU／codecや全OS連携の手動確認を今回の実行ゲートにしない。同梱依存と配布条件、署名・identity・データ保護、CI・独立監査は引き続き必要。

追加の手動実機は、固定条件に関わる具体的な問題を既存証跡と自動検証では判定できない場合に限定する。対象・自動化で不足する理由・最小操作・終了条件をまとめて提示し、必要なユーザー操作をまとめて依頼する。Computer Useは既定／必須手段ではなく、他の経路で操作権限を迂回しない。以下の操作手順・確認一覧はこの条件で選択する参照表であり、全件の再実行指示ではない。過去の未確認を成功に読み替えず、詳細は作業記録の「現在の残る条件（v5）」を参照する。

## 生成

TauriのLinux開発依存とAppImage生成に必要な `libfuse2`、`patchelf` 等を用意し、frontend依存をlockfileどおりに導入する。ローカルでは実際の依存を `pkg-config` で確認する。CIで導入する一覧は `.github/workflows/kukuri-linux-package.yml` に置く。

```bash
npx pnpm@10.16.1 install --frozen-lockfile --dir apps/desktop
cargo xtask desktop-package
```

`TAURI_SIGNING_PRIVATE_KEY` は既存のTauri鍵fileまたは鍵の内容を環境変数で渡し、必要なら `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` を設定する。引数やログへ秘密値を出さない。`apps/desktop/src-tauri/tauri.conf.json` のupdater公開鍵と一致する鍵が必要で、Linuxでは鍵なしの生成を認めない。

既定の出力先は `apps/desktop/src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage/`。`CARGO_TARGET_DIR` を指定した場合はその下の `x86_64-unknown-linux-gnu/release/bundle/appimage/` になる。

- `kukuri_<version>_amd64.AppImage`
- `kukuri_<version>_amd64.AppImage.sig`
- `appimage-artifacts.json`: version、`linux-x86_64`、本体名、SHA-256、署名file名、署名内容。

xtaskは生成後に本体と署名の存在、および設定した公開鍵との署名一致を確認してから一覧を書く。これだけでは起動・OS連携・更新成功の証拠にならない。

### 非公開のCI生成

`Kukuri Linux Package` はPRでは検証専用の一時鍵を使う。manual dispatchの `signing=distribution` だけが設定済み配布鍵を使用する。どちらもGitHub Releaseを公開せず、検証済み成果物をActions artifactへ保存する。

- `signing-mode.txt` と `source-commit.txt` で用途とsourceを確認する。
- `test-updater-key.pub` はその成果物の検証用公開鍵であり、秘密鍵ではない。`distribution` の場合は配布用公開鍵が入る。
- `test` 成果物を公開配布用や既存利用者への更新として使わない。一時鍵で署名されたAppImage内には検証用公開鍵が設定されている。
- 1 byte改変、別公開鍵、署名欠落の拒否は、実成果物に対する `appimage::tests::signed_appimage_accepts_only_matching_bundle_and_key` で確認する。

## 実機確認の準備

### 同梱runtimeの証跡

ビルド直後の同じUbuntu／Debian hostで、未変更のAppDirを次のcollectorへ渡す。既存の出力directoryは上書きせず拒否する。

```bash
python3 scripts/release/test_appimage_runtime_inventory.py
python3 scripts/release/appimage_runtime_inventory.py \
  apps/desktop/src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage/kukuri.AppDir \
  test-results/kukuri/runtime-evidence
```

GNU build IDがhost上のELFと一致する場合だけdpkg packageを対応付け、version・source package・copyright原文・共通license原文を保存する。`runtime-inventory.json` の `unmatched_elf`、`missing_copyright`、非ELFとsymlinkも確認する。CIは同じ証跡を `runtime-evidence/` としてartifactへ添付する。copyright欠落はcollector失敗となる。

このcollectorの終了成功は配布承認ではない。AppRunなどの未特定ELF、外側のAppImage runtime、静的依存、非ELF asset、source提供条件は別途確認し、公開時のlicense義務を満たすまで未完了とする。自動抽出された `License:` labelだけで許諾条件を判断しない。

#889の有限な照合結果と#890の公開前引渡し条件は [同梱物の証跡](linux-appimage-runtime-evidence.md) に置く。schema／xdg-mimeは内容hashがbuild hostと一致する場合だけpackageを対応付け、copyrightとsource版をartifactへ保存する。

### 配置と操作

1. 担当者が対象commit、AppImageと署名、SHA-256、検証用／配布用の区別、今回確認する操作を提示する。
2. Ubuntu Desktopへのファイル配置・実行権限付与と必要な依存のインストールはユーザーが行う。AppImageはNSIS等のinstallerではなく、実行可能な単体fileである。
3. 手動補完が必要な場合だけ、合意した最小操作をユーザーまたは利用可能な手段で確認する。Computer Useを前提にしない。OS・session等は今回の確認に必要な情報だけ記録し、既存記録を再利用する。
4. 日常利用のprofileを避け、専用の一時profileを明示して実行する。通常のprofileを削除・初期化しない。
5. 修正前の観測が必要な場合は先にbeforeを残し、修正後の同条件と比較する。画像やログへ鍵・招待token・DM本文を含めない。

リモートデスクトップ接続自体はX11／XWaylandや実GPUの使用を証明しない。clipboardの転送、音声／camera転送、logout時の切断等は試験条件として記録する。logout前に再接続方法を確認する。

### 起動時のGIOモジュール警告

`Failed to load module` の直前に `undefined symbol: g_task_set_static_name` が出る場合は、単なる依存package不足とは区別する。初回検証用AppImageでは、同梱GIO 2.72.4とUbuntu 24.04.4側GVfsのABI不整合を観測した。修正版はLinuxの `main` で、GTK／Tauri／スレッド起動前に `GIO_MODULE_DIR` と `GIO_EXTRA_MODULES` を同梱GIOモジュールの場所へ設定する。`APPDIR` がない通常起動とWindowsには適用しない。詳細と修正状況は [作業記録](../progress/2026-09-05-issue-889-linux-appimage.md#ubuntu-desktopでの初回観測とgio警告修正前) に置く。

ホスト側libraryの削除・差替えや追加package導入を一般的な解決手順にしない。AppImageの同梱構成とモジュール探索先を確認する。画面の起動成功とOS連携の正常動作は別に検証する。

同梱モジュールはTauriが配置する `usr/lib/x86_64-linux-gnu/gio/modules/libgiognutls.so` を使用する。これが欠落したAppImageはGTK起動前にerrorとして終了し、ホスト側へ黙って切り替えない。ホストの任意GIO拡張は読み込まないため、GVfsを使うネットワーク共有等の動作をローカルファイル参照の成功から推定しない。

生成後に実際の同梱GIOと初期化処理を使う回帰検査を実行する。生成環境のC compiler、`pkg-config gio-2.0`、Rust compilerを使い、GUI・外部通信・ユーザーprofileは使用しない。

```bash
bash xtask/tests/appimage/gio-isolation.sh \
  apps/desktop/src-tauri/target/x86_64-unknown-linux-gnu/release/bundle/appimage/kukuri.AppDir
```

新しい関数を要求する不整合fixtureで修正前のerrorを確認し、修正後のTLS backend・ローカルファイル参照、通常起動への非適用、同梱TLS欠落時の拒否を確認する。これはUbuntu Desktop上の再検証の代替ではない。

`app-level legal consent required; deferring runtime startup` は同意待ちを示す情報ログであり、このモジュール読込み失敗とは別である。

### 同梱しない表示系library（#1222）

`libEGL`／`libGL`／`libgbm`／`libdrm`はホストのMesaを使う。Mesaと同じ世代で揃える必要がある次の10件は同梱しない: `libwayland-client`／`-cursor`／`-egl`／`-server`、`libxkbcommon`、`libxcb-randr`／`-render`／`-shm`、`libXau`、`libXdmcp`。Ubuntu 22.04の版を同梱したv0.2.8以前は、Fedora 44（Mesa 26）等でWebKitWebProcessが`Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...`で終了し、windowが真っ白になった。

Tauri bundlerにはlinuxdeployの除外設定がなく、GTK pluginが内部で呼ぶlinuxdeployには`--exclude-library`が届かない。Tauriは実行のたびにlinuxdeploy本体の先頭byteを書き換えるため、本体をscriptへ置き換えることもできない。そこで`cargo xtask desktop-package`は、Tauriのtools directory（`~/.cache/tauri`）のAppImage出力plugin `linuxdeploy-plugin-appimage.AppImage`をwrapperにし、Tauriが取得するものと同じpluginを`kukuri-appimage-output.real.AppImage`として呼ぶ。linuxdeployは依存の収集とGTK pluginの後にこのpluginを呼ぶので、wrapperはAppDirから上記libraryを削除して本来のpluginへ引数をそのまま渡す。AppImageの生成と署名は従来どおりTauriの1回のbuild内で完結する。環境変数`KUKURI_APPIMAGE_HOST_LIBRARIES`がない呼出し（同じcacheを使う他のTauri project）では何も削除しない。pluginを取得できない場合、Tauriは古い内蔵版へ切り替えるが、xtaskは同梱runtimeが変わるのを避けてbuildを止める。一覧とwrapperは`xtask/src/linuxdeploy.rs`と`xtask/src/appimage-output-wrapper.sh`に置く。

生成後、xtaskはAppDirと、FUSEを使わず展開したAppImage本体の両方に上記libraryがないことを検査し、混入時は失敗する。

修正前後の起動は、Fedoraのcontainer内でheadlessのWayland session（weston、Xwayland、software rendering）を使って比較できる。これはWebKitWebProcessのEGL初期化の確認であり、実GPUや実desktopでの表示確認の代替ではない。手順と観測は[#1222作業記録](../progress/2026-09-20-issue-1222-appimage-egl-white-screen.md)に置く。

## 確認と証跡

### 終了・更新の実装と確認境界

Linuxではトレイ表示先と当該processの登録をD-Busで確認し、利用できる場合だけ閉じる操作でwindowを隠す。確認失敗・2秒以内に応答しない場合は正常終了へ進む。非表示中に表示先を失うとwindowへ復帰する。これらは新しい実装であり、利用環境ごとの確認結果は作業記録を参照する。

登録先の表現はGNOME系の `bus@path`、KDE系の `bus/path` またはservice名だけの形式を扱う。表示されているトレイでもcloseで終了してしまう場合は、表示先の不在と登録文字列の解釈失敗を区別して確認する。

Quit、SIGTERM／SIGINT／SIGHUP、通常の終了要求、更新後の再起動は共通の終了処理へ進む。起動・復元・切替等の既存の排他処理を待ち、host停止後にprocessを終了する。終了要求後の新操作は拒否するが、進行中バックアップの取消は可能なままにする。

更新の適用中は「更新を適用中」を表示し、重複操作を受け付けない。Linuxのinstall成功後は `restart_after_update` を呼び、host停止後にTauriへ再起動を要求する。Windowsのupdater installer起動処理は変更しない。この呼出し順のtest成功と、実AppImageの旧版→新版更新成功は別に記録する。異なる一時鍵で生成した検証用AppImage同士を更新試験の組として使わない。

取得・検証後は確認buttonがdisableになり、定期checkでも再起動待ちを維持する。「あとで」で案内を閉じても適用buttonを残し、panelの再表示でも検証済み更新を保持することを確認する。asset 404では更新file欠落・未適用を案内し、「確認」からの再試行を可能にする。これらの修正版は再生成した成果物で確認し、旧sourceの実更新成功と分けて記録する。

検証pairを端末内で配信する場合は、分離したbuildコピーだけに試験用version・公開鍵・`127.0.0.1` endpointを設定する。HTTP許可はこのコピーに限り、TLS検証やupdater署名検証を無効化しない。serverはloopbackへだけbindし、固定manifest／asset以外を返さず、試験後に停止する。異常系を正常更新より先に実行し、失敗後の旧version・実行file hash・同じprofileの保持を確認する。今回生成したpairのhash・同梱serverのmode・配置手順は作業記録と成果物のREADMEに記録する。

自動の実置換試験はLinuxの非root userで次を実行する。`KUKURI_UPDATER_BUNDLE`、`KUKURI_UPDATER_SIGNATURE`、`KUKURI_UPDATER_PUBLIC_KEY_FILE`に検証済みAppImage・署名・公開鍵fileを指定する。実updater pluginがloopbackから取得・署名検証し、一時directoryだけで置換不能時の保持と明示retryを検査する。GUI、再起動、実利用profileは使わない。同じcommandはpackage CIにも登録する。

```bash
cargo test --locked --release --target x86_64-unknown-linux-gnu \
  --manifest-path apps/desktop/src-tauri/Cargo.toml --test updater_install -- --ignored
```

### OS通知の利用可否

Linuxの「通知の利用可否を確認」は、session D-Busの通知サービス情報だけを2秒期限で照会する。`available` はサービスへの接続成功であり、OS設定による表示許可や実表示の成功ではない。サービス不在・拒否・応答停止は `unavailable` と表示し、設定確認後に再確認できる。確認だけでOS通知や本文previewは有効化しない。

LinuxのTauri lib testは `dbus-daemon` を使って隔離したサービスを起動し、不在・拒否・応答停止・回復と通知送信0件を検証する。ホストの通知サービス・OS設定は変更しない。実際の表示、静音、クリックによる復帰、OS側の拒否は本人による設定操作を含む実機確認として別に記録する。

### Windows通知クリックの回帰確認

- NSIS版は通常のWindowsユーザー環境でinstallし、Start menuのkukuri shortcutが存在することを確認する。別アプリの隔離環境経由でinstallerを起動した場合は、shortcutやexeが通常ユーザー領域へ配置されたかも確認する。単体exeのコピーや生成成功だけで通知のOS登録を確認済みとはしない。
- 通知用shortcut hookのfile-only契約は `scripts/release/test-windows-notification-shortcut.ps1` で実行する。これはworkspace内の一時shortcutだけを扱い、OSのStart menuやregistryを変更しない。
- 修正版は `kukuri://notification/?id=...` で既存の起動経路へ復帰する。Windowsがhostの後ろへ追加するroot slashを生成側・受信側で扱い、通知IDが失われないことをnative URI／frontend testsでも検査する。通常表示／常駐非表示から、新しく発行した通知をバナーと通知センターでクリックし、同じprofileの通知対象に移動することを確認する。修正前に発行された旧形式のOS通知と混同しない。
- permission表示、OS履歴への保存、バナーの表示、クリックでのウィンドウ再表示、正しい通知対象への移動は別々に記録する。通知本文previewをOFFのまま確認できる。未知の通知IDを別profileへ探しに行かない。
- 投稿通知はOS／アプリ内の共通導線から通知元object_idを指定し、同じThread内で取得後に対象投稿へ一度スクロールする。初期page外の対象、同じ通知の再クリック、通常refreshで位置を奪わないこと、別Columnの縦scrollとdraftの保持を確認する。欠落対象へ別投稿を代入せず、遅れた取得で新しい操作を上書きしない。DM／follow導線や通知の既読契約は変更しない。

### 外部リンク

Releaseの資料／feedbackと通報画面のpolicy／権利侵害受付URLは、明示操作でOSの既定ブラウザーへ渡す。Linuxではsession D-BusのOpenURI portal（`org.freedesktop.portal.Desktop`）とdesktopに対応するbackendが必要。アプリはHTTP(S) URLだけを渡し、AppImageのlibrary環境をブラウザーへ引き継がない。

リンクを押すと処理中を表示し、同じ画面での連打を抑止する。サービス不在・拒否・60秒の期限超過では、起動を確認できなかったことを局所表示する。OSの既定ブラウザー／portalを確認した後、利用者が同じリンクを押して再試行する。自動retryやshellへのfallbackはしない。成功応答はOSへの起動要求の完了であり、リンク先ページの読込み完了を保証しない。

Linuxの境界testは隔離D-BusのOpenURI／Request.Responseを使い、サービス不在・成功・取消・拒否・応答停止・回復を検証する。実ブラウザーが開くことはAppImageで別に確認する。

### 手動補完の候補一覧（全件必須ではない）

| 確認群 | 操作・失敗条件 | 記録する結果 |
| --- | --- | --- |
| 起動・永続性 | cold／warm、同意前／後、second instance、投稿→終了→同profile再起動 | 画面、process数、identity／投稿／購読状態の保持、同意前の保護I/O |
| tray・終了 | tray有・無・生成失敗・途中消失、close、Open／Quit、SIGTERM／logout、起動・切替・復元と終了の競合 | hidden orphanがないこと、停止完了後の終了、同profileの再open |
| desktop連携 | cold／warm deep link、無効URI、登録した起動path、clipboard、external link、download、dialog、fullscreen、select、dark／light | 正しいwindow／対象への到達、失敗・取消時の表示と状態保持 |
| OSサービス・device | notification daemon／Secret Serviceの有・無・拒否、既存keyring鍵、mic／camera正常・不在・拒否、GPU不在／Mesa／NVIDIA | 成功または安全なerror／利用不能表示、別identityを生成しないこと、他機能の継続 |
| library・media | ELF依存、desktop entry／icon／category／protocol、音声・動画再生、GStreamer plugin不在 | 同梱／ホスト依存の区別、library／codec版とlicense、利用不能時の結果 |
| 更新 | 隔離した旧版→新版、正常署名／改変／別鍵／欠落／download・置換失敗 | verify／install／restartの順序、旧版・identity／DB／Iroh／CN設定／private capabilityの保持 |

公開用manifestや既存Releaseは試験で上書きしない。更新試験のendpoint、鍵、旧新版のhashを固定し、直接起動できたことと更新できたことを別に記録する。

全条件の進捗は作業記録に集約する。WSLやbrowser mockの成功を実機成功に置き換えず、未確認のDebian／表示session／device条件を明記する。#889全体の完了には必須CIと独立監査も必要。

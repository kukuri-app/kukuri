# #1222 新しいLinuxでAppImageが真っ白になる不具合の作業記録

2026-09-20 / 区分B / 基準commit `e08743fb` / AC・INVARはIssue #1222本文。

## 原因

v0.2.8以前のAppImageは、build host（Ubuntu 22.04）の`libwayland-client`／`-cursor`／`-egl`／`-server`、`libxkbcommon`、`libxcb-randr`／`-render`／`-shm`、`libXau`、`libXdmcp`を同梱し、`AppRun`の`LD_LIBRARY_PATH`でホスト側より優先していた。`libEGL`等はホストのMesaを使うため、新しいMesaと古い`libwayland-client`の組合せで`eglGetDisplay`が`EGL_BAD_PARAMETER`になり、WebKitWebProcessがabortする。UI processは残るのでwindowは出るが中身が描画されない。上流のtauri-apps/tauri#15976・#15665と同じ現象。

配布済みの`kukuri_0.2.8_amd64.AppImage`（SHA-256 `d1513b41…572e5f`、ReleaseのSHA256SUMSと一致）を展開し、上記10件の同梱を確認した。

## 修正前後の観測

条件: Fedora 44のcontainer（Mesa 26.2.2、libwayland-client 1.26.0、libxkbcommon 1.13.1）、headlessのweston＋Xwayland、software rendering、未同意の空profile。`bash xtask/tests/appimage/egl-startup-fedora.sh <AppImage> <出力先>`で再実行できる。実GPU・実desktopの確認ではない。

| 対象 | stderr | 起動25秒後のWebKitWebProcess | window |
| --- | --- | --- | --- |
| 配布済みv0.2.8 | `Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...`（Issueコメントと同じ） | 0 | 真っ白（[画像](assets/1222/fedora44-before.png)） |
| 配布済みv0.2.8から10件だけ削除 | EGLのerrorなし | 1 | — |
| 配布済みv0.2.8＋`LD_PRELOAD=/usr/lib64/libwayland-client.so.0` | EGLのerrorなし | — | — |
| 修正後のbuild（下記） | EGLのerrorなし | 1 | 同意画面を描画（[画像](assets/1222/fedora44-after.png)） |

## 実装

Tauri bundlerには除外設定がなく、上流の`excludeLibraries`（tauri-apps/tauri#15662）は未merge。試した経路と結果:

- linuxdeployの環境変数による除外: Tauriが固定配布するlinuxdeployは非対応（`--exclude-library`引数のみ）。
- linuxdeploy本体をwrapperに置換: GTK pluginが内部で呼ぶlinuxdeployへ除外が届かず再混入する。加えてTauriが実行のたびに本体のoffset 8–10をzeroで上書きするため、scriptのshebangが壊れて起動できない。
- 採用: AppImage出力plugin（`linuxdeploy-plugin-appimage.AppImage`）をwrapperにする。linuxdeployは依存収集とGTK pluginの後にこれを呼ぶので、AppDirから10件を削除して本来のpluginへ渡す。生成と署名はTauriの1回のbuild内のままで、署名の入口は変わらない（区分Bのまま）。

`cargo xtask desktop-package`はwrapperを配置し、生成後にAppDirとAppImage本体の両方で混入を検査する。仕組みは[AppImage手順](../runbooks/linux-appimage-smoke.md#同梱しない表示系library1222)。

## 検証

ubuntu:22.04のcontainer（Rust 1.92.0、Node 22、CIのpackage jobと同じapt依存、検証用の一時鍵）で実施。

- `cargo test --locked -p xtask --no-default-features`: 成功（wrapperの動作testを含む）。
- `cargo xtask desktop-package`相当: AppImage・Debの生成、updater署名検証、混入検査が成功。混入検査と同じ展開方法で配布済みv0.2.8からは対象libraryが見つかることも確認した（検査の空振りではない）。
- `gio-isolation.sh`: 成功。
- `appimage_runtime_inventory.py`: ELF 175→165件（10件減）、`unmatched_elf`は`AppRun.wrapped`のみ、`missing_copyright`なし。
- 外側runtime: digest領域をzero正規化した先頭944632 bytesのSHA-256が`1cc49bcf…5aebbf`で、配布済みv0.2.8・native complianceの固定値と一致。
- INVAR-1: Ubuntu 24.04.5実機（Mesa 25.2.8、GNOME Wayland sessionのXwayland、隔離profile・独立D-Bus session）で修正後のAppImageを起動。EGLのerrorなし、同梱WebKitWebProcessが生存、同意待ちのlogまで到達。sshからの起動のため画面の目視はしていない。

未実施: 実updater置換試験（`updater_install`）とdistribution署名でのnative source取得は、PRのpackage CIと次回Releaseの配布jobで確認する。Fedora／Arch／Ubuntu 26.04の実機・実GPUでの確認は未実施で、次回Preview Release後に報告者へ確認を依頼する。

## 利用者への案内

該当環境ではv0.2.8以前の画面が出ないためアプリ内更新を使えない。修正を含むReleaseのnotesで手動の再取得を案内する。それまでの暫定回避（`LD_PRELOAD`）は[troubleshooting](../runbooks/mvp-troubleshooting.md)に記載した。

# MVP Troubleshooting

## OSが日本語なのに英語で表示される

初回の規約画面にある「Language」から「日本語」を選べます。通常画面では「Control Center → Settings → Language & theme」の先頭にある言語選択を使ってください。切替だけで規約へ同意することはなく、日本語を選べば同梱の日本語正文を読めます。

保存済みの言語はOS設定より優先します。以前の起動で自動的に英語が保存された場合も含み、OSだけを変更しても既存の言語設定は書き換えません。言語が未保存の場合は、OSのUI言語、WebViewの言語候補、英語の順で決めます。Linuxでは起動元の `LANGUAGE` と、`LC_ALL`／`LC_MESSAGES`／`LANG` の優先順位も確認してください。

「保存できませんでした」と表示された場合、そのsessionの表示は切り替わっていますが、再起動後の保持は保証されません。「言語の保存を再試行」を使ってください。同意保存中は表示言語を固定するため、言語選択と保存の再試行は完了または失敗後に操作できます。

## テキストなど画像・動画以外のファイルを添付できない

投稿・返信・メッセージの添付は画像と動画だけを受け付けます。テキスト、PDF、その他のファイルは添付できません。添付欄の「ファイルを選択」の下に対応形式を常に表示し、非対応のファイルを選んだ場合は「「ファイル名」は添付できません。画像と動画のみ添付できます。」を投稿作成の中に表示します。複数選んだ場合は対応するファイルだけを添付し、残りの件数を理由に含めます。非対応のファイルは読み込まれず、入力済みの本文と添付済みのファイルはそのまま残ります。

Linuxのファイル選択ダイアログは既定で「画像と動画」の絞り込みになっており、対象外のファイルは選べません。「すべてのファイル」へ切り替えると選べますが、その場合もアプリ内で上記の理由が表示されます。動画は選んだ端末で再生できる形式だけが添付でき、プレビュー画像を作れない動画は「動画プレビューの生成に失敗しました」として添付されません。

## Community Node の見方

見つけるの「規約への同意は保存されています」は同意保存の成功を示し、接続や検索の成功を意味しない。以下の操作を使い分ける。

- 「規約を確認する」: 未同意・撤回・規約更新の場合に対象Nodeの文書を読み、明示的に同意する。初回説明で「あとで」を選んだ場合もここから再開できる。
- 「状態を再確認」: 状態/公開Node情報の取得をやり直す。接続失敗の場合は同意済みNodeの接続も再確認する。再試行時刻が示されている間は待つ。
- 招待コードが必要: 「コミュニティノード設定を開く」から対象Nodeの「招待コード」を確認する。参加禁止など、招待コードでは解決しない拒否は別Nodeの選択やそのNodeの参加条件確認が必要。
- 検索・発見機能が非提供: 接続補助は別機能。検索を提供するNodeを設定・選択する。同意だけで提供機能は増えない。
- 明示選択先で検索停止: 検索文を別Nodeへ勝手に送らないための停止。見つけるのNode選択で別Nodeを選ぶか、「自動選択に戻す」。Node情報が取得できない場合も無断で既定Nodeへ切り替えない。

状態の取得失敗やquery失敗を「結果0件」とは扱わない。検索を実行して成功した場合だけ空結果を表示する。

検索成功で0件のときは、検索先Node、検索した範囲（このNodeが索引する公開トピック全体）、照合対象が索引済み投稿の本文であることを表示する。ユーザー名や公開鍵は投稿本文に含まれない限り一致しない。あわせて同じNodeへ索引状況（自分の索引登録申請の状態。トピック内検索では対象トピックが索引対象か）を読み、確定した状態は断定文で、取得中・取得失敗は未確認として区別する（#975）。個々の投稿の反映状況は読めないため「まだ索引に反映されていない」は可能性のまま残し、未確認のときは「トピックがそのNodeの索引対象外」も可能性として示す。再検索、タイムライン、索引登録申請（トピック一覧。申請済み・索引対象が確定した対象には出さない）、接続診断、Community Node設定への導線を置く。公開鍵をそのまま検索した場合はそのユーザーを直接開ける。索引登録申請の画面でも、選択したNodeでの索引状況を開いた時点で表示し、非公開チャンネルの索引対象判定だけは閲覧権限の送信に同意したうえで「索引状況を確認」を押したときに読む。

- `Session Phase`
  - `connecting`: node 到達と session 準備を開始
  - `authenticating`: challenge / verify を実行中
  - `accepting`: ローカルで明示同意済みの required 文書を server の同意記録へ同期中
  - `refreshing`: bootstrap metadata, connectivity URL, seed peer を更新中
  - `ready`: current session で利用可能
  - `retrying`: backoff 中。`Retry After` 以降に再試行する
- `Retry After`
  - 次回の自動再試行予定時刻
- `Connectivity URLs`
  - current session に反映済みの assist URL
- `Last Error`
  - 直近の auth / consent / metadata refresh 失敗理由

## よくある状態

### `ready` まで行かない

- `Last Error` を確認する
- `Retry After` が出ているなら待ってから再確認する
- 一度ウィンドウをフォーカスし直して即時再試行を促す

### `authenticating` と `retrying` を繰り返す

- node の `base URL` が正しいか確認する
- Node の auth endpoint と consent endpoint が有効か確認する

### `accepting` で止まる

- `Last Error` を確認する
- `Consents` を開き、表示された現行版の文書へ明示的に同意済みか確認する

### `restart required` が出る

- まず `Refresh` を試す
- それでも消えない場合だけ app restart を fallback にする

## Manual Actions

- `Authenticate`: ローカル同意済み Node の token を明示的に取り直す
- `Consents`: Node の公開文書を表示し、required 文書への同意・再同意・撤回を行う
- `Refresh`: bootstrap metadata と connectivity assist を再取得する
- `Clear Token`: 該当 node の token を破棄し、次回 auth をやり直す

preview の primary UX は明示同意後のセッション確立・維持を自動処理しますが、上の操作は troubleshooting 用に残しています。

## Updates

- `Settings -> Release -> Check` が失敗する場合は、ネットワーク到達性と GitHub Releases の `latest-preview.json` を確認する。
- `Install` が失敗する場合は、同じ release の updater bundle と `.sig` が揃っているか確認する。
- 署名検証に失敗した更新はインストールしない。release asset の差し替えや誤った signing key を疑う。
- 更新後にデータが消えたように見える場合は、別の Windows user profile、別の app data dir、または keyring fallback の使用有無を確認する。

## Linux配布・CLI

- 公開済みLinux成果物の有無はRelease asset一覧で確認する。AppImage／Debはx86_64、CLIはx86_64／aarch64で、別architectureのbinaryを実行しない。
- Debの認証取消／拒否後は再起動や別認証の自動要求をしない。適用失敗時は`dpkg-query -W -f='${db:Status-Status} ${Version}\n' kukuri`で実状態を確認し、[Deb手順](linux-deb.md)に沿って明示回復する。旧版への自動rollbackは保証しない。
- AppImageの実行権限、FUSE／展開実行、X11の前提は[quickstart](./mvp-user-quickstart.md)を確認する。追加Ubuntu／Debianやnative Wayland-onlyは確認済みとしない。
- AppImageのwindowが真っ白のままで、端末に`Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...`が出る場合は、v0.2.8以前のAppImageが同梱する古い表示系libraryと、新しいMesa（Fedora／Arch Linux／Ubuntu 26.04等）の不整合である（#1222）。修正を含む版のAppImageを取得し直す。画面が出ないためアプリ内の更新は使えない。それまでの暫定回避は、`ldconfig -p | grep libwayland-client.so.0`で得たホスト側のpathを`LD_PRELOAD=<path> ./kukuri_<version>_amd64.AppImage`のように指定して起動する。ホストのlibraryを削除・差替えしない。
- CLIの接続失敗は、同じ`--profile`のdaemonが動いているか、同意状態、XDG runtime directoryを確認する。[foreground例](./linux-cli.md)はsystemdの導入を要求しない。
- timeout／切断で変更結果が不明ならstatusを確認し、変更要求を無条件に再送しない。GUIのdata directoryをCLIへ共有したり、鍵取得失敗時にidentityを削除したりしない。
- 配布担当者: native source取得／hash検証やasset集約が失敗した候補は公開しない。同一候補の再開とCDN確認は[release runbook](./release.md)に従い、既存assetの上書きで修復しない。

## Device Backup / Restore

- `wrong passphrase or corrupted data` の場合は、入力したパスフレーズとファイルの転送完了を確認する。部分ファイルや改変されたファイルは復元しない。
- `unsupported ... version` の場合は、そのバックアップを作成した kukuri と同じか新しい対応版で復元する。未知版を強制的に展開しない。
- 同じ公開鍵のアカウントがある場合、置換確認なしでは復元しない。置換を選ぶ前に現在の状態も別ファイルへバックアップする。
- 容量不足・復号失敗・DB検証失敗・`Installed`までの切替失敗では、現在のアカウントを残してエラーを表示する。再起動時に`Installing`／`Installed`のjournalがあれば旧アカウントへ自動rollbackする。registry commit後の`Committed`／`AwaitingConsent`は新アカウントの再同意待ちへfinish-forwardし、`Activated`後のcleanup中断は次回起動で完了する。
- 復元後に規約同意と年齢申告が再表示されるのは仕様である。Community Node の文書同意と認証、成人向け表示設定も端末側で再設定する。
- 復元途中でアプリを終了した場合は、次回起動時に未完了の復元を先に回収する。回収に失敗した場合はruntimeを開始せず起動失敗として表示するため、同じbackupから再試行する前に表示されたerrorを確認する。復元したfrontend設定はactivation完了後にだけ適用し、適用または確認応答に失敗した場合は旧設定へ戻して次回起動で再試行する。
- 端末バックアップは旧端末、relay、接続相手、P2Pネットワーク上のコピー削除を行わない。

## Diagnostics

- `設定 -> 開発者` で開発者モードを有効にすると、接続・ディスカバリー・コミュニティノードの診断と「診断レポートを開く」が同じ画面に出る。診断レポートは `設定 -> リリース -> 診断` にあり、開発者モードOFFでは表示されない。
- `レポートをコピー` で GitHub issue に貼れる診断レポートを作る。
- 既定のレポートには secret key、auth token、private channel secret、invite/share token、DM 本文、ローカル DB path を含めない。
- `書き出し` は `kukuri-diagnostics.txt` を作成する。

## ログの確認

- 開発者モードON時は `設定 -> 開発者 -> ログ` で直近のログを確認できる(#978)。Tauri backend は `tracing` の出力を標準出力へ書くのと同じ内容を、プロセス内のメモリ(最新 2,000 件 / 1 MiB、1 行 4 KiB)に保持し、上限を超えると古い行から消える。ファイルへは保存せず、開発者モードOFFでは表示も取得もしない。
- 「ログを更新」で再取得する(自動更新はしない)。「ログをコピー」「ログを書き出す」(`kukuri-logs.txt`)は利用者操作でだけ動き、自動送信はしない。書き出し先は保存ダイアログで選ぶ。
- ログには peer / topic / Node URL などの識別子が含まれる。共有する前に不要な行を除く。秘密鍵、token、DM 本文、パスフレーズはログへ出力しない契約であり、それらが見えた場合は不具合として報告する。
- ターミナルから起動した場合は同じログが標準出力にも出る。出力レベルは `RUST_LOG` で変える(アプリ内の保持も同じ filter に従う)。既定値と noise 抑制対象は [dev runbook](./dev.md) の `RUST_LOG` の記述を参照する。

```bash
RUST_LOG=info ./kukuri_*.AppImage 2>&1 | tee kukuri.log
```

- Windows の release build はコンソールを表示しない(`windows_subsystem = "windows"`)ため、標準出力のログは取得できない。アプリ内のログビューアから書き出す。
- 起動に失敗して設定画面へ到達できない場合は、アプリ内ログを読めない。ターミナル起動の標準出力(Linux)と再現手順を添えて報告する。

## 通知の受信設定

- `設定 -> 通知` で OS 通知の有効化、ダイレクトメッセージ、メンションと返信、フォローとリポスト、静音モード、本文プレビューを切り替える。通知カラムのヘッダーと空状態からも同じ設定へ移動できる。
- アプリ内の通知一覧はこの設定に関係なく届く。OS 通知だけを止めたい場合は「OS 通知を有効にする」を外すか静音モードを使う。
- 「通知の利用可否を確認」は端末の通知サービスへの接続だけを確認する。Linux の `利用可` は表示許可を保証せず、表示は OS 側の設定に従う。

## Data Safety

- `Settings -> Release` includes the release runbook and third-party notices.
- Reinstall or migration failures should be reported with diagnostics and must not silently clear local data.
- If state appears missing after update, confirm the Windows user profile, app data directory, and keyring fallback path before resetting anything.

## Installer Notes

- 初回 preview で Windows code signing が未設定の場合、SmartScreen warning は想定内として release note に明記する。
- 未署名 preview の場合も、updater bundle の Tauri signature は必須とする。

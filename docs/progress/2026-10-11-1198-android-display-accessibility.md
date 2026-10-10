# #1198 AC-3 Android の表示・読み上げ

Scope revision `2026-10-11-r9`、区分B。元のAC-3を共有表示AC-3a（main、PR #1762）とAndroid端末・読み上げAC-3b（統合branch）へ分けるユーザー判断を反映する。[固定計画](https://github.com/kukuri-app/kukuri/issues/1198#issuecomment-6102568765)。D7、元の機能・期待結果、emulator先行、実機batch後の復元、添付容量上限なしを維持する。独立監査は計画どおり追加しない。

## 変更と修正前の観測

- `MainActivity`の既存inset処理を、IMEだけでなくsystem bars・display cutoutの和集合へ接続する。各辺は該当insetの最大値を使う。旧candidateではheaderが画面上端から0.76 CSS px、status barの後ろに重なった。新候補はcontentをstatus/navigation barの外へ収め、API36の通常viewportは914から866 CSS pxになる。
- OSの`fontScale`を既存WebViewの`textZoom`へ反映する。起動時と`onConfigurationChanged`だけで更新し、Manifestの既存configChangesへfontScaleを足す。旧candidateはOS font_scale=2.0でも文字が拡大されなかった。新候補は16pxの操作文字が32pxになり、同じdocument・URLを保持する。
- API29では、IMEが実際に出てもviewport842pxのままで入力画面を覆う不足を再現した。Manifestで`adjustResize`を明示すると560pxへ縮む。API36の既存IMEと戻るの契約も維持する。
- 共有側のClear Token文字84.9px/ボタン35.9px、拡大nav623px/本文94pxはAC-3aが所有する。mainの共有修正を前提にし、Android側に別の設定UI・文字縮小・機能制限を作らない。

## 固定候補と環境

- 実装・画面のsource: `ed34b24a8df72b2982612e16d3b95e389c745865`。共有PR #1762の最終candidate `d6ad0222b`を含む。正式main-followの祖先関係は別に照合する。
- debug x86_64 APK SHA-256: `ceb743993c7a0b0bac2b3df547dce128b348a79dbbfed19a4ffe0b1d108cee6a`。配布候補ではない。Kotlin/ManifestとTauriの実buildは成功。
- task専用API36.1 emulator `Kukuri_Links_API36_1`、API29 emulator `Kukuri_UI_API29`。両者は1080×2400/density420、Android WebView134.0.6998.135。API29標準imageの古いWebView74はES構文/CDPが現行frontendの確認に足りないため、GoogleのSDK imageから同じ署名のWebView134とTrichrome library（minSdk29）を入れた。個人のGoogleアカウントは使わない。
- Pixelは未使用。接続時の未インストール状態へ復元済みの実機を保持した。tablet/foldは共通画面の寸法確認で、専用UI・hinge・実ハードウェアの確認を行ったとは扱わない。

## 実WebViewの表示結果

| 固定条件 | 実viewport | 結果 |
| --- | --- | --- |
| API36 phone最小幅、dark、標準文字 | 320×866 | nav390px・本文355px。操作名の全文がボタン内、document横overflowなし。navの末端と本文へ既存scrollで到達 |
| API36 phone、light、OS font_scale=2.0 | 411×866 | nav390px・本文327px。Clear Token文字169.8px/ボタン271.0px。navと本文の全文包含、scroll到達、横overflowなし |
| API36 tablet相当、light | 800×866 | 共通2列の設定。全文包含・横overflowなし・本文scroll |
| API36 fold相当、dark | 673×858 | 共通mobile設定。全文包含・横overflowなし・本文scroll |
| API36 phone横向き、dark | 914×363 | nav/本文250px。双方がscrollでき、全文包含・横overflowなし |
| API29 phone最小幅、dark、標準文字 | 320×842 | nav379px・本文342px。全文包含、scroll到達、横overflowなし |

各候補のCSS名と実画面を照合し、`index-CRBnIfWj.css`の最終共有45dvh/全文折返しを確認した。旧candidateのラベル包含だけの成功を、navによる内容圧迫の解消へ換算しない。共有browserの3言語9contractはAC-3aの証跡を再利用する。

IMEは最終APKで実タップから確認した。API29はviewport560px、textarea178..298px/送信492..524px。API36の日本語QWERTYは554px、textarea172..292px/送信486..518px。フォームの既存scroll後に両方を可視範囲へ収め、確認中の追加送信は0。API36は繰り返す寸法/回転後にGboardが透明のinset0のIMEを出す状態があったため、task emulatorをデータ保持で再起動し、同じ製品codeで通常のキーボード表示と上記結果を確定した。

## 実TalkBack

API36のTalkBack16.0.0.777931756とGoogle TTSのoffline en-US voiceを使用した。synthetic accountだけのtask emulatorで発話のdebug出力を記録し、実touch探索・double-tap、TTSの受付0とutterance完了trueを照合した。単なるAX treeの検査ではない。

- 「Open Control Center · Connection needs attention, button」→double-tapでControl Center。
- 「Settings, button, Language and theme」→設定、「Language & theme, button」→対象section。
- 「Light, radio button」「Double-tap to toggle」→実際のlight選択、Close settingsの名前/役割→閉じる。
- 「Post to Public · test, button」→投稿欄、「Edit box, Write a post」→入力、「Post, button」→明示送信。test topicで1投稿を表示し、composerを終了。Enter/戻るだけでは送信せず下書きを保持した。
- 最終APKで「Go to Column 5 of 9, button」→Notificationsとcanonical URLへ移動。
- 最終APKのfont_scale=2.0で「Language & theme, button」→appearance、現在位置を確認。実際の28pxへの更新完了後もdocument marker/URLは同じ。

最初のSettings探索はControl Centerの可視範囲外のtargetを使ったdriver観測で、成功扱いしない。既存scrollでtargetを表示してから名前・発話完了・double-tapの遷移を確定した。native tapはWebViewのscreenY=63pxを含める。fixtureの初回案内は準備として保留し、本人の同意を要する実機やPlay Consoleの操作を代行したとは扱わない。

共通の操作名/semantic/callbackは変更していない。最初のTalkBack6操作はsource `eba3118de` / APK `139b2b11...`、最終2操作と表示/IMEは上記最終候補。変更したnavのscroll/折返しは最終候補で再確認し、未変更の送信・設定callbackの成功を再利用した。既存AC-1 SNS導線、AC-2 S1〜S12/R8は変更影響に対応する証跡を再利用する。

## 終了判定

共有main #1762は実CI21success/3skip・merge後の全tree一致で完了。正式main-follow、AC-3bの実PR CI・merge後担当内容一致はPR側で記録する。未実施のPlay署名・公開、Android専用成果のmain最終統合を完了とは扱わない。

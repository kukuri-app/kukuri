# #1198 AC-2 Android の入力・戻る・回転（2026-10-10）

#1198 の AC-2b（PR-A5-2b）の記録。Scope revision `2026-10-11-r8`、基準 integration `03c359f4f4033a14f4211cc3afd1f64bef150bde`。共有設定履歴 AC-2a（#1734 candidate678e617a）とカメラ AC-5（#1759 candidatea4a0fb5）を前提として取り込み済み。正式merge・main-followは別判定。前提の判断は 2026-10-10 のユーザー判断（#1198 の Current status の「AC-2 の判断」）。

## 変更

| 対象 | 変更前（`8a283dc1a`） | 変更後 |
| --- | --- | --- |
| キーボード表示中の画面 | 画面が縮まず（WebView の高さは 914 CSS px のまま）、キーボードが投稿欄の大半と送信ボタンを覆った | IME insetで画面をキーボードの上までに縮める（旧候補で914→578 CSS pxの観測、今回候補も914→578 CSS pxで確認） |
| メニュー・ダイアログ・コントロールセンターを開いて戻る | 開いたまま、下の画面の履歴が戻った（最初の画面ではアプリが終了した） | 開いているものだけを閉じ、下の画面は変わらない |
| 投稿欄を開いて戻る | 投稿欄を開いたまま前の画面へ戻った | 投稿欄を閉じる（下書きは残る） |
| 前回と別の項目で設定を開いて戻る | 前回の項目へ移り、もう一度の戻るで閉じた | 1 回で閉じ、開く前の画面へ戻る（main の PR #1734 で直し、取り込んだ） |
| 最初の画面で戻る | アプリが終了した（下書きと画面の位置は保存から戻る） | 終了せず背景へ移り、開き直すと同じ状態から続く |

- `apps/desktop/src/lib/androidBackButton.ts`: Tauri の `onBackButtonPress`（Android の戻るボタンと端からのスワイプ）で受け、focus のある要素へ Escape を送る。メニュー・ダイアログ・コントロールセンター・投稿欄は、desktop の Esc と同じ処理で 1 つ閉じる。閉じるものが無ければ `history.back()`、履歴が無ければ、起動時に復元した設定・スレッド・著者を閉じ、それも無ければアプリを背景へ移す。登録は `main.tsx` で、Android の build（`IS_ANDROID`）だけ。
- `apps/desktop/src/shell/useDesktopShellRouting.ts`: 戻るが送った Escape では、履歴に積んだ設定・スレッド・著者を replace で閉じない（replace で閉じると、戻る先に同じ画面の履歴が残り、次の戻るが空振りする）。
- `MainActivity.kt`: キーボード（IME）の inset の分だけ content の下に padding を置く（edge-to-edge では OS が画面を縮めない）。最初の画面で背景へ移す橋渡し `kukuriAndroid.moveTaskToBack()` を WebView へ渡し、release の最適化で消えないよう `proguard-rules.pro` に keep を置いた。
- キーボードの表示中の戻るは、OS がキーボードを閉じるだけで、frontend へは届かない（変更前から同じ）。

## 確認の条件

task専用 emulator Kukuri_Links_API36_1（API36、x86_64、1080x2400/density420、RAM4GB）でdebugとR8 releaseを確認する。実機は使わない。日本語IME・ボタン/端からのswipe・回転/scrollは以下のS1〜S12の元の期待結果を維持する。

## 固定シナリオ

S1〜S12の操作・期待結果の正本は #1198 の固定表。今回候補のdebug S1〜S12はすべてPASS。日本語QWERTYの実composition/変換/Enterは確定だけで送信0、明示Post後に署名付き相手fixtureで日本語投稿1件を確認。viewport914→578 CSS px、フォーム31pxの実scrollでtextarea/submitを可視範囲に表示。S2/3はIMEだけ→composerの順に閉じてdraft保持、S4/5/6/7はmenu/report/thread/Control Center/settingsを1回1層、settings内部切替はreplaceのまま。S8/9は最初の画面のbutton/edge swipeでPIDを保持してLauncherへ、S10回転でPID/document marker/draft/column保持、S11は文字選択・縦scroll・Messages→Notificationsの意図した隣列移動とcanonical URLの一致、追加送信0。S12は保存済みthreadのprocess death後復元→thread閉じ→同じPIDで背景へ。R8 releaseも最初の戻るでLauncherへ移り、PID17145を保持して復帰した。

## targeted test

現行mainを含むcandidateで shellChrome19test、Android8test（既存feature gateと戻る/履歴/背景化、focusの無いcomposer追加）を合わせ27test PASS。追加composer contractは修正前831e6468の同helperでFAIL（textareaが残る）→既存f382の修正後PASS。実機/OS試験の成功とは区別する。

- WebView document再読込の登録回収: lockedTauriのPlugin.ktはeventごとにlistenerを追加し、removeListenerで解除する。新しい戻るlistenerの所有をdocumentに揃え、pagehideでSDKunregisterを1回呼ぶ。2documentの登録/終了のcontractは修正前に残数1でFAIL、修正後は各終了0でPASS（1test）。副作用や戻る操作は増やさない。既存shell27testは同じ戻るhandlerの証拠として再利用する。

## 固定候補と補足

- 最終実装head802f501c83134dbd2983a792fe884b0fc2860f36は、APK/test source51ef85a9と全tree86a21d10ce2cfc86ba96714d260677c9ca82acfb一致。正式main-follow#1760（統合44679e017）を祖先として保持。debug APK SHA256 b1269f07c33216a92ae6ba9242719917b9d96e5e7970231625d6c22e9a98b476、R8最適化/検証用debug署名 APK ff04bf6e5ef97e66b2b4ef8fd0b02beedbd99f957dcae4ecea3bde2e391cc900。配布用の署名ではない。
- 初回の試験driverはscrollportでclipされたsubmitの矩形中心をtapして送信0、フォームの実scroll後の可視中心でclick/相手1件受信を確認した。製品変更なし。S12の初期起動中の観測は保存完了/初回案内を保留してから同条件で確定した。S11でURL不変を要求したassertは、DESIGNのcanonical URLがfocus列を投影する契約に照合し、実移動先とURL一致で判定した。未実行を成功へ置き換えず元観測も保持する。
- 区分B、既存計画どおり独立監査は追加しない。最終実装headの実CIは22success/3skip。今回の追記は観測結果の文書同期だけで、製品/全treeの変化は文書のみ。merge後に担当pathと前提内容を照合する。
# #1198 AC-2 Android の入力・戻る・回転（2026-10-10）

#1198 の AC-2b（PR-A5-2b）の記録。Scope revision `2026-10-11-r8`、基準 integration `03c359f4f4033a14f4211cc3afd1f64bef150bde`。共有設定履歴 AC-2a（#1734 candidate678e617a）とカメラ AC-5（#1759 candidatea4a0fb5）を前提として取り込み済み。正式merge・main-followは別判定。前提の判断は 2026-10-10 のユーザー判断（#1198 の Current status の「AC-2 の判断」）。

## 変更

| 対象 | 変更前（`8a283dc1a`） | 変更後 |
| --- | --- | --- |
| キーボード表示中の画面 | 画面が縮まず（WebView の高さは 914 CSS px のまま）、キーボードが投稿欄の大半と送信ボタンを覆った | IME insetで画面をキーボードの上までに縮める（旧候補で914→578 CSS pxの観測、今回候補は未観測） |
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

S1〜S12の操作・期待結果の正本は #1198 の固定表。今回候補のdebug/releaseの観測は未実施で、実行後に結果と証拠を記録する。

## targeted test

現行mainを含むcandidateで shellChrome19test、Android8test（既存feature gateと戻る/履歴/背景化、focusの無いcomposer追加）を合わせ27test PASS。追加composer contractは修正前831e6468の同helperでFAIL（textareaが残る）→既存f382の修正後PASS。実機/OS試験の成功とは区別する。

- WebView document再読込の登録回収: lockedTauriのPlugin.ktはeventごとにlistenerを追加し、removeListenerで解除する。新しい戻るlistenerの所有をdocumentに揃え、pagehideでSDKunregisterを1回呼ぶ。2documentの登録/終了のcontractは修正前に残数1でFAIL、修正後は各終了0でPASS（1test）。副作用や戻る操作は増やさない。既存shell27testは同じ戻るhandlerの証拠として再利用する。

# #1198 AC-2 Android の入力・戻る・回転（2026-10-10）

#1198 の AC-2（PR-A5-2）の記録。Scope revision `2026-10-10-r7`、基準 commit `{{BASE}}`（main の取り込み後の `integration/android-1193`）。前提の判断は 2026-10-10 のユーザー判断（#1198 の Current status の「AC-2 の判断」）。

## 変更

| 対象 | 変更前（`8a283dc1a`） | 変更後 |
| --- | --- | --- |
| キーボード表示中の画面 | 画面が縮まず（WebView の高さは 914 CSS px のまま）、キーボードが投稿欄の大半と送信ボタンを覆った | 画面をキーボードの上までに縮め（914 → 578 CSS px）、入力欄と送信ボタンが見える |
| メニュー・ダイアログ・コントロールセンターを開いて戻る | 開いたまま、下の画面の履歴が戻った（最初の画面ではアプリが終了した） | 開いているものだけを閉じ、下の画面は変わらない |
| 投稿欄を開いて戻る | 投稿欄を開いたまま前の画面へ戻った | 投稿欄を閉じる（下書きは残る） |
| 前回と別の項目で設定を開いて戻る | 前回の項目へ移り、もう一度の戻るで閉じた | 1 回で閉じ、開く前の画面へ戻る（main の PR #1734 で直し、取り込んだ） |
| 最初の画面で戻る | アプリが終了した（下書きと画面の位置は保存から戻る） | 終了せず背景へ移り、開き直すと同じ状態から続く |

- `apps/desktop/src/lib/androidBackButton.ts`: Tauri の `onBackButtonPress`（Android の戻るボタンと端からのスワイプ）で受け、focus のある要素へ Escape を送る。メニュー・ダイアログ・コントロールセンター・投稿欄は、desktop の Esc と同じ処理で 1 つ閉じる。閉じるものが無ければ `history.back()`、履歴が無ければ、起動時に復元した設定・スレッド・著者を閉じ、それも無ければアプリを背景へ移す。登録は `main.tsx` で、Android の build（`IS_ANDROID`）だけ。
- `apps/desktop/src/shell/useDesktopShellRouting.ts`: 戻るが送った Escape では、履歴に積んだ設定・スレッド・著者を replace で閉じない（replace で閉じると、戻る先に同じ画面の履歴が残り、次の戻るが空振りする）。
- `MainActivity.kt`: キーボード（IME）の inset の分だけ content の下に padding を置く（edge-to-edge では OS が画面を縮めない）。最初の画面で背景へ移す橋渡し `kukuriAndroid.moveTaskToBack()` を WebView へ渡し、release の最適化で消えないよう `proguard-rules.pro` に keep を置いた。
- キーボードの表示中の戻るは、OS がキーボードを閉じるだけで、frontend へは届かない（変更前から同じ）。

## 確認の条件

{{CONDITIONS}}

## 固定シナリオ

{{SCENARIOS}}

## targeted test

{{TESTS}}

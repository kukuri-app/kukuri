# #1887: 公開スレッドの会話順と操作の切り分け

基準は `dea966f6866d9b3978a5a9db1add7f1e3e8e5d5e`。Scope revision 1、区分B。依頼された公開スレッド詳細だけを変更する。

| 条件 | 期待結果 | 確認 |
| --- | --- | --- |
| K-1 | 取得済みの窓を古い順に表示し、新着は下、古いページは上へつなぐ | 同時刻のcursor、新着・古いページの回帰試験、変更前後の画面 |
| K-2 | 詳細のフィルター・ブックマーク表示切替と投稿のブックマーク操作を除く。旧表示設定で投稿を隠さない | 保存レイアウトの復元試験、画面の操作一覧 |
| K-3 | 投稿・返信・Live・コピー、通常タイムラインの降順、取得窓とcursorを保つ | 関連試験、ブラウザ操作、共通処理の利用側確認 |

公開スレッド用Feedを明示し、既存の行・操作・スクロール・ページ取得を共用した。取得済みの上限付き配列だけを反転する。保存する列の種類、API、ブックマーク記録は変えない。通常の返信ツリーと公開スレッド一覧の並び順は対象外。DESIGNとADR 0068へ同じ方針を記録した。

## 実装Runと後続の検証

koujou-sanのRun専用worktreeでCodexが実装した。原レポートは、型検査成功・依存不足でVitest未実行のためINCONCLUSIVE。依存による起動失敗を挙動の再現とは扱わない。原レポートは変更していない。

後続で依存をリポジトリ外の専用node_modulesへ固定し、次を確認した。通常checkoutとlockfileは変更していない。

- 基準commitへ新しい回帰試験だけを適用: 同じ13試験中5件FAIL。候補では13件PASS。
- 共通Feedの空・欠落・行・overscroll、scroll anchor、merge、通常timeline filter/view: 64件PASS。
- channelと公開スレッドLive: 15件PASS。
- `pnpm run typecheck`、変更したTS/TSXのESLint、Storybook build: PASS。
- Chromeの実画面: 通常幅の英語と390×844の日本語で変更前後を比較。返信をpointerで開き、Ctrl+Enterで送信。公開スレッドのリンクをコピーし、channelを保持したrouteを確認。

試験ログはkoujou-san側の `test-results/issue33-kukuri-{before3,after4,common,channels,typecheck,lint2}.log`。公開の画面証跡と未確認事項は[UI記録](../ui-reviews/2026-10-19-public-thread-order.md)へまとめる。固定commitの独立検品と実際のPR CIを確認するまで完了扱いにしない。

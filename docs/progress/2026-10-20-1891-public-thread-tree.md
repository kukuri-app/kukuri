# #1891: 公開スレッド詳細の返信ツリーと列内の返信

基準は `92299fb938106e6a0c506128d999b176f7e336c3`。Scope revision 1、区分B。完了範囲（AC-1・INVAR・対象外）は[Issueのコメント](https://github.com/kukuri-app/kukuri/issues/1891#issuecomment-6287158180)。

| 項目 | 結果 | 確認 |
| --- | --- | --- |
| AC-1.1 ツリー | 窓の中に親がある返信を親の下へ、最上位と兄弟は古い順 | Vitestの`data-depth`と順序、変更前後の画面 |
| AC-1.2 窓に無い親 | 最上位に置き、返信先の簡略表示を残す。ツリーの中の返信には出さない | 同上 |
| AC-1.3 列内の返信 | 「返信」は詳細の入力欄を返信先つきで開き、`reply_to`をその投稿にして同じスレッドへ送る。列の並び・親は不変 | Vitestの`createPost`引数・列のID・route、ブラウザ操作 |
| AC-1.4 分岐なし | 本文・簡略表示を押しても返信の列を開かない | Vitest |

実装: 公開スレッドのFeedは`buildThreadTree`で窓をツリーにし、返信の列と同じ行の描画（`ThreadTreeItem`、`ThreadTree`から切り出し）を使う。行のviewは投稿ごとに使い回す。返信は投稿の入力欄の下書きに返信先を置き、送信は返信先があれば`reply_to`に使う。入力欄の見出しは返信先がある間「返信」と出す。API・Rust・保存形式・取得の件数は変えていない。

修正前の再現: 基準commitの実装へ変更後のテストだけを当てると、`DesktopShellPage.publicThreads.test.tsx`の9件中2件がFAIL（詳細の入力欄に返信欄が無い、行に深さが無く返信が平ら）。#1873の復元テストは、返信の列を詳細の「返信」ではなくrouteから開くよう書き換えた（同じ復元を確認する）。

検証（ローカル）:
- `src/components/core`・`src/components/shell`の全試験、`DesktopShellPage.publicThreads`・`columnScope`・`liveStage`、`useDesktopShellActions*`: 58ファイル441件 PASS
- `DesktopShellPage.notifications`・`routing`・`LiveStage`: PASS
- `pnpm run lint`（全体）、`pnpm run typecheck`: PASS
- Storybookの変更前後の画面とブラウザ操作は[UI記録](../ui-reviews/2026-10-20-public-thread-tree.md)。

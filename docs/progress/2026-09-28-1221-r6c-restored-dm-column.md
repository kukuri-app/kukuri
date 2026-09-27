# #1221 R6-C: 復元した DM の列を読み込む

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R6-C と、2026-09-28 のユーザー決定「R6-A 再確認の所見4の補修」。
R6-B の再確認(Linux の実アプリ)で、再起動で復元された、選ばれていない DM の列が「不明なユーザー」
「メッセージはまだありません」のまま、列を選ぶまで読み込まれなかった。#1221 の回帰ではない
(表示中の非 active の列を読む処理 `visibleListLoads` は #1349 で入り、DM の列は対象外だった。それより前も DM の列は選んだときだけ読んでいた)。

## 受入条件

| ID | 対象・期待結果 | 判定(試験) |
| --- | --- | --- |
| AC-1 | 表示中の非 active の DM の列は、選ばなくても相手ごとに 1 回、会話・メッセージの窓(100 件)・状態を読む(列の更新操作と同じ読込み) | frontend `a restored inactive conversation column loads its peer and messages`(修正前は失敗) |

## 実装

- `refreshConversationColumn`(列の更新操作の読込み)を `DesktopShellPage.tsx` から section loader
  (`useDesktopShellSectionLoaders.ts`)へ移し、`useDesktopShellData.ts` の `visibleListLoads` で、メッセージをまだ読んでいない
  DM の列に使う。列の更新ボタンと消去の後の読み直しは同じ関数を使う。
- `useDesktopShellData.ts` は 1110 行から 1117 行になった(分岐と受け渡しだけ)。oversized の baseline を更新した。

## 検証

- 修正前の再現: AC-1 の試験は、修正前はメッセージが出ずに失敗する。
- ローカル: frontend の typecheck・変更 file の eslint・`vitest run src/shell`、`cargo xtask oversized-files`。
- 全体は PR の CI。実機の確認は R6-A の再確認で行う。

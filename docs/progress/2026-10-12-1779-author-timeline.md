# #1779: 著者タイムラインの正本統合

## 対象と成果

- 条件の正本: [#1779](https://github.com/kukuri-app/kukuri/issues/1779)、親 [#1777](https://github.com/kukuri-app/kukuri/issues/1777)。`AC-PROFILE / PR-PROFILE`、Scope revision `2026-10-12-refactor-r1`、区分B。
- 比較起点: `1f77dd4e3252b37fc944446544b901435c04063f`。
- 著者の行・cursor・追加取得loadingを既存の著者別mapへ一本化した。`selectedAuthorTimeline`、`selectedAuthorTimelineNextCursor`、`selectedAuthorTimelineLoadingMore`のstate、setter、同期、fallbackを撤去した。
- 追加取得/refreshのrequest id・head cursor・error、プロフィール情報、保存形式、API、CSS、8窓/200行と継続cursor保持は変更しない。

表示は既存のColumnのpubkeyと著者mapから作る。advisory/mediaと既知投稿の参照は、`selectSelectedAuthorPosts`から同じmapの行を読む。これは参照の導出で、別のrow cacheや更新ownerを持たない。空値を安定させ、store購読で毎回新しい空配列を作らない。

著者view modelを一度投稿へ戻して再変換するfallbackと、そのためだけの選択著者view modelを削除した。選択著者の行を取得対象に含める既存のadvisory/media経路は、導出した参照を使って維持する。

## 維持する挙動・変更前の確認

既存の著者mapがある場合、selected-onlyのreaction/optimistic更新は`boundVisibleLists`でmapの値へ戻される。変更前に既存reaction testへ著者mapを置き、その行が変わらないことを確認した。今回その書込みをmap更新へ置換せず、冗長な書込みだけを削除した。不具合修正や新しい表示結果を混ぜていない。

test fixtureの行も著者mapへ置き、著者隔離・選択解除・private投稿が公開著者feedへ入らない条件のassertionを維持した。プロフィール情報やerror stateの統合、cursor/request台帳の回収、social一覧のページ化は行わない。

## 検証

Windowsの同一worktree、pnpm 10.16.1で確認した。最初は共有node_modulesに依存欠落があり6workerが開始できなかった（test実行0）。junctionを外し、このworktreeへfrozen lockfileで依存をinstallした。製品/lockfileの変更はない。

| 対象 | 変更前 | 変更後 |
| --- | --- | --- |
| section loader、pagination model、store、actions、routing、mediaの6 Vitest file | 72 PASS。map fixture/ignored-write確認後も72 PASS | 同6fileに変更consumerのview model testを加え7file/80 PASS |
| `pnpm typecheck` | PASS | PASS |
| 変更した17 TS/TSX fileのESLint（warning許容0） | 既存CIを参照 | PASS |
| 下記Chromiumの既存操作2本 | 2 PASS | 2 PASS |
| `git diff --check` | clean | PASS |

Vitestの既存THREE CJS deprecation、browser buildの既存chunk-size/NO_COLOR warningは前後共通。ESLintの初回commandはPowerShellの配列引数が1patternとして渡り、検査開始前に失敗した。NodeのESLint entryへ引数を個別に渡して再実行した結果が上表である。

### 表示・操作の確認範囲

内部state正本の整理であり、layout/navigation/design languageの変更はない。対象利用者は著者の投稿を読む利用者、操作は著者Columnの閲覧・追加取得・refreshと親の文脈への戻り。draft/focus/scroll/scopeを変える処理を追加していない。

同条件のbrowser mockをWindows/Chromium・1400×980・EN・既定darkで、変更前後にfresh buildして次の既存testを実行した。

- `browser mock shell can switch topics, publish, open thread, open author, and update discovery from settings`
- `browser mock shell can open an author from messages without leaving the dm workspace`

実pointerによる投稿→Thread→Author、DM→Author→Close→DM、URL/親Columnの復帰を維持した。著者A/Bのpagination隔離、遅延pageとhead refresh、8窓/200行、画面外private Column、cursor保持は既存Vitestで確認した。OS固有処理、Tauri実機、実peer、全面Accessibility適合や画面全体のpixel一致はこの確認結果に含めない。表示/CSSに差分がないため新しい視覚baseline・Story・DESIGN改訂は作らず、既存のLinux UI/visual CIを確認する。

## コード量・終了

変更した同じ17 TS/TSX pathの物理行を、比較起点と完成sourceで集計した。実装9,938行等の部分集計と混同せず、testを含む合計は **9,938→9,882（56行減）**。内訳は実装62行減、既存test6行増、新しい補助実装fileは0。本文の証跡doc追加は別計測する。

旧3state・setter・fallback・選択著者の再変換の参照は0。著者の正本はmapに一つだけで、新旧row cacheの併存はない。固定head独立review・実必須CI・merge後一致はIssue/PRへ集約し、揃うまでCompleteとしない。差し戻しは本PR単独revertで可能（永続移行なし）。

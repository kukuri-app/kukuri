# #1777 / #1783: 新しい原則に基づくリファクタリング完了記録

## 判定と固定範囲

正本は [#1777](https://github.com/kukuri-app/kukuri/issues/1777) と [AC-FINAL / #1783](https://github.com/kukuri-app/kukuri/issues/1783)。Scope revision `2026-10-12-refactor-r2-cn-build`。選定した4構造整理と8文書同期の末端AC/INVAR、固定head review、実CI、main対象path一致を確認した。各子の詳細証拠は下表の一つの記録へ集約する。今回の有限監査は17候補を分類して終了し、全branch・全OS・全DB実行計画や全経路の件数非依存を証明したものではない。

2026-10-12のユーザー依頼で実装・検証・監査・PR・merge・完了確認が承認された。r2ではCIで再現したCN辞書cache欠落の最小修正だけを同じPR-CNへ含めることを追加承認し、ACを着手前に再定義した。UX/API/保存互換性の変更はない。未選定J1〜J8等の製品判断を本campaignへ追加していない。

## 比較commitと確定入力

| 入力 | commit / 意味 |
| --- | --- |
| 今回の調査・before | `1f77dd4e3252b37fc944446544b901435c04063f` |
| 実装・現状文書完了source | `5595018f8306ca9722fae032920431d4fd2f0af3`（5成果の完成main） |
| baseline比較commit | `cffa5eddd3bb378c5f2f6717109574a7ba38b7d8`（上記製品treeを保持し、解消済み大型登録1件を除去。公開PRのmerge commitでmain祖先へ保持する） |
| baseline公開merge | #1783 / PR-FINALのComplete evidenceで記録（本JSONの自己参照hashにしない） |
| 前回review baseline | `e98d2025e01332a6dbd4b13f5f4712654a931c65`、#872。過去本文/記録は変更しない |

`xtask/refactoring-audit-baseline.json` は既存v1 schemaで、code tree OID、Git祖先、before/afterのtracked集合/digest、ratchet実測と許容上限、構造metric、4製品PRの固定head/merge/checkを保存する。更新scriptや測定frameworkは保守対象に追加しない。次回の確定入力はreview付きPRがmainへ入り、親#1777のCurrent statusがCompleteとなった後に有効にする。

## ACと成果の対応

| 末端AC | 成果と維持条件 | PR・証拠 |
| --- | --- | --- |
| AC-GEN / #1778 | topic別世代履歴を単一account共有counterへ。旧世代の遅着を拒否、delivery診断/holder/wireは維持 | [#1784](https://github.com/kukuri-app/kukuri/pull/1784)、[証跡](2026-10-12-1778-subscription-generation.md) |
| AC-PROFILE / #1779 | 行/cursor/loadingを著者mapの1正本へ。選択コピー・二重更新・VM fallbackを撤去。著者隔離・戻る・8窓/200行・close後cursor保持は維持 | [#1785](https://github.com/kukuri-app/kukuri/pull/1785)、[証跡](2026-10-12-1779-author-timeline.md) |
| AC-CLI / #1780 | 38操作のenum/実行/登録の手書き3正本を1表へ。157catalog metadata bytes、guard/redaction/単回実行/file契約一致 | [#1786](https://github.com/kukuri-app/kukuri/pull/1786)、[証跡](2026-10-12-1780-content-command-table.md) |
| AC-CN / #1781 | auth後の準備と結果処理を1責務へ。q検証順・推薦public横断・private所属/epoch・locator/advisory/optout/需要を維持。承認済み辞書cache修正を含む | [#1787](https://github.com/kukuri-app/kukuri/pull/1787)、[証跡](2026-10-12-1781-index-query-response.md) |
| AC-STATE / #1782 | 固定8文書で現状/調査履歴/採用範囲/後継を区別。運用操作・保持設定・歴史判断本文・Tantivy例外を維持 | [#1788](https://github.com/kukuri-app/kukuri/pull/1788)、[Issue完了証拠](https://github.com/kukuri-app/kukuri/issues/1782) |

必要なcaller切替・旧経路撤去は各成果PR内で完了。新旧の採番/state/手書き宣言/応答処理の併存、未使用基盤、新crate/汎用frameworkはない。既存のpagination、query gate、command registry、network ownerを再利用する。各PRは永続移行なしの単独revert、baselineは本PR単独revertで直前のreview入力へ戻せる。

## 同一責務範囲のコード量

UTF-8 Git blobの物理行（空行込み、末尾LFの空要素を除く）。各責務の同じpath集合について実装・必要test・補助を数え、新規/削除/移動を含める。対象pathはJSON metricと子証跡で固定する。新規code/test/補助pathはなく、必要testは既存fileの拡張・fixture正本の置換だけ。移動や行圧縮を成果に数えない。

| 責務 | before | after | 差分 |
| --- | ---: | ---: | ---: |
| #1778 / 4path | 2,358 | 2,357 | −1（実装−21、test+20） |
| #1779 / 17path | 9,938 | 9,882 | −56（実装−62、test+6） |
| #1780 / 1path | 353 | 243 | −110 |
| #1781 query / 1path | 754 | 739 | −15 |
| 4refactor合計 / 23path | 13,403 | 13,221 | **−182** |
| 承認済みDockerfile修正 / 1path | 70 | 68 | −2 |
| code/test/補助全体 / 24path | 13,473 | 13,289 | −184 |

Dockerfileの2行減はcache mountを外した命令変更の物理差分で、refactorの182行減へ水増ししない。構造成果は世代履歴の撤去、選択コピー参照0、38操作の局所1宣言、query準備/結果3→1・需要2→1で判定する。文書は別計測: 子証跡4fileは新規0→187行、現状8文書は3195→3204行（+9）。本完了記録も別追加。。baseline JSONの更新量もコード削減へ混ぜない。

## 検証・review・merge一致

局所検証のcommand/環境/前後結果は各証跡を参照する。#1778はscope10/診断10/wire13、旧sourceへ最終拡張testを当てた失敗再現と完成sourceの成功。#1779はbefore72/after80 Vitest・型・ESLint・同条件Chromium2導線。#1780はparity5/single1/commands9/content2、clippy、157catalog metadata完全一致。#1781はPG/Valkeyを有効化したbefore/after HTTP14、module4、clippy。未設定skipをPASSに数えず、不変の製品testは再実行していない。

| Issue / PR | 固定head review | 最終実CI | main対象一致 |
| --- | --- | --- | --- |
| #1778 / #1784 | PASS `ec83ed283` | 29成功 / 3skip | 5path一致 |
| #1779 / #1785 | PASS `db21f50d1` | 27成功 / 3skip | 18path一致 |
| #1780 / #1786 | PASS `f5ce7a62b` | 29成功 / 3skip | 2path一致 |
| #1781 / #1787 | PASS `ab8582780` | 30成功 / 3skip | 3path一致 |
| #1782 / #1788 | PASS `2b15913be`（指摘2行修正後） | 発生0、製品suite対象外 | 8path一致 |

#1782は文書path filterで発生CI0。schemaのchild.check_countは1以上なので4製品PRだけをJSON childrenに保存し、文書成果は本表とIssueへ対応付ける。0を成功1へ置換しない。`all_checks_success=true` は必要CIを満たしfailure/pendingが無い記録で、条件付きskipを実行成功へ数える意味ではない。内訳は表に明示する。source treeの組合せは全子の対象pathが監査headと完了sourceで一致し、共有contractに追加差分がないことを確認した。audit/merge SHAはJSONの固定値、全体判定はIssue/PRへ記録する。

失敗と再実行: #1778の不変PWA shell cache assertionが1回失敗し、同headの失敗jobだけ再実行して成功した（既存#1767/#1773の記録と一致）。#1781は元headのCN image buildが辞書欠落で2回失敗。ユーザー承認後にcook/buildのcache mountを外し、通常layerの辞書保持を追加監査し、最終headの実image build成功を確認した。cold cook→source compileは実CI、復元層に辞書が残ることは固定Dockerfile/layer経路の監査で確認した。別runnerへcache export/importする追加実験は未実施。失敗をPASSへ言い換えず、assertion・必須jobの削除/緩和も行っていない。

## 次回監査のvalidationと限界

既存checkerのschema/祖先/集合/digest/cap/metric意味検証、変更後fixture14件（実xtask経由）、実repositoryの読み取り専用check、oversized gate、diff/相対リンク検証はPASS。読み取り専用checkはtracking_issue=1777、ratchet新規0/上限増0、audit_required=falseを返した。2791候補path・21大型登録・10構造metricを保存。固定head独立review・実際に発生するPR CI・main一致とbaseline公開SHAの最終結果は#1783 Complete evidenceへ集約する。

発火signalは既存 `ratchet_new_paths > 0 OR ratchet_increased_caps > 0` のまま。schedule、required CI、Issue writer、schemaは無変更。oversized ratchetは調査起点1f77で既に991行だったDesktopShellPageの旧cap1000登録だけを、REFACTORING.mdの既存解消手順で除去した（JSON4行減）。他cap・1000行判定・発火閾値は維持し、コード削減へ混ぜない。比較commitをmain祖先へ残すため本PRはmerge commit方式を使い、squash/rebaseしない。commit数・changed path・hotspot・経過日は観測のみで、新しい閾値を追加していない。初期監査signal（新規11/上限増4）は分類の開始理由として記録し、baseline更新でそれまでの承認済み変化を確定した。

候補は実施4、延期2、却下3、別種別8（文書1のみ今回実施）、未分類0。J1〜J8の保持/満杯時/一覧互換/再集計/取消意味などは#1777の有限調査で判断点を記録して終了し、未承認の製品変更は実装していない。ADR0025§10/#1724のTantivy一致数例外を維持。全アプリCPU/RSS/通信benchmark、実機全OS、production DB/deployは今回の完了条件外で未実施。過去campaignの未測定事項を今回測定済みとも主張しない。

独立reviewで、旧snapshotから引き継いだexact除外に現行scannerのCHANGELOG.md・tools/promo/pnpm-lock.yamlが欠けることを指摘された。現行source_policy_blobの17拡張子・2prefix・5exact除外へ同期し、候補集合/digestを2791pathで再計測した。groupは既存の全tracked集合という定義を維持する。初回のschema/意味検証だけでscanner方針との一致を判断せず、この修正を最終headで再確認する。

DesktopShellPageの991行は調査起点・製品完了main・比較commitで同一であり、今回の削減成果ではない。旧大型登録の解消を#1779の成果とした初回説明は独立reviewで訂正した。上表の182/184行集計は不変で、旧登録除去は監査設定の整合として別計測する。

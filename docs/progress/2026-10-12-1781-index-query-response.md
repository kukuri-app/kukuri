# #1781: CN index query の要求準備と結果処理の統合

## 完了境界

条件の正本は [#1781](https://github.com/kukuri-app/kukuri/issues/1781)、親 [#1777](https://github.com/kukuri-app/kukuri/issues/1777)。`AC-CN / PR-CN`、区分C、Scope revision `2026-10-12-refactor-r2-cn-build`。比較起点は `1f77dd4e3252b37fc944446544b901435c04063f`、実装起点は `23dd5cf10c445713688ea30284b8179a1bea75eb`。対象sourceは両commitで同一。

## 構造と維持条件

3 HTTP handler は既存名・signature・routeのまま、module内の `run_index_query` を呼ぶ。有限の3種類を `IndexQuerySurface` で渡し、readiness/auth/consent、scope準備、所属証明と現epoch、需要記録、query error変換、relation visibility、DTO生成を一つの経路にした。旧handler内の重複は撤去し、framework・crate・依存・別adapterを追加していない。

search は `require_index_query` の後、scope解釈の前に既存と同じ q 必須検証を行う。discovery はscopeを解釈し、recommendation は従来どおりscope/qを解釈せずpublic横断の `list_recent(None)` を呼ぶ。private所属拒否は需要記録とqueryより前。操作別 `IndexingOperation` は維持した。

limitの既定20とgateの1〜100、検索順/新着順、fail-closed、source locator、content advisory、distance opt-outは既存query/gate/DTOをそのまま再利用する。Tantivy方式、W/B/T、信頼再計算、配備は変更しない。推薦に渡すepochは常にNoneで、追加のfilterは即時に同じentryを返す。

helper単位の抽出案は呼出側に結果処理や所属/需要組立てを残すため、3入口の有限なdispatchを一つにする案を選んだ。共通処理の対象は有界query応答だけで、全台帳を新たに読む経路はない。

## 検証

Windows、Rust 1.98.1。専用Compose project `kukuri-refactoring-1777` のPostgres/Valkeyを127.0.0.1:25432/26379で起動し、`KUKURI_CN_RUN_INTEGRATION_TESTS=1`、対応するdatabase/Redis URLで実行した。synthetic fixture認証値のみを使い、既存配備環境は変更していない。未設定によるskipはない。

| command | before | after |
| --- | --- | --- |
| `cargo test --locked -p kukuri-cn-user-api --test index_query` | 7 PASS | 7 PASS |
| 同 `--test activation_gate` | 7 PASS | 7 PASS |
| 同 `--lib handlers::indexing` | 既存test再利用 | 4 PASS |
| `cargo clippy --locked -p kukuri-cn-user-api --lib -- -D warnings` | — | PASS |
| 対象rustfmt・`git diff --check` | — | PASS |

HTTPの認証/需要、非設定404、parameter拒否、3surfaceのallow判定、private所属証明、locator、advisory、distance opt-out、activationの失効/取消を既存assertionのまま確認した。新しいtestやassertion緩和はない。既存Windows linker warningのみ。全CN suiteはPR CI、固定head独立監査・実CI・merge一致の結果はIssue/PRに記録する。

## 同一対象のコード量

UTF-8物理行、空行込み、末尾newlineの空要素を除く。同じ `crates/cn-user-api/src/handlers/indexing.rs` は754→739行（15行減）。実装/test/補助の新規fileはない。commentは元の内容を維持し、移動や行圧縮による削減ではない。本証跡文書はコード量と別計測する。差し戻しは本PR単独revert、永続移行なし。

## 承認済みのビルド修正

2026-10-12にユーザーが同じPRへ最小修正を含めることを承認し、AC-CNをScope r2へ先に更新した。元headのCN image buildはrun38224282308の初回job114727848846と再実行114729451204で、復元したcook層が参照するLindera辞書cache mount内の9file欠落により失敗した（製品処理到達前）。失敗を成功扱いせず記録する。

`docker/cn/Dockerfile` のcook/buildの2 cache mountを外す。既存cache path・辞書・compiler/package・feature・loop・runtime stageは維持し、生成辞書を依存layerへ含める。cold cookで作った辞書は復元したcook層にも残り、source変更後のcompileから参照できる。外部配備・CI必須job・workflow変更はない。初回は辞書生成を伴う。変更済みDockerfileの静的差分と最終headのCN image CIを確認し、固定head独立監査へ渡す。Rust source/testsは元監査headから無変更なので既存成功を再利用する。

同一対象集合にDockerfileを追加して計測する（基準70→68行、2行減）。indexing.rsと合わせて824→807行、17行減。辞書layerの修正は命令継続行を短く見せることが成果ではなく、cache mount外に辞書を保存して欠落を解消することが成果。証跡文書は別計測する。

# #1724 AC-3: 検索 reader への切替

受入条件と現在判定の正本は [#1724](https://github.com/kukuri-app/kukuri/issues/1724)。
Scope revision 3、担当 AC-3 / PR-3。比較元は AC-2 の head `970ba5b78`。

## 実装と撤去

cn-user-api は `COMMUNITY_NODE_INDEXER_DATA_DIR/search` の reader を開き、既存の
`FailClosedIndexQuery` の検索に渡す。新着列挙は `IndexRecent` で従来の ArcadeDB に委譲し、
gate、応答、認証・所属・limit・需要の記録は維持する。検索の CPU 処理は blocking pool で行う。
要求ごとに reader の metadata を更新し、1秒間隔で commit した変更を読む。

ArcadeDB の `SEARCH_INDEX` の2つの文と、`MemoryIndexProjection` の近似と2つの検索を消した。
旧全文索引は schema 初期化で削除する。RAM の試験も同じ Tantivy / NEologd を使い、fixture の
書込み・削除で commit する。従来の新着の文・投影の upsert / remove / count_all の文は変更しない。
実 DB の試験の重複した HTTP / readRecord helper は共有した。

本番 / 開発の compose は indexer の同じ volume / disk を cn-user-api に書込み可能で渡す。
E2E は同じ TempDir を API と indexer に渡し、本番と同じ書込み境界と committer を使う。
fixture の投影待ちは commit 後の索引の存在まで待つ。指定された allowed / denied / failure の
テスト本体は変更していない。仕様は ADR 0025 §2.1 / §2.7 / §6.5 / §6.7 / 新§10 に反映した。

## 局所検証（2026-10-10、Windows）

- 検索の unit 2件: AND、phrase の並び、BM25、scope、横断の非公開除外、NFKC / 小文字、
  固有名詞の部分語を返さないことを確認。2,000 / 20,000件で `Count` と返す rare の件数は50、
  common の返す文書は100件。
- 専用 ArcadeDB 26.10.1: `search_database_contracts` で topic 内・横断の検索の前後の
  `readRecord` 差が2,000 / 20,000件とも0。旧全文索引の削除で投影の文書数は変わらない。
- `query_contracts` 7件、advisory の取込2件、blob text の取込3件、実 ArcadeDB / Postgres17 の
  `runtime_integration` 6件、cn-user-api の `index_query` 7件が成功。
- Postgres17 (`127.0.0.1:15438`) / Valkey8 (`127.0.0.1:16387`) / ArcadeDB26.10.1
  (`127.0.0.1:32487`) で `cn-e2e` の allowed 1件、denied 1件、failure 6件が変更なしで成功。
  E2E のゲートを有効にし、`--test-threads=1` で実行。
- 関連3 crate の lib の clippy `-D warnings` と compile、関連 rustfmt が成功。
  dummy の非秘匿 env 値で開発 compose の `config --quiet` が成功。Terraform module の fmt が成功。

## 表3の取り直し（本番コード、release build、10回の中央値 ms）

起票コメントと同じ20語から15語を取り、印の語と「の」「。」を加えた本文を使用。
同じ `StdRng` 0.8・追加開始位置の seed・scope の分け方で、2,000→20,000→200,000件と追加。
本番の `SearchIndex` / `SearchReader` で query の形態素解析・metadata 更新・結果の復号まで含めた。
以前の試作の時間は query の事前構築後の検索から計測しており、測定区間は異なる。

| 検索 | 2,000件 | 20,000件 | 200,000件 | 読む保存文書 |
| --- | --- | --- | --- | --- |
| 全件一致（横断） | 0.616 | 0.984 | 4.043 | 100 |
| 1/10一致 | 0.554 | 1.134 | 1.449 | 100 |
| 50件一致 | 0.539 | 0.506 | 0.676 | 50 |
| 1文字「東」 | 0.278 | 0.259 | 0.555 | 0 |
| AND「東京 写真」 | 0.611 | 1.269 | 3.787 | 100 |
| 大きい scope の全件一致 | 0.489 | 1.016 | 3.192 | 100 |
| 200件の scope の全件一致 | 0.412 | 0.403 | 0.568 | 100 |
| 同 scope の rare | 0.269 | 0.278 | 0.331 | 5 |
| 索引 bytes（保存を含む） | 291,202 | 2,710,357 | 27,081,883 | — |

OR は今回の製品の意味に含めないため、試作の参考行を再実装しない。性能は本番の予測値ではなく
件数との増え方の証拠。全件一致の走査はユーザーが受け入れた例外であり、保存文書は返す件数だけ。
一時的な benchmark project / script は repository に入れない。

固定 head の独立監査、PR CI、merge 後の対象内容の一致を別に確認する。本番配備は対象外。

# #1724 AC-1: 検索索引への書込みと回収

受入条件・現在判定の正本: [#1724](https://github.com/kukuri-app/kukuri/issues/1724)、Scope revision 2。
比較起点は `3be91e4ca`。担当は AC-1 / PR-1、維持する条件は INVAR-1〜3。

## 実装と経路

`SearchProjection` を `compose_ingest_stack` で組み、既存の `IndexProjection` の呼出元
（取込、撤回、de-index、送信防止、scope の解除、保持期間・容量の回収）へ共通に適用する。
ArcadeDB の既存の文は変更しない。検索の保存先は indexer の data dir の `search`。
Tantivy 0.26.2、lindera 5.3 / IPADIC-NEologd を使い、本文を NFKC と小文字に揃えて
位置つきで索引する。元の本文と投稿の情報を保存し、reader は別にディレクトリを開く。

変更のあるときだけ1秒間隔で commit する。commit 前の追加と削除の台帳は512件までで、
容量に達したときもまとめて commit し、commit 後に台帳を消す。停止時は最後の変更を commit する。
scope と作成時刻の posting は必要件数で読取りを止め、回収は各索引で128件以内。
未確定の変更を含めて選ぶので、commit 前の追加を取り残さず、同じ削除をページとして数え直さない。

今回の検索 reader は AC-1 の反映の検証にも使う。cn-user-api の検索の入口、既存投稿の写し、
ArcadeDB の旧検索と全文索引の撤去は、それぞれ予定済みの AC-3 / AC-2 / AC-3 が担当する。

## 検証（2026-10-10、Windows）

- `cargo test -p kukuri-cn-indexer --test search_projection`: 2件成功。
  別の reader で本文の更新・削除、commit 前の追加と削除、300件以上の有界な回収を確認。
  実際の取込、非 allow への判定変更、署名済み撤回も検索に反映された。
- 専用 Postgres 17 (`127.0.0.1:15438`) で `KUKURI_CN_RUN_INTEGRATION_TESTS=1` を設定し、
  `cargo test -p kukuri-cn-indexer --test retention_contracts`: 7件成功。
  送信防止の消し待ちを含め、成功・失敗・次の巡回での再試行を確認。
- `cargo test -p kukuri-cn-indexer --test query_contracts`: 7件成功（既存の gate、limit、非公開 scope）。
- 対象 lib と上記の変更した2つの test の `clippy -D warnings`、対象の rustfmt、
  `actionlint`（Fast / Nightly）、notice generator の既存の smoke / 別 workspace / 拒否試験が成功。

## 辞書・配布と検証先

辞書の出典と全文の notice を `docs/licenses/` へ置き、第三者の表示の生成にも含めた。
生成された表の追加分に合わせて oversized baseline の該当行数だけを更新する。
Docker の cook / build は共通の辞書 cache を使う。Fast / Nightly では辞書の組立ての出力
（5.3.0、format 2、OS 別）を各 job が再利用する。既存の job の必須条件・timeout は変更しない。
手元で辞書を含む最初の build は成功し、以後の変更した test の再ビルドは約14秒だった。
実 runner の cache の取得、全体の CN 試験と E2E は PR CI、Linux の image の build と容量は
専用の手元の Docker build で確認する。これらは本記録の局所成功へ加算しない。

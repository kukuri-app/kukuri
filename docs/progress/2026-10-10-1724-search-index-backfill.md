# #1724 AC-2: 既存投影の写しと停止・再開

受入条件と現在判定の正本は [#1724](https://github.com/kukuri-app/kukuri/issues/1724)。
Scope revision 3、担当 AC-2 / PR-2、比較元 `cde615e8e`（AC-1 / PR #1739 の merge）。

## 実装・根拠

作成時刻だけを位置として `< last_created_at` で続ける案は、同じ秒の投稿を取りこぼす。
専用 DB に同じ作成時刻の2,000件を置くと、最初の128件の後の読取りが0件になった。
ユーザーは決定8で `(created_at, scope_kind, scope_id, object_id)` の複合索引の追加と、
初回作成中の読み書きの待ちを受け入れた。最初の作成時刻を1件の読取りで得て、
同じ時刻の scope / object の範囲を索引で読む。複数列の ORDER BY は全件を読んだため使わない。

`BackfillCursor` は、開始、時刻の探索、同じ時刻の scope / object の位置、完了を表す。
各回の投影の文書の読取り・upsert は合計128件以内。時刻の探索で得た投稿を、同じ時刻の
範囲でも読むことがあるが、upsert は scope / object の組で冪等であり文書は増えない。
この投稿も128件の読取りの枠に含める。

位置・完了は Tantivy の commit payload に保存し、普通の live の commit でも維持する。
写しの途中で容量による commit が入っても位置を先に進めず、再開時の同じ行の upsert で続ける。
完了後の巡回・再起動では投影を読まない。進行の台帳は一つの位置だけで、件数で増えない。

live の書込み・削除と写しは同じ `SearchProjection` の mutation lock で直列にする。
投影から読んだ後に削除が入り、その古い行を写して削除を戻す経路を作らない。
既存の1秒間隔の committer と背景の写しを一つの task にまとめ、同じ停止・最後の commit を使う。
ArcadeDB の upsert / remove / count_all、list_recent の文、API と gate は変更しない。
検索の入口の切替と旧全文索引の撤去は既定の AC-3 が担当する。

## 局所の検証（2026-10-10、Windows）

- `KUKURI_CN_RUN_ARCADEDB_TESTS=1`、空の専用 ArcadeDB 26.10.1 (`127.0.0.1:32487`) で
  `cargo test -p kukuri-cn-indexer --test backfill_contracts -- --test-threads=1`: 成功。
  同じ秒・複数 scope・公開/非公開を含む300件から1回128件で止め、同じディレクトリを
  開き直して全件の検索を確認。live の commit 後にも完了記録が残り、再起動後の読取りは0。
  文書2,000件 / 20,000件で最初と次の page の `readRecord` の差はそれぞれ128件。
- `cargo test -p kukuri-cn-indexer --lib backfill_tests`: 完了記録の維持と、到達不能な投影へ
  読取りを送らない試験が成功（1件）。
- 同じ専用 ArcadeDB と専用 Postgres 17 (`127.0.0.1:15438`) で、変更した schema / commit を通す
  `runtime_integration` 6件、`retention_contracts` 7件、`search_projection` 2件が成功。
  `KUKURI_CN_RUN_INTEGRATION_TESTS=1`、ArcadeDB の試験は `--test-threads=1` で順に実行。
- 対象 lib / 新しい test / 反映 test の `clippy -D warnings`、対象 rustfmt、diff の空白検査が成功。

実 ArcadeDB のこの試験は CI に無いため、手元の結果を PR に記録する。全体の確認は PR CI。
固定 head の独立監査と merge 後の一致を別に確認する。

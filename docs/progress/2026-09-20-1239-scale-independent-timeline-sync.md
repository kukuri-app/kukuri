# #1239 タイムラインの反映と復旧を総件数に依存させない（2026-09-20〜）

- 対象 Issue: #1239（区分 C、Scope revision 2026-09-20-v2）。統括は #1221。子は #1243（replica の時間分割）。
- 設計: [ADR 0052](../adr/0052-scale-independent-timeline-sync.md)。inventory: [replica の読み出しの inventory](../architecture/replica-read-inventory.md)。
- 段階ごとに PR と独立監査を分ける。本書は段階ごとに追記する。

## T1: ADR と inventory（PR #1244、merge commit `1c70abd3`）

文書のみ。ADR 0052 と inventory を追加した。

## T2: docs-sync の読み出し（基準 commit `1c70abd3`）

### 修正前の再現

計測 test `measure_exact_query_cost`（`crates/docs-sync/src/tests/iroh_sync.rs`、`#[ignore]`、debug build、iroh の memory store）。
1 つの replica の entry 数を変えて、key を 1 つ指定した読み出し（`DocQuery::Exact`）の平均時間を測った。

| entry 数 | 修正前 | 修正後 |
| --- | --- | --- |
| 1,000 | 2.44 ms | 0.27 ms |
| 10,000 | 24.04 ms | 0.31 ms |

修正前は entry 数に比例し、修正後は依存しない。計測値は「足りる」の根拠ではなく、件数に対する増え方を示すために記録する。

原因: `docs-sync` は query を既定の並び順（`SortBy::AuthorKey`）で組んでいた。iroh-docs 0.101.0 は、著者の指定が無い `AuthorKey` の query を
namespace 全体の table scan として実行する（`src/store/util.rs` の `IndexKind::from`、`src/store/fs/query.rs` の「full table scan with the provided key filter」）。
`SortBy::KeyAuthor` を指定したときだけ、key の索引（`records_by_key`）を境界つきの range で読む（`src/store/fs/bounds.rs` の `ByKeyBounds`）。

### 変更の要約

- `crates/docs-sync/src/iroh_sync.rs`: すべての query を `indexed_query`（`SortBy::KeyAuthor`、昇順）で組む。`Exact`・`Prefix`・`All` の結果の集合は変わらず、
  並びは key の昇順になる（`MemoryDocsSync` と同じ）。同じ key に複数の docs 著者の entry があるときの順序（docs 著者 id の昇順）は変わらない。
- `crates/docs-sync/src/types.rs`: key だけを返す上限つきの読み出し `DocsSync::query_replica_keys`（prefix、昇順 / 降順、`limit`）を追加した。entry の本体は読まない。
  trait の既定実装はエラーを返す（全件読みへ黙って落ちる実装を作らない）。`IrohDocsSync`・`MemoryDocsSync`・`ReloadableDocsSync`（desktop-runtime の委譲 wrapper）が実装する。
- `crates/docs-sync/src/time_index.rs`: 時系列の索引（`indexes/timeline/…`・`indexes/thread/<root>/…`）を、新しい順に上限つきで読む `query_time_index_desc` を追加した。
  cursor があれば、cursor の時刻の桁を下から順に 1 つずつ減らした prefix を新しい側からたどって、古い側の 1 ページを読む。1 回の呼び出しの query 数は
  「時刻の各桁の数字の和 + 1」以下の定数で、索引の大きさに依存しない。
- `crates/desktop-runtime/src/runtime/sync_live_api.rs`: test と harness が使う `has_topic_timeline_doc_index_entry` を、索引の全件読みから key 指定の読み出し 2 回へ置き換えた（inventory の P-15）。

caller の並びへの依存は無いことを確認した。prefix 読みの結果を順に反映する caller は、同じ key の最後の entry（docs 著者 id が最大のもの）が残る。
変更前（著者、key の順）でも変更後（key、著者の順）でも、key ごとに最後に反映される entry は同じ。

### AC / TR と証跡

| 条件 | 証跡 |
| --- | --- |
| TR-13（key 指定・上限つきの読み出しが総 entry 数に依存しない） | `every_docs_query_is_built_on_the_key_index`（どの query も `KeyAuthor` で組まれる）、計測 test の前後比較 |
| AC-2・AC-7 の前提（上限つきの読み出し） | `key_query_respects_prefix_order_and_limit_on_both_implementations`（memory と iroh で同じ結果、`limit` を超えない）、`walking_older_entries_uses_a_bounded_number_of_bounded_queries`（索引 100 件と 10,000 件で query 数が同じ上限に収まり、返す entry 数は `limit` 以下） |
| 遡りの読み出しの正しさ | `time_index_matches_reference_on_memory_docs`・`time_index_matches_reference_on_iroh_docs`（同じ秒の複数 entry、桁の繰り下がり、空の範囲、`limit` ちょうど、ページを継いだ全件の読み出しを、基準実装と突き合わせる） |
| 委譲 wrapper の転送 | `reloadable_docs_sync_forwards_bounded_key_queries` |
| INVAR-1〜3（既存の挙動） | `cargo xtask rust-test` 1,087 件、`cargo test -p kukuri-cn-indexer` が無変更で成功 |

### 未確認・残課題

- 実 peer を相手にした同期中の読み出しの計測は未実施。
- この段階では prefix の全件読みそのものは残っている（T3 以降で無くす）。全件読みも key の索引を使うようになったため、`objects/` を読む走査は `indexes/*` などほかの prefix の entry を読み飛ばさなくなった。

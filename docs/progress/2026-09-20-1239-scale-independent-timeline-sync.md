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

## T3: 利用者の操作と view の生成（基準 commit `783a813e`）

T2 は PR #1245（merge commit `783a813e`）で完了した。独立監査は PASS、必須 CI は全 job 成功。

### 修正前の再現

`crates/app-api/src/tests/sync/scale_independence.rs` の 8 本を基準 commit の実装で実行し、すべて失敗することを確認した。
判定は所要時間ではなく、docs の query が返した record 数で行う。topic の投稿数を 20 件と 400 件にして比べた。

| 操作 | 修正前（20 件 → 400 件） | 修正後 |
| --- | --- | --- |
| reaction（対象が projection にある） | 40 → 800 record | 件数に依存しない |
| reaction（対象が projection に無い） | 40 → 800 | 同上 |
| bookmark | 41 → 801 | 同上 |
| reply | 43 → 803 | 同上 |
| repost（同じ投稿を 2 回） | 83 → 1,603 | 同上 |
| 取り下げ | 42 → 803 | 同上 |
| community index の解決 | 40 → 800 | 同上 |
| repost を含むページの view の生成 | `withdrawals/` の全件読みが走る | topic の replica への読み出しは、repost 元の取り下げの key 指定の確認 1 回以下 |

### 変更の要約

- `ensure_object_projection`（`service/timeline_subscription_support.rs`）: 対象が projection にあれば何も読まない。無ければ、scope の replica から `withdrawals/<id>/state` と
  `objects/<id>/state` を key 指定で読んで反映する（取り下げを先に反映するので、取り下げより後に反映した投稿でも本文が残らない）。replica は走査しない。
  private channel の scope は、参加状態の確認（`ensure_private_channel_access`）を通った epoch の replica だけを読む。#1225 の `hydrate_scope_projection_for_target` と `forget_scope_scan_cache` は削除した。
- caller: repost 元の bookmark・取り下げ・reply（`timeline.rs`）、reaction（`reactions.rs`）、community index の解決（`community_index.rs`）、repost 元の解決（`service/mod.rs`）。
- reaction: 自分の既存の reaction が projection に無ければ、`reactions/<target>/<reaction id>/state` を key 指定で反映してから toggle を決める（reaction id は対象・著者・key から決まる）。
- `find_existing_simple_repost`: topic の全 `objects/` を読んで deserialize する代わりに、projection の索引で引く。`crates/store` に `find_author_reposts_of` と、
  repost の行だけを対象にした式の索引（migration `20260920000000_repost_source_index`）を追加した。取り下げ済みの repost は既存の repost として扱わない
  （以前は docs の state が残るため、取り下げた repost の id を返し続け、同じ投稿を repost し直せなかった）。
- view の生成（`timeline_view_support.rs` の 3 か所）: 行ごとの `withdrawals/` の全件読みをやめ、projection の取り下げ表だけで判定する。購読していない topic の投稿でありうる
  repost 元と profile の投稿は、背景で `withdrawals/<id>/state` を key 指定で確認する（`WithdrawalCheckLedger`: object ごとに 60 秒の間隔、同時 4 本、台帳 4,096 件）。
- docs の event の個別反映に `withdrawals/` の key を足した（以前は public topic では hint か全件走査まで反映されなかった）。

### 独立監査の指摘と修正（1 回目は FAIL）

1 回目の独立監査（対象 head `9aa826dd`）は、blocker 2 件で FAIL だった。どちらも監査が失敗する test で再現し、その test を恒久の回帰 test として取り込んだ。

| 指摘 | 原因 | 修正 |
| --- | --- | --- |
| 購読していない topic の取り下げ済みの返信先が、profile の reply preview に本文つきで出続ける（INVAR-1） | view の生成から `withdrawals/` の全件読みを外したとき、背景の確認の対象を「profile の投稿」と「repost 元」だけにし、返信先を入れなかった | `profile_post_to_view` で、返信先の object id も背景の取り下げの確認へ出す。回帰 test `withdrawn_reply_parent_in_an_unsubscribed_topic_is_masked_in_the_profile_reply_preview` |
| 退出した private channel の投稿を、手元に残った projection の行から bookmark できる（INVAR-2） | 参加状態の確認（`ensure_private_channel_access`）が、対象が projection にあるときの早期 return より後ろに移っていた | `ensure_object_projection` は、private channel の scope では早期 return より前に参加状態を確認する。回帰 test `left_private_channel_post_cannot_be_bookmarked_from_a_leftover_projection_row` |

同じ監査の non-blocker のうち、この段階で直したもの:

- 取り下げの確認の台帳の key を、object id だけから「replica と object id の組」に変えた（topic を偽った repost の snapshot が、正しい topic での確認を見送らせないようにする）。
- 取り下げの event が対象の envelope より先に届くと、取り下げを検証できずに反映されない。docs の event と hint の投稿の個別反映を `hydrate_object_by_id` に通し、
  投稿を反映するときに同じ object の取り下げを先に確認するようにした（T4 への申し送りだったが、個別反映の入口が T3 で出来たのでここで対応した）。
- 読み出しの policy を引数にした。利用者の操作は `LocalOnly`（基準 commit の走査と同じ）、repost 元の解決と event・hint の個別反映は `LocalThenRemote`。方針は ADR 0052 §4 に書いた。
- view の生成に残る key 指定の docs の読み出し 2 か所（V-1・V-2）を inventory に分類つきで記録した（T5 で外す）。
- 取り下げ済みの simple repost を既存の repost として扱わない点を ADR 0016 §2.3 に書き、回帰 test `withdrawn_simple_repost_can_be_reposted_again` を足した。
  envelope の id は著者・秒単位の作成時刻・内容から決まるので、取り下げと同じ秒のうちに repost し直すと、取り下げた repost と同じ id になり取り下げ済みのままになる（以前から同じ。test は秒をまたぐ）。

### AC / INVAR と証跡

| 条件 | 証跡 |
| --- | --- |
| AC-3、TR-7 | `reaction_reads_a_constant_number_of_docs_records`、`reaction_on_an_unprojected_target_reads_only_the_target_keys`、`bookmark_…`、`reply_…`、`repost_…`、`withdrawal_…`、`community_index_resolution_…`、`missing_target_fails_without_scanning` |
| AC-6、TR-9 | `timeline_view_generation_does_not_scan_docs` |
| INVAR-1 | `withdrawn_repost_source_in_an_unsubscribed_topic_is_masked_after_the_background_check`、`withdrawn_reply_parent_in_an_unsubscribed_topic_is_masked_in_the_profile_reply_preview`、`withdrawal_doc_event_is_reflected_by_key_without_scanning`、既存の取り下げ・repost・bookmark・reaction の test（無変更で成功） |
| INVAR-2 | `private_channel_target_is_not_read_without_membership`、`left_private_channel_post_cannot_be_bookmarked_from_a_leftover_projection_row`、既存の private channel の test |
| INVAR-4 | store の migration の round trip と schema の golden（索引 1 件の追加）、`author_reposts_lookup_matches_between_backends`、`author_reposts_lookup_uses_the_repost_source_index` |

修正後の検証: `cargo xtask rust-test`（nextest 1,104 件と doc test: 成功）、`cargo test -p kukuri-app-api --lib`（234 件: 成功。追加した回帰 test 3 本を含む）、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`cargo xtask oversized-files`（いずれも成功）。

### 未確認・残課題

- repost 元・profile の投稿・その返信先の取り下げが表示へ反映されるまで、最大 60 秒遅れうる（購読していない topic の場合。以前は表示のたびに確認していた）。
- repost 元と profile の投稿の取り下げの確認は、購読していない topic の replica を開いて同期する。以前からの副作用で、開く replica の上限は #1224・#1243 で扱う。
- view の生成には、key を 1 つ指定した `LocalOnly` の docs の読み出しが 2 か所残る（inventory の V-1・V-2）。総件数には依存しないが、T5 で外す。
- private channel の取り下げは現在の epoch の replica に書かれ、対象が過去の epoch にあると検証できない（以前から同じ。inventory の観察に記録した。別 Issue の候補）。
- `cargo xtask rust-test` の 1 回目で `kukuri-transport` の `transport_custom_relay_bootstrap_seed_reports_relay_supported_p2p` が失敗した（差分外。単独実行と再実行では成功）。

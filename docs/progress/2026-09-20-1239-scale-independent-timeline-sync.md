# #1239 タイムラインの反映と復旧を総件数に依存させない（2026-09-20〜）

- 対象 Issue: #1239（区分 C、Scope revision 2026-09-20-v2）。統括は #1221。子は #1243（replica の時間分割）。
- 設計: [ADR 0052](../adr/0052-scale-independent-timeline-sync.md)。inventory: [replica の読み出しの inventory](../architecture/replica-read-inventory.md)。
- 段階ごとに PR と独立監査を分ける。本書は段階ごとに追記する。
- 段階の順序: T1 → T2 → T3 → T5a（タイムラインと thread の取得）→ T4（購読タスク）→ T5b → T6 → T7。T5a を T4 より先に行う理由は、inventory の「段階の順序の変更」に書いた。

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
  repost 元と profile の投稿は、背景で `withdrawals/<id>/state` を key 指定で確認する（`WithdrawalCheckLedger`: 確認先（replica と object id の組）ごとに 60 秒の間隔、同時 4 本、台帳 4,096 件）。
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

### 独立監査の 2 回目（対象 `40838d4e`、PASS）の non-blocker の対応

2 回目の監査は blocker 0 件で PASS だった。non-blocker のうち 3 件は、窓の追いつき（T4）が同じ helper を使う前に決めておくべき内容なので、この段階で直した。

| 指摘 | 修正 |
| --- | --- |
| 投稿の個別反映で、取り下げの先読みの失敗（読めない record、署名の不一致）が投稿の反映を止める。読めない record を 1 件置くだけで、後から同期した閲覧者から特定の投稿を隠せる | 取り下げとして扱えない record は `PostWithdrawalHydration::Invalid` として warn を出して無視し、投稿の反映を続ける（ADR 0052 §2 に明記）。docs と projection の読み書きの失敗はエラーのまま返す。全件走査（T7 で削除）も、読めない record 1 件で topic 全体が失敗しなくなる。回帰 test `unreadable_withdrawal_record_does_not_hide_the_post`、`withdrawal_record_signed_by_another_author_does_not_hide_the_post` |
| reaction の「自分の既存の reaction」の読み出しだけが `LocalThenRemote` のままで、利用者の操作が remote を待ちうる | `hydrate_reaction_cache_from_key` と `hydrate_post_withdrawal_from_record` に読み出しの policy を渡す。利用者の操作は入れ子の読み出しまで `LocalOnly`。回帰 test `user_operations_do_not_wait_for_a_remote_docs_fetch` |
| 「取り下げ → 投稿」の順序を固定する test が無い（先読みを無効にしても全 test が通る） | 回帰 test `withdrawal_that_arrives_before_the_post_masks_it_when_the_post_is_projected`。先読みを無効にする mutation で失敗することを確認した |

追加した 4 本（`crates/app-api/src/tests/sync/withdrawal_reflection.rs`）のうち 3 本は、修正前（`40838d4e`）の実装で失敗することを確認した。
残る 1 本（順序の test）は `40838d4e` で成功し、mutation で失敗する。

### AC / INVAR と証跡

| 条件 | 証跡 |
| --- | --- |
| AC-3、TR-7 | `reaction_reads_a_constant_number_of_docs_records`、`reaction_on_an_unprojected_target_reads_only_the_target_keys`、`bookmark_…`、`reply_…`、`repost_…`、`withdrawal_…`、`community_index_resolution_…`、`missing_target_fails_without_scanning` |
| AC-6、TR-9 | `timeline_view_generation_does_not_scan_docs` |
| INVAR-1 | `withdrawn_repost_source_in_an_unsubscribed_topic_is_masked_after_the_background_check`、`withdrawn_reply_parent_in_an_unsubscribed_topic_is_masked_in_the_profile_reply_preview`、`withdrawal_doc_event_is_reflected_by_key_without_scanning`、`withdrawal_that_arrives_before_the_post_masks_it_when_the_post_is_projected`、`unreadable_withdrawal_record_does_not_hide_the_post`、`withdrawal_record_signed_by_another_author_does_not_hide_the_post`、既存の取り下げ・repost・bookmark・reaction の test（無変更で成功） |
| INVAR-2 | `private_channel_target_is_not_read_without_membership`、`left_private_channel_post_cannot_be_bookmarked_from_a_leftover_projection_row`、`user_operations_do_not_wait_for_a_remote_docs_fetch`（利用者の操作は `LocalOnly`）、既存の private channel の test |
| INVAR-4 | store の migration の round trip と schema の golden（索引 1 件の追加）、`author_reposts_lookup_matches_between_backends`、`author_reposts_lookup_uses_the_repost_source_index` |

修正後の検証: `cargo xtask rust-test`（nextest 1,109 件と doc test: 成功）、`cargo test -p kukuri-app-api --lib`（238 件: 成功。追加した回帰 test 7 本を含む）、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`cargo xtask oversized-files`（いずれも成功）。

### 未確認・残課題

- repost 元・profile の投稿・その返信先の取り下げが表示へ反映されるまで、最大 60 秒遅れうる（購読していない topic の場合。以前は表示のたびに確認していた）。
- repost 元と profile の投稿の取り下げの確認は、購読していない topic の replica を開いて同期する。以前からの副作用で、開く replica の上限は #1224・#1243 で扱う。
- view の生成には、key を 1 つ指定した `LocalOnly` の docs の読み出しが 2 か所残る（inventory の V-1・V-2）。総件数には依存しないが、T5 で外す。
- private channel の取り下げは現在の epoch の replica に書かれ、対象が過去の epoch にあると検証できない（以前から同じ。inventory の観察に記録した。別 Issue の候補）。
- 取り下げの検証には対象の envelope が要る。envelope の本体だけが手元に無く state の本体はある、という順序では、投稿が伏せられずに反映されうる
  （envelope は key の順で state より先に届くので、通常の同期では起きにくい。対象の envelope が届いた後の次の個別反映か背景の確認で伏せられる）。
- bookmark した投稿とその返信先は、その topic を購読していないと取り下げが届かない（以前から同じ。view の生成時の確認は基準 commit にも無い）。
- `toggle_reaction` を channel の指定なしで呼ぶと、参加状態の確認を通らない（以前から同じ。2 回目の監査の範囲外の観察）。
- `cargo xtask rust-test` の 1 回目で `kukuri-transport` の `transport_custom_relay_bootstrap_seed_reports_relay_supported_p2p` が失敗した（差分外。単独実行と再実行では成功）。

## T5a: タイムラインと thread の取得（基準 commit `0521a38b`）

T3 は PR #1246（merge commit `0521a38b`）で完了した。独立監査は 3 回目で PASS（blocker 0 件）、必須 CI は全 job 成功。

### 着手前に洗い出した、全件走査が覆っていた対象

`list_timeline_scoped` と `list_thread` の全件走査（S-6）は、scope の全 replica の 5 prefix（取り下げ・投稿・reaction・live・game）を反映していた。置き換え後の対応は次のとおり。

| 走査が覆っていた対象 | 置き換え後 |
| --- | --- |
| ページの範囲の投稿 | ページの範囲の照合（`reconcile_timeline_range`・`reconcile_thread`）が、索引にあって projection に無い object を key 指定で反映する |
| ページの範囲の取り下げ | 同じ照合が、projection にある行の `withdrawals/<id>/state` を key 指定で確認する。新しく反映する object は、`hydrate_object_in_topic_with` が取り下げを先に確認する |
| ページの範囲より古い投稿・取り下げ | 反映しない（利用者がその範囲へ遡ったときに照合する。best effort） |
| reaction | 照合が新しく反映した投稿の reaction は、照合が上限つき（1 投稿あたり 32 件）で反映する（#1252 の取り込みの節）。それ以外の reaction は、この段階では、docs の event と購読タスクの全件走査（S-1〜S-5。T4 で置き換える）が引き続き覆う |
| live、game | この段階では、購読タスクの全件走査（S-1〜S-5。T4 で置き換える）と `list_live_sessions`・`list_game_rooms`（S-9。T5b）が引き続き覆う |
| 欠けた本文の取り直し | 本文の取り方（`BodyFetch`）を docs の読み出しの policy から分けた。object を 1 件ずつ反映する経路（`Bounded`）は `MissingBodyLedger` の間隔の内でだけ remote を試す。照合（`LocalOnly`）は手元の本文だけを読み、欠けた本文は `recover_missing_bodies`（背景）に任せる |

### 修正前の再現

`crates/app-api/src/tests/sync/range_reconcile.rs` の `timeline_and_thread_pages_are_built_without_any_replica_scan` は、prefix の読み出し（全件走査）を失敗させる docs で
`list_timeline` と `list_thread` を呼ぶ。`timeline.rs` を基準 commit の実装へ戻すと、空の projection からの先頭ページが `hydrate_scope_projection` の全件走査に頼るため、失敗する。

### 変更の要約

- `crates/docs-sync/src/time_index.rs`: 新しい側の読み出し `query_time_index_window`（未来の時刻の entry を読み飛ばす）を追加した。遡りの読み出しは、形の違う key が余裕を超えて
  枠を埋めたとき、古い側の prefix へ進まずに prefix を 1 桁細かく分けて読み直す（T2 の監査の non-blocker。有効な entry を飛ばしたページを返さない）。query 数に上限（256）を置いた。
- `crates/docs-sync/src/iroh_sync.rs`: `query_replica_keys` は `limit` が 0 でも replica を開く（`MemoryDocsSync` と同じ挙動。T2 の監査の non-blocker）。
- `crates/app-api/src/service/replica_window.rs`（新規）: `ensure_index_entries_projected`、`reconcile_timeline_range_checked`、`reconcile_thread_checked`、`RangeCheckLedger`。
  照合の結果（`RangeReconcile`）は、今回反映した件数と、索引の範囲のうち projection に在る件数を持つ。
- `crates/app-api/src/timeline.rs`: `list_timeline_scoped` と `list_thread` の全件走査を、ページの範囲の照合へ置き換えた。
  private channel の現在 epoch に投稿が無い間、取得のたびに scope を全件走査していた経路も、間隔つきの照合になった。
- `crates/app-api/src/service/object_hydration.rs`（新規）: object を 1 件 key 指定で反映する関数群（`hydrate_object_in_topic_with`）。本文の取り方を `BodyFetch` で受け取る
  （`LocalOnly` は手元の本文だけ、`Bounded` は `MissingBodyLedger` の間隔の内でだけ remote を試す）。反映の結果は `ObjectHydration`（`Hydrated`・`Missing`・`Invalid`）で返す。
  以前は、key 指定の反映が台帳を通らず、直後の行単位の取り直しと合わせて同じ本文を 2 回取りに行っていた。
  （1 回目の実装は本文の取り方を docs の読み出しの policy で決めていた。監査の指摘で分けた。下の監査の節。）
- test double: `query_replica_keys` の既定実装はエラーを返すので、app-api の test double に転送を宣言した。

### AC / TR と証跡

| 条件 | 証跡 |
| --- | --- |
| AC-1、AC-2、TR-5 | `older_page_is_filled_from_the_time_index_with_a_bounded_number_of_reads`（300 件と 1,500 件で、読む docs の record 数が同じ。prefix の読み出しは 0 回）、`head_page_reads_only_the_newest_entries`、`timeline_and_thread_pages_are_built_without_any_replica_scan` |
| AC-7 の前提（同じ仕事を繰り返さない） | `the_same_range_is_not_checked_again_within_the_interval`、`range_checks_are_spaced_per_range_and_bounded` |
| INVAR-1 | `reconcile_applies_a_withdrawal_that_was_never_delivered_as_an_event`、既存の取り下げの test |
| INVAR-2、TR-12 | `private_channel_range_is_not_read_without_membership`、既存の private channel の test |
| thread | `thread_reconcile_fills_missing_replies_without_scanning`、`thread_index_keys_are_parsed_and_malformed_keys_are_dropped` |
| T3 の監査の non-blocker(取り下げの中の読み出しの policy) | `user_operation_does_not_wait_for_a_remote_fetch_inside_the_withdrawal_check`(対象の envelope の読み出しを `LocalThenRemote` に固定する mutation で失敗することを確認した) |
| TR-13、docs-sync の読み出し | `window_skips_future_entries_with_a_bounded_number_of_queries`、`malformed_index_keys_do_not_make_the_walk_skip_valid_entries`、`zero_limit_key_query_opens_the_replica_on_both_implementations`、`walking_older_entries_uses_a_bounded_number_of_bounded_queries`（1 回の query が返す key 数の上限を assert に足した） |

### 未確認・残課題

- TR-6（遡った範囲の提供 peer が不在のとき、取得できない旨を示す）の画面側は T5b で扱う。この段階では、本体が手元に無い entry は飛ばし、5 秒以上の間隔で照合し直す。
- 索引の key は、その replica に書ける誰もが置ける。有効な形の偽の key や、形の違う key を大量に置かれると、その古い側へ遡るのに照合を何度も重ねる必要がある（1 回の照合と 1 回の遡りの読み出しには上限がある。best effort）。
- 512 件を超える thread は、超えた分を照合しない。
- 先頭ページの途中の欠け（projection が尽きていないページ）は、この段階では購読タスクの全件走査が埋める。T4 で窓の追いつきに置き換える。
- `list_thread` は、最初に channel を絞らずに projection を読む（以前から同じ）。退出した private channel の thread の行が projection に残っていると、その行を表示しうる。
  照合は参加状態の確認を通らない channel の replica を読まない。表示側の扱いは別 Issue の候補。

### 独立監査の 1 回目（対象 `01a3faba`、FAIL）と修正

1 回目の独立監査は、blocker 1 件で FAIL だった。監査が失敗する test で再現し、その test を恒久の回帰 test として取り込んだ。

| 指摘 | 原因 | 修正 |
| --- | --- | --- |
| 投稿として読めない `objects/<id>/state` と、それを指す索引の entry が 1 件ずつあるだけで、行のあるページの `list_timeline` / `list_thread` が失敗を返す（基準 commit では成功。台帳の再試行の間隔で失敗が繰り返す） | 照合が、1 件の反映の失敗（header の parse の失敗）をページ全体の失敗として返していた | 投稿の header として読めない state は `ObjectHydration::Invalid` として warn を出して読み飛ばす。docs と projection の読み書きの失敗はエラーのまま返す（ADR 0052 §2）。回帰 test `unreadable_state_record_does_not_fail_a_non_empty_head_page`・`…_cursor_page`・`…_thread` |

同じ監査の non-blocker と、監査の再現から分かった弱点のうち、この段階で直したもの。

- 反映できない entry（本体が届いていない、投稿として読めない）が 1 ページぶん続くと、その先の投稿へ遡れなかった（cursor は projection の行でしか進まないため。以前の全件走査は、反映できる object をすべて反映していた）。
  照合は、反映できない entry を読み飛ばして先へ進み、projection に在る object が 1 ページぶんに届くまで読む。1 回に受け取る索引の entry 数は上限（200 件）のままで、届かなかったときは読み進めた位置を台帳に残し、次の照合が続ける。
  回帰 test `a_run_of_unresolvable_entries_does_not_block_older_history`、`a_long_run_of_unresolvable_entries_is_passed_over_several_bounded_checks`（読み飛ばしを無効にする mutation で失敗することを確認した）。
- 本文の取り方を、docs の読み出しの policy から分けた（`BodyFetch`）。1 回に多数の object を反映する照合は手元の本文だけを読み、object を 1 件ずつ反映する経路（docs の event・hint、利用者の操作の対象、community index の解決）は、
  台帳の内で remote を試す。1 回目の実装は `LocalOnly` の key 指定の反映がすべて本文を取りに行かなくなり、community index の解決と bookmark の内容が本文の無いままになりえた（監査の指摘）。
  回帰 test `community_index_resolution_shows_the_body_of_an_unprojected_post`、`reconcile_does_not_fetch_a_body_blob_that_is_not_local`、`event_hydration_and_the_following_listing_share_the_missing_body_schedule`。
- 空の索引を 1 回見た先頭の範囲が 30 秒の間隔に入り、投稿が同期された直後のページの復旧が遅れた。索引が空の先頭の範囲は、間隔を空けない。回帰 test `an_empty_head_range_is_checked_again_on_the_next_listing`。
- 取り下げの対象の envelope がまだ手元に無い行は、「まだ反映できない」として短い間隔で照合し直す。
- test で固定されていなかった主張に test を足した: 遡ったページは projection が尽きていなくても欠けを埋める、先頭のページは projection が尽きていれば行があっても照合する、thread はページが空でなくても照合する、
  1 回の照合の上限（200 件、thread は 512 件）、照合は remote を待たない、退出した private channel の replica を読まない、窓の読み出しの境界（`not_after` ちょうどの秒）、遡りの query 数の上限。
- `hydration_limits.rs` の陳腐化した comment、inventory の段階の表記（T5 → T5b）、ADR 0052 §2 の契機（private channel の現在 epoch）と上限の記述を直した。
- file の行数の上限（1,000 行）を守るため、object を 1 件 key 指定で反映する関数群を `service/object_hydration.rs` へ分けた。

### 独立監査の 2 回目（対象 `e09f6118`、FAIL）と修正

2 回目の監査（delta `01a3faba..e09f6118`）は、1 回目の blocker の解消と、読み飛ばしの loop・本文の取り方・関数の移動を確認したうえで、同じ種類の入力が 2 つ残っているとして FAIL だった（どちらも `01a3faba` の時点からあり、1 回目の監査で見落とされていた）。

| 指摘 | 原因 | 修正 |
| --- | --- | --- |
| 索引の prefix の下に UTF-8 でない key が 1 件あるだけで、行のあるページの取得が失敗を返す（public topic の namespace の secret は replica id から決まるので、誰でも書ける） | `IrohDocsSync::query_replica_keys` が、UTF-8 でない key を読み出し全体の失敗にしていた | docs-sync の読み出しの層の問題として #1253（PR #1255）が直した（UTF-8 でない key の entry を飛ばす）。この PR は main を取り込んでそれを含む。飛ばした entry が「尽きたか」の判定を狂わせる点は、#1257 としてこの PR で直した（下の節） |
| 署名の正しい取り下げで `generation` が符号つき 64 bit に収まらないと、projection への書き込みが失敗し、その object を含む範囲の取得が失敗を返す | 検証は `generation` が 0 でないことしか見ておらず、保存のときの変換の失敗が取得まで伝わっていた | 保存できない `generation` の取り下げは `PostWithdrawalHydration::Invalid` とする。回帰 test `withdrawal_with_an_oversized_generation_does_not_fail_a_non_empty_head_page` |

監査は、照合の call tree の失敗源を iroh と sqlite の実装で全件たどり、投稿者の data が到達する失敗源はこの 2 つだけだと確認している。

同じ監査の non-blocker のうち、この段階で直したもの。

- 反映できない entry が残ったまま何も反映できない照合が続く範囲は、照合の間隔を 5 秒から 2 倍ずつ伸ばす（上限 5 分）。監査の計測では、実体の無い索引の key が 1 件あるだけで、5 秒ごとに exact query 約 100 回の照合が続いていた。
- test で固定されていなかった主張に test を足した（`range_reconcile_ledger.rs`）: 反映できない entry が残った範囲は短い間隔、欠けの無い範囲は長い間隔、取り下げの対象の envelope が未着の行は短い間隔、
  読めない entry は「反映済み」に数えない（数える mutation で失敗することを確認した）、docs の読み出しの失敗はエラーとして返す。
- ADR 0052 §2 の「索引が空の先頭の範囲」の記述と、1 回の照合が読む key 数の上限の記述を、実装に合わせて直した。§4 に、利用者の操作の対象の本文の取り方を書いた。

### T4・T5b への申し送り（2 回目の監査から）

- 読み進めた位置（`resume_before`）が残っている間の照合は、その位置より古い側だけを読む。`before` と `resume_before` のあいだの entry は、その範囲が `before` から読み直されるまで拾われない（1 ページぶんを確かめ終えると、次の照合は `before` から読み直す）。
- 反映できない entry が 1 回の上限（200 件）を超えて続く範囲は、同じ cursor の取得を重ねる必要がある。現行の画面は、`next_cursor` が null で返ったページを再取得しないので、画面からは先へ進めない。
  この段階では購読タスクの全件走査が projection を埋めるので、顕在化しにくい。T5b で TR-6 の画面側（取得できなかった範囲の表示と再取得）と合わせて扱う。
- 同じ `objects/<id>/state` に、docs 著者 id の小さい読めない record を置かれると、key 指定の経路は先頭の 1 件しか見ないので、正しい投稿が反映されない（基準 commit でも、event の経路はエラーになり、全件走査は全体が失敗していた）。
  T4 で、parse できる最初の entry を使う形を検討する。

### main（#1248 / PR #1249）の取り込み

`bed95a98`（投稿の反映で署名つき envelope と replica を確かめる。この段階の作業中に切り出した task が #1248 になったもの）を取り込んだ。

- 投稿の反映は `objects/<id>/state` を読まず、署名つき envelope から行を作る形になった。object を 1 件 key 指定で反映する関数は `hydrate_object_in_topic`（topic を受け取る）になり、
  この PR の `object_hydration.rs` は、その上に `BodyFetch`（本文の取り方）と `ObjectHydration`（反映の結果の内訳）を載せる形に作り直した。
  envelope が未着の object（`Missing`）と、record はあるが検証に通らない object（`Invalid`）を区別するため、`post_integrity.rs` に `load_post` / `PostLoad` を足した（`load_verified_post` はその wrapper）。
- 同じ key に複数の docs 著者の record があるとき先頭だけを見る問題（T3 の監査の申し送り）は、#1248 の `query_replica_exact_bounded` で解消済み。
- test: 行は envelope から作られるので、時系列の並びを固定する test は、作成時刻を指定して署名した envelope を使う（`kukuri_core::sign_envelope_json_at` を公開した）。
  「読めない record」の test は、読めない record を `envelope` の key に置く形に直した。#1248 の test のうち、`list_timeline` が空の projection から全件走査することに頼っていた 1 本は、
  購読タスクが使う全件走査（`hydrate_subscription_state`）を直接呼ぶ形に直し、その test double に `query_replica_keys` の転送を足した。

### 取り込みの後に直したもの

- `late_joiner_backfills_timeline_from_docs`（desktop-runtime）が失敗した。ページの範囲の照合は本文の blob を remote から取らないので、後から参加した利用者のタイムラインに、
  投稿が本文の無いまま先に現れる（以前の全件走査は、本文を 1 件ずつ待ってから返していた）。`recover_missing_bodies` が、取りに行き始めた本文を 300 ms だけ待ってから view を作る形にした
  （件数に依存しない上限。過ぎた取得は背景で続く）。test は、本文が入るまで待つ形に直した。
- 購読タスクが同じ範囲を先に反映すると、照合の反映件数が 0 になり、最初に読んだ古い（空の）ページをそのまま返していた。照合の結果に「索引の範囲のうち projection に在る件数」を持たせ、
  それが最初のページの行数より多いときだけページを読み直す。照合のたびに必ず読み直す形は採らない（ページの読み出しに Q-1・Q-2 の問題が残っているため）。
  test `the_page_is_read_again_only_when_the_projection_holds_more_than_the_page_showed`。

### #1253 の取り込みと、#1257 の対応（この PR で合わせて扱う）

main の `150ca5f7`（#1253 / PR #1255）を取り込んだ。`IrohDocsSync` の読み出しは、UTF-8 でない key の entry を飛ばすようになった（飛ばした分は読み足さない）。

#1257（利用者の指示で、この PR で合わせて対応）: 飛ばした entry は返る件数に入らないので、この PR の `time_index.rs` が「返った件数が要求した件数に達したか」で行っていた
「打ち切られたか」の判定が働かなくなる。UTF-8 でない byte は数字より後ろに並び、降順の読み出しでは必ず先頭に来るので、UTF-8 でない key が余裕を超えて新しい側を埋めるだけで、
窓の読み出しは 0 件を返し、遡りは埋まった prefix の有効な entry を飛ばしていた。

- `DocsSync::query_replica_keys` の戻り値を `DocKeyPage { entries, reached_limit }` にした。`reached_limit` は「query が `limit` 件の entry を読んで打ち切られたか」で、飛ばした entry も数える。
  `IrohDocsSync`・`MemoryDocsSync`・`ReloadableDocsSync`（転送）・test の double の全実装が、同じ意味で返す。trait の既定実装は従来どおりエラーを返す（黙って「打ち切られていない」を返さない）。
- `query_time_index_window` と `query_time_index_desc` は、`reached_limit` で判定する。UTF-8 でない key の entry は、形の違う key と同じ余裕・読み直しの対象になる。query 数の上限（256）は変えていない。
- 再現: `non_utf8_keys_filling_the_newest_side_do_not_hide_valid_entries_from_the_window`（有効な entry 5 件 + UTF-8 でない key 80 件で、窓が 5 件を返す）と
  `non_utf8_keys_do_not_make_the_walk_skip_valid_entries`。判定を「返った件数」へ戻すと、前者は `[]` を返し、後者は古い側の entry だけを返して失敗することを確認した。
  実 iroh-docs を通す結合 test `non_utf8_keys_on_the_newest_side_of_the_time_index_do_not_hide_the_timeline`（feature `iroh-integration-tests`）も足した。
- `query_replica_keys` の caller を再列挙した（`rg "query_replica_keys\(" crates`）: `time_index.rs`（4 か所）、`replica_window.rs` の thread の照合（1 か所）、`ReloadableDocsSync` の転送。
  thread の照合は、返った件数を「尽きたか」の判定に使っていない（古い側から最大 512 件を読むだけ）。

| #1257 の条件 | 証跡 |
| --- | --- |
| AC-1、TR-2、TR-3 | `non_utf8_keys_filling_the_newest_side_do_not_hide_valid_entries_from_the_window`（未来の時刻の有効な entry も混在） |
| AC-2、AC-3、TR-4、TR-5 | `non_utf8_keys_do_not_make_the_walk_skip_valid_entries`、`walk_stops_at_the_query_cap_and_returns_a_contiguous_head`（query 数の上限） |
| AC-4 | `key_query_respects_prefix_order_and_limit_on_both_implementations`（memory と iroh が同じ意味で `reached_limit` を返す）、`non_utf8_key_does_not_fail_prefix_reads`（飛ばした entry も数える）、`reloadable_docs_sync_forwards_bounded_key_queries` |
| INVAR-1 | `crates/docs-sync/src/tests/time_index.rs` の既存 test（無変更の期待で成功） |
| INVAR-2 | `non_utf8_key_does_not_fail_prefix_reads`（#1253 の test。期待は無変更）、`non_utf8_key_does_not_stop_the_timeline` |
| INVAR-3 | `key_query_respects_prefix_order_and_limit_on_both_implementations`（返る件数は `limit` 以下） |

### #1250（PR #1256）の取り込み

main の `515d8c18`（取り下げの key に不正な record が先にあっても、著者の取り下げを反映する）を取り込んだ。PR #1247 へのコメント（#1250 の INV-6）の依頼に対応する。

- 取り下げの反映は `post_withdrawal_hydration.rs` へ移り、key を指定して読む入口は `hydrate_post_withdrawal_for_object`（同じ key の record を上限つきで複数調べる）を通す規則になった。
  この PR の入口 2 つを載せ替えた: ページの範囲の照合が projection にある行の取り下げを確認する箇所（`replica_window.rs` の `ensure_index_entries_projected`）と、
  object を 1 件反映するときの取り下げの先読み（`object_hydration.rs` の `hydrate_object_in_topic_with`。main の `hydrate_object_in_topic` と同じ形）。どちらも先頭の 1 件（`.into_iter().next()`）を見なくなった。
- 保存できない `generation` の取り下げを取り下げとして扱わない修正（この PR）は、`post_withdrawal_hydration.rs` の `storable_post_withdrawal` として載せ直した。
  record 単位の入口（全件走査）と、key 指定の入口の両方に効く。key 指定の入口は、保存できない取り下げを飛ばして残りの候補を調べる。
- 回帰 test `range_reconcile_applies_the_withdrawal_behind_invalid_records`（`withdrawal_record_selection.rs`）。照合の確認を「先頭の 1 件だけを読む」形へ戻すと失敗することを確認した。

### 全体の独立監査（対象 `7a6c3bd1`、PASS）と、T4 の前提条件

#1248・#1253・#1257・#1250 を取り込んだ後の PR 全体を、別コンテキストの監査人が監査し直した。blocker 0 件で PASS。non-blocker のうち、次の 3 件は T4（窓の追いつき）が同じ関数を使う前に直す。

- 遡りの読み出しが query 数の上限（256）に達すると、`reconcile_replica_timeline_range` は「有効な entry の件数が要求に満たない」を「索引は尽きた」とみなし、読み進めた位置を台帳に残さない。
  `query_time_index_desc` が「上限に達して打ち切った」を返し、照合が `record_resume` へつなぐ形にする。
- thread の索引の読み出し（古い側から 512 件）には、形の違う key・UTF-8 でない key への余裕と読み直しが無い。
- 照合は、非表示の著者の行も「projection に在る」と数える。ページの読み出しは非表示の著者の行を読み飛ばすので、非表示の著者の投稿が多い範囲では、照合のたびにページを読み直す。T5b の Q-2（読むページ数の上限）と合わせて扱う。

その他の non-blocker（この段階では直さない。T4・T5b で扱う）。

- test で固定されていない主張: ページを読み直す条件は単体 test だけで固定している（`list_timeline` を通した test は無い）。docs の event の経路が `MissingBodyLedger` を通ること、300 ms の待ちが上限として働くことは、直接の assert が無い。
- 欠けの無い定常状態でも読み直す場面が残る: 1 ページを超える thread、scope に複数の replica がある範囲、非表示の著者の行がある範囲。
- 先頭の範囲が形の違う key で埋まっていると、「索引から 1 件も読めなかった」として間隔を空けずに、取得のたびに最大 257 回の query を発行する。読み進めた位置が残っている間は、間隔を伸ばさない。

### #1252（PR #1259）の取り込み

main の `d40c1ea6`（reaction・live session・game room の反映で署名と replica を確かめる）を取り込んだ。

- `hydration_support.rs` は main の内容を土台にし、この PR の差分（`fetch_projection_blob_text_bounded` を `hydration_limits.rs` へ、投稿の key 指定の反映を `object_hydration.rs` へ移したこと）だけを載せ直した。
  reaction の反映は main の `reaction_hydration.rs`、検証は `reaction_integrity.rs`・`session_integrity.rs` にある。
- #1252 の test `honest_reaction_is_counted_by_another_viewer` が失敗した。空の projection の viewer が最初の `list_timeline` を呼び、その戻り値で投稿の reaction が数えられていることを確かめる test で、
  main では空ページの全件走査（取得の中で完了を待つ）が reaction も反映していた。この PR の照合は投稿しか反映しないので、最初のページには投稿が reaction 0 件で現れ、reaction は購読タスクの起動時の全件走査（背景）が終わるまで入らなかった（着手前の洗い出しで「購読タスクの全件走査が覆う」とした対象が、
  最初のページでは覆われていなかった）。
- 修正: 照合が新しく反映した投稿について、その投稿の reaction を上限つきで反映する（`hydrate_reaction_cache_for_target_bounded`、1 投稿あたり `RANGE_CHECK_REACTIONS_PER_OBJECT` = 32 件）。
  読むのは `reactions/<target>/` の key だけの上限つきの一覧 1 回（64 key）と、見つかった reaction ごとの envelope の key（#1252 の検証。上限つき 8 record）で、対象の reaction の総数にも replica の総件数にも依存しない。
  検証に通らない reaction は飛ばす（warn）。docs と projection の読み書きの失敗は、エラーとして返す。投稿 1 件につき 1 回（projection に入るとき）だけ走る。
- 回帰 test `newly_reconciled_post_brings_a_bounded_number_of_its_reactions`: reaction が 40 件の投稿でも 128 件の投稿でも、照合が反映する reaction は 32 件で、
  照合が発行する docs の読み出しの回数が同じ。上限を外す mutation で失敗することを確認した（40 件が反映される）。
- 残る範囲（T4 へ申し送り）: 上限を超える reaction と、projection に既にある投稿の reaction は、照合では反映しない。#1252 の migration は reaction の行を消して手元の docs から反映し直す前提で、
  いまは購読タスクの全件走査（S-1〜S-5）がそれを担っている。T4 で全件走査を外すときは、窓の object の reaction を上限つきで読み直す経路が要る。

### 検証

`cargo xtask rust-test`（nextest と doc test: 成功。件数は PR の本文に記録）、`cargo test -p kukuri-app-api --lib`（成功。件数は PR の本文に記録）、`cargo test -p kukuri-docs-sync --lib`（成功）、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`cargo xtask oversized-files`（いずれも成功）。

## T4a: 購読タスクの全件走査を外す前に直す事項（基準 commit `14c8c596`）

T5a は PR #1247（merge commit `14c8c596`）で完了した。独立監査は 4 回（FAIL、FAIL、PASS、delta PASS）、必須 CI は全 job 成功。#1257 も同じ PR で完了した。

T4（購読タスクの全件走査の置き換え）は 2 つに分ける。T4a は、T5a の独立監査が「全件走査を外す前に直す」とした事項で、取得側の照合だけを変える。
T4b で、購読タスクの全件走査（S-1〜S-5）を窓の追いつきへ置き換える。いまは全件走査が projection を埋めるので、T4a の事項は利用者には現れない。全件走査が無くなると、
照合で届かない投稿へは二度と届かなくなる。

### 修正前の再現

| 事項（監査の non-blocker） | 再現 test | 修正前の結果 |
| --- | --- | --- |
| 遡りが query 数の上限に達すると、照合が「索引は尽きた」と誤判定する | `a_walk_stopped_at_the_query_cap_is_continued_by_the_next_reconcile`（2 件の投稿のあいだに、形の違う key で埋めた秒を 40 個置く） | 続きの起点を無視する mutation で、12 回照合しても古い投稿へ届かない |
| thread の索引の読み出しに、形の違う key への余裕と読み直しが無い | `malformed_keys_at_the_head_of_a_thread_index_do_not_hide_the_replies`（数字より前に並ぶ key を 600 件） | 索引の全体を細かい prefix へ分けない mutation で、反映が 0 件 |
| 先頭の範囲が形の違う key で埋まっていると、間隔を空けずに取得のたびに読み直す | `a_head_range_buried_under_malformed_keys_is_not_read_on_every_listing` | 以前の条件へ戻す mutation で、2 回目の取得も同じ回数の読み出しを発行する |
| reaction の読み出しの主張が test で固定されていない（4 回目の監査の 2・3・4） | `reactions_are_read_only_when_the_post_enters_the_projection`、`reconcile_does_not_wait_for_a_remote_reaction_envelope`、`reaction_key_listing_returns_a_bounded_number_of_keys`（監査の再現 test を取り込んだ） | それぞれ、毎回読む・remote へ出す・一覧の上限を外す mutation で失敗する |

### 変更の要約

- `crates/docs-sync/src/time_index.rs`: 索引の読み出しを、向きを受け取る 1 つの実装（`walk_time_index`）にまとめた。
  - 古い順の読み出し `query_time_index_asc` を足した。
  - 戻り値を `TimeIndexPage { entries, resume, queries }` にした。`resume` は、query 数の上限に達して `limit` 件に届く前に打ち切ったときの「続きの起点」。
    `queries` は、その読み出しが発行した query の数。
  - entry の無い範囲へ出たら、索引の端を逆向きの読み出し 1 回で調べ、端より先の prefix を読まない。索引に有効な entry が 1 件も無ければ、そこで止める。
    以前は、entry が尽きた後も残りの桁の prefix を空振りで読んでいた（新しい順で数十回。古い順は 100 回を超える）。
  - 索引の時刻は 20 桁の数字だけとした。符号つきの表記（`+…`）は、以前は entry として読めていた。
  - 起点の無い新しい順の読み出し（`query_time_index_desc` に `None`）も、形の違う key への余裕と読み直しを持つようになった。
- `crates/app-api/src/service/replica_window.rs`: タイムラインと thread の照合を、1 つの実装（`reconcile_replica_index_range`）にまとめた。
  - 索引の読み出しが `resume` を返したら、件数が足りなくても「尽きた」とみなさず、その位置を台帳に残して次の照合が続ける。
  - thread は、取得するページの範囲（cursor より新しい側を `limit` 件）だけを照合する。以前は、どのページの取得でも、古い側の 512 件を読んでいた
    （512 件を超える thread の先は照合されず、1 ページを超える thread は「projection に在る件数がページの行数より多い」ので、照合のたびにページを読み直していた）。
  - 間隔を空けずに次も読むのは、索引から 1 件も読めず、読み出しが query 1 回で済んだ先頭の範囲だけにした。
- `crates/app-api/src/timeline.rs`: `list_thread` が、ページの cursor と件数を照合へ渡す。

### 残した事項（T4b・T5b で扱う）

- `reactions/<target>/` の下に、正しい reaction より先に並ぶ key を 64 個置かれると、照合はその投稿の正しい reaction を反映できない（4 回目の監査の 1）。
  reaction の反映の全体（購読タスクの全件走査が担っている分、`Lagged` の後の読み直し、表示の経路での読む量の上限）と合わせて、T4b で設計する。
- 読み進めた位置が残っている間の照合は、間隔を伸ばさない（5 秒）。読み進めるたびに位置が先へ進むので、同じ仕事の繰り返しにはならない。
- 非表示の著者の行を「projection に在る」と数える点は、T5b の Q-2 と合わせて扱う。

### 独立監査（対象 `602658c8`、PASS）と、取り込みで直したもの

blocker 0 件で PASS。main の `d22ce89c`（#1258 / PR #1264: 索引の entry に docs author を持たせる）の取り込みで head が変わるので、同じ delta で non-blocker を直した。

- N-1: 古い順の読み出しが、符号つき 64 bit に収まらない時刻の範囲（有効な entry は在りえない）で query 数の上限に達すると、続きの起点が `i64::MAX` に張り付き、
  thread の最後のページの照合が毎回 257 query を読んで「確かめ終えた」にならなかった。その範囲の prefix は読まず、古い順ではそこで索引は尽きたとする。
  回帰 test `malformed_keys_beyond_the_valid_time_range_do_not_keep_the_thread_tail_unfinished`、`ascending_resume_beyond_i64_max_makes_progress`（監査の再現 test）。
- N-2: 続きの起点の向き・細分化の積む順を固定する test が無かった。監査の乱数 test（基準実装との突き合わせ。memory と iroh）を `time_index_random.rs` として取り込んだ。
- N-4: 「sort key の object id と末尾の object id の一致」の assert を docs-sync の test へ足した。
- N-5: 索引の端の読み出しも query 数の上限の内に収めた（1 回の読み出しは、同じ秒の 1 回 + 256 回以下）。inventory に `query_time_index_asc` を足した。
- main の取り込み: thread の索引の key の分解は docs-sync の `parse_entry` に寄せてあるので、#1258 の docs author は thread の照合にもそのまま渡る（main の `thread_index_entry` の変更は不要になった）。

## T4b-1: docs の購読の通知（基準 commit `833ce8cd`）

T4a は PR #1265（merge commit `833ce8cd`）で完了した。独立監査は 2 回（PASS、delta PASS）、必須 CI は全 job 成功。

T4b（購読タスクの全件走査の置き換え）は大きいので、挙動を変えない追加を先に分ける。この段階は、購読タスクにはまだ触れない。

- PR #1265 の監査の non-blocker: 索引の entry の docs author（#1258）が、索引の読み出しの層を通って照合へ渡ることを固定した
  （`reconcile_passes_the_index_entry_docs_author_to_the_envelope_read`。`parse_entry` が docs author を捨てる mutation で失敗することを確認した）。恒久化した監査の test の `println!` と topic 名を直した。
- docs-sync: `ReplicaNotice { Entry, SyncFinished, ContentReady, Lagged }` と `DocsSync::subscribe_replica_notices` を足した。
  docs の event は buffer（256 件）を通り、溢れた分は `item.ok()` で黙って捨てられていた。1 投稿は 4 entry なので、まとまった同期では必ず溢れる。
  購読側は取りこぼしを知る手段が無く、replica の全件走査がそれを覆っていた。iroh-docs の `SyncFinished`・`PendingContentReady` も捨てていた。
  - `IrohDocsSync`・`MemoryDocsSync` は、同じ buffer から通知を流す。`subscribe_replica`（cn-indexer も使う）は従来どおり entry だけを返す。
  - `ReloadableDocsSync` は転送を宣言した（宣言が無いと既定実装に落ち、取りこぼしが購読側へ届かない）。
  - test: `overflow_is_reported_as_lagged_on_memory_docs`・`…_on_iroh_docs`、`default_notices_carry_entries_only`、`reloadable_docs_sync_forwards_replica_notices`。
  - 未確認: `SyncFinished`・`ContentReady` が実際の 2 node の同期で届くことは、購読タスクがそれを使う段階（T4b-2）の結合 test で確かめる。この段階では本番の caller が無い。
- `crates/desktop-runtime/src/stack.rs` が 1,000 行を超えたので、docs sync の転送の test を `tests/reloadable_docs_sync.rs` へ移した（内容は同じ）。

## T4b-2: 購読タスクの全件走査の置き換え（基準 commit `9fe399fa`）

T4b-1 は PR #1266（merge commit `9fe399fa`）で完了した。独立監査は PASS、必須 CI は全 job 成功。

### 着手前に洗い出した、購読タスクの全件走査（S-1〜S-5）が覆っていた対象

| 走査が覆っていた対象 | 同期 / 背景 | 置き換え後 |
| --- | --- | --- |
| 購読の開始前に手元へ入っていた投稿・取り下げ（窓の範囲） | 背景（購読タスクの起動時） | 起動時の窓の追いつき（`LocalOnly`。窓の object の取り下げも確かめる） |
| 同、窓より古い範囲 | 背景 | 反映しない。利用者が遡ったときのページの範囲の照合（T5a・T4a）が反映する（best effort） |
| 取りこぼした event（buffer の溢れ）の投稿・取り下げ・reaction | 背景（recovery tick、hint の miss） | docs の通知 `Lagged` からの追いつき（窓の object の取り下げと reaction も読み直す） |
| 本体が後から届いた entry | 背景（recovery tick の `LocalThenRemote`） | `SyncFinished`・`ContentReady` と、個別反映が 0 件だった event からの追いつき（`LocalThenRemote`） |
| docs の同期より先に届いた hint の対象 | 背景（hint の miss の走査） | 追いつきの依頼。対象が届けば docs の event が反映する |
| reaction（全件） | 背景 | docs の event（key 単位）、投稿が projection に入るときの上限つきの読み出し、取りこぼしの後の窓の object の読み直し。32 件を超える古い reaction は入らない（best effort） |
| live session・game room（全件） | 背景 | 追いつきのたびに、新しい側の固定件数（それぞれ 32 件）を反映し直す。それより古い session は、その state の event が届いたときに入る |
| 通知の起点（`objects/` の全 entry） | 背景（起動時） | 窓の object の key と hash だけ |
| #1252 の migration の後の reaction・session の再反映 | 背景 | 起動時の追いつき（窓の object の reaction、session の固定件数）。窓より古い投稿の reaction は、遡ったときの照合が反映する |

### 変更の要約

- `crates/app-api/src/service/subscription_catch_up.rs`（新規）: 窓の追いつき `catch_up_replica_window`、session の固定件数の反映、追いつきの契機をまとめる `CatchUpSchedule`、
  窓の object だけから作る通知の起点 `snapshot_window_notification_baseline`。
- `crates/app-api/src/service/private_channels_support.rs`: 購読タスクが `subscribe_replica_notices` を購読し、`hydrate_subscription_state` の呼び出し 4 か所（起動時、private channel の doc event の
  fallback、hint の miss、recovery tick）を無くした。`HintRecoveryGate` は削除した。行数は 1,074 行から減った。
- `crates/app-api/src/service/reaction_hydration.rs`: 上限つきの読み出しが、打ち切られたときに reaction id の先頭の 1 文字ごとに読む（PR #1247 の 4 回目の監査の 1）。上限なしの `hydrate_reaction_cache_for_target` は削除した（P-4）。
- `crates/app-api/src/service/profile_docs_support.rs`: `snapshot_object_notification_baseline`（`objects/` の全件読み）を削除した。
- PR #1266 の監査の non-blocker: 2 node の同期で `SyncFinished`・`ContentReady` が届くことの test（監査の test を恒久化）、移した test のコメントと後始末。

### 残した事項

- 全件走査の関数（`hydrate_subscription_state` など）は、`list_live_sessions`・`list_game_rooms`（S-9、T5b）と test が使うので残る。削除は T7。
- author 購読（S-10）と follow の通知の起点は T6。
- 32 件を超える reaction の背景の backfill は入れていない。1 回の照合・追いつきが読む reaction の総数の上限も入れていない（投稿 1 件あたりの上限だけ）。T5b で扱う。

### 独立監査の 1 回目（対象 `e670b80d`、FAIL）と修正

| 指摘 | 原因 | 修正 |
| --- | --- | --- |
| 空振りで伸びた追いつきの間隔（最大 5 分）が、取りこぼしや相手から届いた entry の契機でも縮まらない。静かな public topic では、recovery tick の再 sync のたびに `SyncFinished` が届いて空振りが積み、定常的に間隔が伸びる。その後に取りこぼした新着や取り下げの反映が、数十秒〜5 分遅れる（以前は 3 秒で走査していた） | `CatchUpSchedule` が、契機の種類を区別せずに同じ期限を使っていた。`record_progress` も回数を戻すだけで、決まった期限を縮めなかった | 反映するものがあると分かっている契機（`Lagged`・`ContentReady`・相手から届いた entry と hint の miss）と `record_progress` は、期限を「前回の追いつき + 最小間隔」まで縮める。`SyncFinished` だけは伸びた間隔を待つ。回帰 test `a_lag_after_an_idle_period_is_caught_up_within_the_minimum_interval`（単体）、`a_lag_after_empty_catch_ups_is_reflected_within_the_minimum_interval`（購読タスク。監査の再現 test） |

同じ監査の non-blocker のうち、この段階で直したもの。

- 相手から届いた索引の entry（個別反映が必ず 0 件）が、投稿 1 件ごとに追いつきを依頼していた。指す object が projection に既にあれば依頼しない（`missed_entry_needs_catch_up`）。
  test `a_remote_index_entry_requests_a_catch_up_only_for_an_unprojected_object`（自分が書いた entry で依頼しないことも固定する。`MemoryDocsSync` の event は `source_peer` が無いので、通知を流し込む test double で確かめる）。
- 追いつきが失敗したとき、読み直しの依頼が失われていた。`CatchUpSchedule::restore` で戻す。
- 取りこぼした取り下げが、`Lagged` の後と起動時の追いつきで反映されることの test（監査の test を恒久化）。

残した non-blocker: session と reaction の読み直しを直接確かめる test、追いつきが走っているあいだ event を消費しない点と `refresh_all` の最悪の読み出し回数（約 1 万回。T5b の「1 回の追いつきが読む reaction の総数の上限」で扱う）。

## T5b-1: live / game の一覧の全件走査の置き換えと、全件走査の関数の削除（基準 commit `323be894`）

T4b-2 は PR #1267（merge commit `323be894`）で完了した。独立監査は 2 回（FAIL、delta PASS）、必須 CI は全 job 成功。

- `list_live_sessions`・`list_game_rooms`（S-9）は、行が空のとき（live は viewer が 0 の live session があるときも）scope の全 replica を全件走査していた。
  session の固定件数を key の一覧から反映する形（`catch_up_scope_sessions`）に置き換えた。読むのは、参加状態の確認を通った replica の手元の docs だけで、replica ごとに間隔を空ける。
- session の固定件数の読み方を、id の形に依らないものにした。`live-`・`game-` の prefix の降順（ほぼ新しい順）に加えて、prefix 全体の昇順と降順の固定件数も読む
  （Dome の room の `dome-<hash>` や、それ以外の形の id は時刻順に並ばない。id の形は検証の対象ではない）。
- これで replica の全件走査の本番の caller が無くなったので、関数を削除した（T7 の予定を前倒しした）:
  `hydrate_subscription_state`・`hydrate_topic_state`・`hydrate_scope_projection`・`hydrate_post_withdrawals_from_replica`・`hydrate_object_projection_from_replica`・
  `hydrate_reaction_cache_from_replica`・`hydrate_live_sessions_from_replica`・`hydrate_game_rooms_from_replica`・`hydrate_post_withdrawal_from_record`、`ReplicaScanCache` と `scan_fingerprint`。
  app-api に残る `DocQuery::Prefix` は、inventory の P-8〜P-13（author・profile・Dome・private channel の参加者）だけになった。
- 全件走査に頼っていた test の載せ替え: 通知と検証の契約 test は窓の追いつき（`catch_up_replica_window`）へ、game room の test は session の固定件数の反映（`catch_up_sessions`）へ。
  全件走査の所要時間を測るだけの test（`measure_full_scan_cost`、`#[ignore]`）と、指紋の単体 test は、対象が無くなったので削除した。
  索引の entry を書かない fixture は、追いつきが索引から読むので、索引の entry を足した。
- PR #1267 の監査が「直接の test が無い」とした経路の test を足した（`session_catch_up.rs`）: session の追いつき、取りこぼしの後の reaction の読み直し、hint からの依頼、
  同期の終わりの通知が伸びた間隔を待つこと。

## T5b-2: ページの取得の上限と、索引の範囲の読み出し（基準 commit `7f19158d`）

T5b-1 は PR #1268（merge commit `7f19158d`）で完了した。独立監査は PASS、必須 CI は全 job 成功。

- PR #1268 の監査の non-blocker: 監査の test 4 本（session の一覧の間隔、`game-` の降順、固定件数と読む量、live の「viewer が 0」の分岐）を恒久化し、全件走査の削除の後に残っていたコメントと inventory の列挙方法を直した。
- Q-2（`filtered_timeline_page`・`filtered_thread_page`）: 非表示の著者の行を除いて `limit` 件集まるまで、上限なくページを読み続けていた。読むページ数に上限（4）を置き、届かなかったときは集まった分と読み進めた位置を返す。
  `limit` 件に届いたときの `next_cursor` がページの末尾を指していて、`limit` が 20 未満のときに同じページの残りの行を飛ばしていた点も直した。
  test `hidden_author_rows_are_skipped_with_a_bounded_number_of_pages`、`next_cursor_points_at_the_last_returned_row`。
- Q-1（`crates/store/src/sqlite/projections.rs`）: cursor の条件が `? IS NULL OR created_at < ? OR (…)` の形で、索引の範囲の読み出しにならなかった。行の値の比較にし、cursor の有無で SQL を分けた。
  thread は `ORDER BY CASE …`（root を先頭に置くための並べ替え）が全行の並べ替えを強いていた（1 ページの取得が thread の返信の総数に比例する）。root は最初のページでだけ 1 行引きし、
  返信は `object_thread_cache` の索引の範囲を読む。test `page_queries_are_index_range_reads`（`EXPLAIN QUERY PLAN` に並べ替えの一時的な木が出ないこと）、
  `thread_pages_list_the_root_first_and_every_reply_once`（root の時刻が返信より後でも、全行を 1 回ずつ読める）、`timeline_pages_list_every_row_once_for_each_channel_filter`。
- 照合が非表示の著者の行を「projection に在る」と数えるので、非表示の著者の投稿がある範囲では照合のたびにページを読み直していた。ページが行を除いて作られているときは、今回反映したときだけ読み直す。
- 1 回の照合・追いつきが reaction を読む投稿の数に上限（64）を置いた（PR #1267 の監査の N-3 のうち、最悪の読み出し回数）。test `one_reconcile_reads_the_reactions_of_a_bounded_number_of_posts`。

### 残した事項

- view の生成に残る docs の key 指定の読み出し（V-1: 返信先が projection に無いときの 1 回の `LocalOnly` の読み出し）と、TR-6 の画面側（遡って取得できなかった範囲の表示）は、この段階では変えていない。
  どちらも総件数には依存しない。T7 の統合確認で、Issue の AC との対応を整理する。
- 追いつきが走っているあいだ購読タスクが event を消費しない点は、1 回の追いつきの量を定数で抑えたうえで残している。
- 32 件を超える reaction の背景の backfill は入れていない（best effort。ADR 0052 §2）。

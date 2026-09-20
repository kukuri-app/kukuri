# replica の読み出しの inventory（総件数への依存）

Issue #1239 の inventory。docs の replica を prefix で全件読みしている箇所（`DocQuery::Prefix` / `DocQuery::All`）を全件列挙し、
処理量が何の総数に比例するかと、解消する段階を記録する。設計の正本は [ADR 0052](../adr/0052-scale-independent-timeline-sync.md)、
原則は `AGENTS.md` の「設計原則: 件数に依存しない処理」。

- 基準 commit: `117af820`
- 列挙方法: `grep -rn "DocQuery::Prefix\|DocQuery::All" crates --include=*.rs` から test と `docs-sync` の実装を除く。全件走査の入口は
  `hydrate_subscription_state` / `hydrate_topic_state` / `hydrate_scope_projection` / `hydrate_author_state` の caller を逆引きする。
- 未分類: 0

## 前提: query の実行計画

`docs-sync` は query を既定の並び順（`SortBy::AuthorKey`）で組んでいたため、iroh-docs は `Exact`・`Prefix`・`All` のどれも namespace 全体の table scan として実行していた。
つまり下の表の prefix 読みだけでなく、**key を 1 つ指定した読み出しのすべて**が、replica の総 entry 数に比例していた
（1,000 → 10,000 entry で 2.4 ms → 24 ms。修正後は 0.27 ms → 0.31 ms。計測 test `measure_exact_query_cost`）。
段階 T2（PR #1245）で、すべての query を key の索引（`SortBy::KeyAuthor`）で読む形に直した。これは下の全行に効くが、prefix の全件読みそのものは T3 以降で無くす。

## 全件走査の入口（app-api）

| ID | 入口・契機 | 読む範囲 | 比例する総数 | 解消する段階 |
| --- | --- | --- | --- | --- |
| S-1 | 購読タスクの起動時・再起動時（`private_channels_support.rs` の `hydrate_subscription_state(LocalOnly)`）。再起動は `restart_active_subscriptions`（`set_discovery_seeds`・`import_peer_ticket`・CN 自己修復）で全購読ぶん | 5 prefix の全 entry | topic / channel epoch の投稿・リアクション・取り下げ・session の総数 × 購読数 | T4、T7（一括の再起動の廃止は #1224） |
| S-2 | 購読タスクの起動時の通知の baseline（`snapshot_object_notification_baseline`・`snapshot_follow_notification_baseline`）。結果を task の寿命のあいだ memory に保持する | `objects/` の全 entry、`graph/follows/` の全 entry | topic の投稿総数、author の follow 総数 | T4（topic）、T6（author） |
| S-3 | public topic の recovery tick（最大 30 秒間隔） | 5 prefix の全 entry（`LocalThenRemote`） | 同 S-1 | T4 |
| S-4 | replica の内容を指す hint で個別反映が 0 件（3 秒の最小間隔） | 同上 | 同 S-1 | T4 |
| S-5 | private channel の doc event で個別反映が 0 件（`withdrawals/` などの event） | 同上 | 同 S-1 | T4 |
| S-6 | `list_timeline_scoped`（空ページ、private channel の現在 epoch が未反映）・`list_thread`（空ページ） | scope の全 replica の 5 prefix | 同 S-1 × scope の replica 数 | T5 |
| S-7 | repost・bookmark・reply・取り下げ（`timeline.rs`）、reaction（`reactions.rs`）の実行時。利用者の操作が走査の完了を待つ | 同 S-6 | 同 S-6 | T3（解消済み。`ensure_object_projection` が対象の key だけを読む） |
| S-8 | community index の解決（`community_index.rs`）、repost 元の解決（`resolve_repost_source` → `hydrate_topic_state(LocalThenRemote)`） | 同 S-6 | 同 S-6 | T3（解消済み） |
| S-9 | `list_game_rooms`（行が空）・`list_live_sessions`（行が空、または live で viewer 0。購読再起動と再 sync も行う） | 同 S-6 | 同 S-6 | T5 |
| S-10 | author 購読の起動時と doc event ごと（`social_runtime_support.rs` の `hydrate_author_state(LocalThenRemote)`）、`list_profile_timeline` | author replica の profile・follow・block・投稿・repost の全 entry | author の投稿・repost・follow・block の総数 × 購読中の author 数 | T6 |

## prefix の全件読み（caller ごと）

| ID | caller | prefix | 比例する総数 | 分類 |
| --- | --- | --- | --- | --- |
| P-1 | `hydration_support.rs` `hydrate_post_withdrawals_from_replica`。全件走査のほか、view の生成中に行ごとに呼ばれる（`timeline_view_support.rs` の `profile_post_to_view`・`profile_repost_to_view`・`repost_snapshot_to_view_with_profiles`） | `withdrawals/` | topic の取り下げ総数 × ページの行数 | 対象。view の生成からは T3 で外した（背景の key 指定の確認へ）。関数は T7 で削除 |
| P-2 | `hydration_support.rs` `hydrate_object_projection_from_replica` | `objects/`（1 投稿につき `state` と `envelope` の 2 entry） | topic の投稿総数 | 対象。T7 で削除 |
| P-3 | `hydration_support.rs` `hydrate_reaction_cache_from_replica` | `reactions/` | topic のリアクション総数 | 対象。T7 で削除 |
| P-4 | `hydration_support.rs` `hydrate_reaction_cache_for_target` | `reactions/<target object id>/` | 1 投稿のリアクション数 | 対象。T4 で上限つきの読み出しにする（集計は best effort）。T3 では、自分の reaction の確認を key 指定の読み出しにした |
| P-5 | `hydration_support.rs` `hydrate_live_sessions_from_replica` | `sessions/live/` | topic の live session の総数（終了したものも残る） | 対象。T5 で上限つき、T7 で全件走査から外す |
| P-6 | `hydration_support.rs` `hydrate_game_rooms_from_replica` | `sessions/game/` | topic の game room の総数 | 同上 |
| P-7 | `service/mod.rs` `find_existing_simple_repost`（repost のたび。全 entry を deserialize） | `objects/` | target topic の投稿総数 | 対象。T3 で projection の索引（`find_author_reposts_of`）に置き換えた |
| P-8 | `profile_docs_support.rs` `hydrate_author_state` | `graph/follows/`、`graph/blocks/` | author の follow・block の総数 | 対象。T6（event 駆動と上限つきの読み出し） |
| P-9 | `profile_docs_support.rs` `load_profile_posts_from_author_replica`・`load_profile_reposts_from_author_replica` | `profile/posts/`、`profile/reposts/` | author の投稿・repost の総数 | 対象。T6 |
| P-10 | `profile_docs_support.rs` `load_custom_reaction_assets_from_author_replica` | `reactions/assets/` | author のカスタムリアクションの総数 | 対象。T6 で上限つき |
| P-11 | `profile_docs_support.rs` `snapshot_object_notification_baseline`・`snapshot_follow_notification_baseline` | `objects/`、`graph/follows/` | S-2 と同じ | 対象。T4・T6 |
| P-12 | `object_persistence_support.rs` `fetch_private_channel_participants_from_replica` | `channels/participants/` | private channel の参加者数 | Non-goal（本 Issue の固定 AC に含まれない。招待制で件数は小さいが、上限が無い点を #1224 の台帳の上限で扱う） |
| P-13 | `dome_connections.rs`（4 か所）、`dome_hosting.rs`（3 か所）、`dome_delete.rs`（1 か所） | `metaverse/dome-instances/`・提案・選択・接続・削除・layout commit | topic の Dome と提案の総数 | Non-goal（本 Issue の固定 AC に含まれない。Dome の一覧と接続の読み出しは別 Issue で、同じ原則で見直す） |
| P-14 | `crates/cn-indexer/src/ingest.rs` `ingest_scope`（3 か所。変更通知で対象を特定できないときの fallback と初回） | `objects/`・`withdrawals/` など | scope の投稿総数 | Non-goal（CN 側。`ingest_changed_keys` が通常経路。T2 の索引化は効く。全件走査の廃止は CN 側の Issue で扱う） |
| P-15 | `desktop-runtime/src/runtime/sync_live_api.rs` `has_topic_timeline_doc_index_entry`（test と harness 用） | `indexes/timeline/` | topic の投稿総数 | 対象。T2 で key 指定の読み出し 2 回へ置き換えた |

## 反映の検証が足す読み出し（Issue #1252）

reaction・live session・game room の検証は、prefix の読み出しを足さない。reaction は、P-3・P-4 の同じ prefix の読み出しに含まれる `envelope` の record から行を作る。
live session と game room は、state 1 件につき `envelopes/<envelope id>` を key 指定・上限つき（8 件）で 1 回読む。key 指定の個別反映は、`state` の key も上限つきで読む。
どれも object 1 件あたり定数で、replica の総 entry 数に依存しない。P-5・P-6 の全件走査では、session の総数ぶんの key 指定の読み出しが足される（T5・T7 で走査ごと無くなる）。

## view の生成に残る docs の読み出し（key 指定）

T3 の後も、view の生成の経路に docs の読み出しが 2 か所残る。どちらも key を 1 つ指定した `LocalOnly` の読み出しで、replica の総 entry 数には依存しない
（T2 の索引化の後）。ただし ADR 0052 §2 の「view の生成中に docs を読まない」に反するので、T5 で外す。
このうち V-2 は Issue #1248 で削除した（残りは V-1 の 1 か所）。repost 元の解決（`resolve_repost_source`）と bookmark（`bookmark_post_in_channel`）が
`objects/<id>/state` を読み直していた箇所も、#1248 で検証済みの projection の行を使う形にして、docs の読み出しを無くした。

| ID | 箇所 | 読む範囲・契機 | 比例する総数 | 分類 |
| --- | --- | --- | --- | --- |
| V-1 | `timeline_view_support.rs` `hydrate_reply_preview_row`（`load_verified_post`） | 返信先が projection に無いときだけ、`objects/<返信先 id>/envelope` を 1 回（`LocalOnly`、最大 8 record）。署名つき envelope と replica の scope を確かめてから反映する（#1248）。反映できれば次回以降は読まない | 依存しない（表示する行ごとに 1 key 以下） | 対象。T5 で、返信先の反映を取得側（窓の追いつき・遡りの取得）と背景へ移す |
| V-2 | `timeline_view_support.rs` `attachment_views_for_projection_row` の fallback | 削除済み（#1248）。署名の無い `state` の添付を表示する経路だった。旧い行（`projection_version < 3`）は migration が消し、docs から反映し直す | — | 解消済み |

## projection 側

| ID | 箇所 | 問題 | 解消する段階 |
| --- | --- | --- | --- |
| Q-1 | `crates/store/src/sqlite/projections.rs` のタイムライン・thread の cursor 条件（`created_at < ? OR (created_at = ? AND object_id < ?)`） | OR 形のため、深いページほど索引の読み飛ばしが増える（遡った深さに比例） | T5（range seek になる形へ） |
| Q-2 | `projection_support.rs` `filtered_timeline_page` / `filtered_thread_page` | 非表示の著者の行を除いて `limit` 件集まるまで、上限なくページを読み続ける | T5（読むページ数に上限） |
| Q-3 | `timeline.rs` `list_profile_timeline` | author の全投稿・全 repost をロードしてソートしてからページを切る | T6 |

## 観察（本 Issue では変えない）

- 同じ key に複数の docs 著者の entry があるとき、caller は `Exact` の結果の先頭（docs 著者 id の昇順で最初）を使う。最新の entry ではない。
  T2 で並びを key の索引に変えても、`Exact` の結果の順序（docs 著者 id の昇順）と、prefix 読みで同じ key の最後に反映される entry は変わらない。
  ただし、投稿の envelope（`objects/<object id>/envelope`、#1248）と取り下げ（`withdrawals/<object id>/state`、#1250）の key 指定の読み出しは、先頭の 1 件を使わず、
  上限つき（key ごとに最大 8 record）で検証に通る最初の record を選ぶ（ADR 0052 §2）。
- repost 元、profile の投稿、profile の投稿の返信先の取り下げの確認は、購読していない topic の replica を開いて同期する（T3 より前から、view の生成のたびに起きていた副作用）。T3 で確認は背景の key 指定になったが、replica を開くこと自体は残る。
  開く replica の上限と、取り下げの置き場所は #1224・#1243 で扱う。
- private channel の取り下げは、現在の epoch の replica に書かれる。対象の投稿が過去の epoch の replica にあると、取り下げを反映する側は同じ replica で対象の envelope を見つけられず、
  取り下げを検証できない（T3 より前から同じ。key 単位の反映でも全件走査でも変わらない）。epoch をまたぐ取り下げの扱いは別 Issue で決める。
- iroh-docs の同期と保存は replica の総 entry 数に比例する（ADR 0052 §7）。replica の時間分割は #1243 が所有する。

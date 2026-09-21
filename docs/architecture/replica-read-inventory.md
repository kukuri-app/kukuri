# replica の読み出しの inventory（総件数への依存）

Issue #1239 の inventory。docs の replica を prefix で全件読みしている箇所（`DocQuery::Prefix` / `DocQuery::All`）を全件列挙し、
処理量が何の総数に比例するかと、解消する段階を記録する。設計の正本は [ADR 0052](../adr/0052-scale-independent-timeline-sync.md)、
原則は `AGENTS.md` の「設計原則: 件数に依存しない処理」。

- 基準 commit: `117af820`
- 列挙方法: `grep -rn "DocQuery::Prefix\|DocQuery::All" crates --include=*.rs` から test と `docs-sync` の実装を除く。全件走査の入口は
  `hydrate_subscription_state` / `hydrate_topic_state` / `hydrate_scope_projection` / `hydrate_author_state` の caller を逆引きした
  （前の 3 つは T5b-1 で削除済み。残る入口は `hydrate_author_state`）。
- 未分類: 0

## 前提: query の実行計画

`docs-sync` は query を既定の並び順（`SortBy::AuthorKey`）で組んでいたため、iroh-docs は `Exact`・`Prefix`・`All` のどれも namespace 全体の table scan として実行していた。
つまり下の表の prefix 読みだけでなく、**key を 1 つ指定した読み出しのすべて**が、replica の総 entry 数に比例していた
（1,000 → 10,000 entry で 2.4 ms → 24 ms。修正後は 0.27 ms → 0.31 ms。計測 test `measure_exact_query_cost`）。
段階 T2（PR #1245）で、すべての query を key の索引（`SortBy::KeyAuthor`）で読む形に直した。これは下の全行に効くが、prefix の全件読みそのものは T3 以降で無くす。

## 全件走査の入口（app-api）

| ID | 入口・契機 | 読む範囲 | 比例する総数 | 解消する段階 |
| --- | --- | --- | --- | --- |
| S-1 | 購読タスクの起動時・再起動時（`private_channels_support.rs` の `hydrate_subscription_state(LocalOnly)`）。再起動は `restart_active_subscriptions`（`set_discovery_seeds`・`import_peer_ticket`・CN 自己修復）で全購読ぶん | 5 prefix の全 entry | topic / channel epoch の投稿・リアクション・取り下げ・session の総数 × 購読数 | T4b-2（解消済み。起動時は窓の追いつき `catch_up_replica_window`）。関数は T5b-1 で削除した（一括の再起動の廃止は #1224） |
| S-2 | 購読タスクの起動時の通知の baseline（`snapshot_object_notification_baseline`・`snapshot_follow_notification_baseline`）。結果を task の寿命のあいだ memory に保持する | `objects/` の全 entry、`graph/follows/` の全 entry | topic の投稿総数、author の follow 総数 | T4b-2（投稿の側は解消済み。窓の object の key と hash だけを読む `snapshot_window_notification_baseline`）。follow の側は T6 |
| S-3 | public topic の recovery tick（最大 30 秒間隔） | 5 prefix の全 entry（`LocalThenRemote`） | 同 S-1 | T4b-2（解消済み。recovery tick は docs を読まず、再 sync を促すだけ） |
| S-4 | replica の内容を指す hint で個別反映が 0 件（3 秒の最小間隔） | 同上 | 同 S-1 | T4b-2（解消済み。追いつきの依頼にした） |
| S-5 | private channel の doc event で個別反映が 0 件（`withdrawals/` などの event） | 同上 | 同 S-1 | T4b-2（解消済み。追いつきの依頼にした） |
| S-6 | `list_timeline_scoped`（空ページ、private channel の現在 epoch が未反映）・`list_thread`（空ページ）。private channel の現在 epoch に投稿が無い間は、取得のたびに走査していた | scope の全 replica の 5 prefix | 同 S-1 × scope の replica 数 | T5a（解消済み。`reconcile_timeline_range`・`reconcile_thread` が、ページの範囲を時系列の索引と照合する） |
| S-7 | repost・bookmark・reply・取り下げ（`timeline.rs`）、reaction（`reactions.rs`）の実行時。利用者の操作が走査の完了を待つ | 同 S-6 | 同 S-6 | T3（解消済み。`ensure_object_projection` が対象の key だけを読む） |
| S-8 | community index の解決（`community_index.rs`）、repost 元の解決（`resolve_repost_source` → `hydrate_topic_state(LocalThenRemote)`） | 同 S-6 | 同 S-6 | T3（解消済み） |
| S-9 | `list_game_rooms`（行が空）・`list_live_sessions`（行が空、または live で viewer 0。購読再起動と再 sync も行う） | 同 S-6 | 同 S-6 | T5b-1（解消済み。session の固定件数を key の一覧から反映する `catch_up_scope_sessions`。replica ごとに間隔を空ける） |
| S-10 | author 購読の起動時と doc event ごと（`social_runtime_support.rs` の `hydrate_author_state(LocalThenRemote)`）、`list_profile_timeline` | author replica の profile・follow・block・投稿・repost の全 entry | author の投稿・repost・follow・block の総数 × 購読中の author 数 | T6-1（購読は解消済み。doc event はその key だけを反映し、起動時・取りこぼし・同期の区切りは `profile/latest` と、follow・block の key の上限つきの一覧（各 512 件）から反映する）。`list_profile_timeline` は T6-2（解消済み。プロフィールの索引からページの範囲と行の key だけを読む） |

## prefix の全件読み（caller ごと）

| ID | caller | prefix | 比例する総数 | 分類 |
| --- | --- | --- | --- | --- |
| P-1 | `hydration_support.rs` `hydrate_post_withdrawals_from_replica`。全件走査のほか、view の生成中に行ごとに呼ばれる（`timeline_view_support.rs` の `profile_post_to_view`・`profile_repost_to_view`・`repost_snapshot_to_view_with_profiles`） | `withdrawals/` | topic の取り下げ総数 × ページの行数 | T5b-1（解消済み。関数を削除した） |
| P-2 | `hydration_support.rs` `hydrate_object_projection_from_replica` | `objects/`（1 投稿につき `state` と `envelope` の 2 entry） | topic の投稿総数 | T5b-1（解消済み。関数を削除した） |
| P-3 | `reaction_hydration.rs` `hydrate_reaction_cache_from_replica` | `reactions/` | topic のリアクション総数 | T5b-1（解消済み。関数を削除した） |
| P-4 | `reaction_hydration.rs` `hydrate_reaction_cache_for_target`（reaction の hint の個別反映） | `reactions/<target object id>/` | 1 投稿のリアクション数 | T4b-2（解消済み。`hydrate_reaction_cache_for_target` を削除し、hint の個別反映も上限つきの `hydrate_reaction_cache_for_target_bounded` を使う） |
| P-5 | `hydration_support.rs` `hydrate_live_sessions_from_replica` | `sessions/live/` | topic の live session の総数（終了したものも残る） | T5b-1（解消済み。関数を削除した） |
| P-6 | `hydration_support.rs` `hydrate_game_rooms_from_replica` | `sessions/game/` | topic の game room の総数 | T5b-1（解消済み。関数を削除した） |
| P-7 | `service/mod.rs` `find_existing_simple_repost`（repost のたび。全 entry を deserialize） | `objects/` | target topic の投稿総数 | 対象。T3 で projection の索引（`find_author_reposts_of`）に置き換えた |
| P-8 | `profile_docs_support.rs` `hydrate_author_state` | `graph/follows/`、`graph/blocks/` | author の follow・block の総数 | 対象。T6-1（解消済み。event の key だけを読む `hydrate_author_key` と、上限つきの `hydrate_author_state`・`catch_up_author_state`） |
| P-9 | `profile_docs_support.rs` `load_profile_posts_from_author_replica`・`load_profile_reposts_from_author_replica` | `profile/posts/`、`profile/reposts/` | author の投稿・repost の総数 | 対象。T6-2（解消済み。関数を削除した。索引の無い replica は、key の上限つきの一覧（各 128 件）から読む） |
| P-10 | `profile_docs_support.rs` `load_custom_reaction_assets_from_author_replica` | `reactions/assets/` | author のカスタムリアクションの総数 | 対象。T6-1（解消済み。key の上限つきの一覧（asset 512 件ぶん）と、`state` の key ごとの読み出し） |
| P-11 | `profile_docs_support.rs` `snapshot_object_notification_baseline`・`snapshot_follow_notification_baseline` | `objects/`、`graph/follows/` | S-2 と同じ | 対象。T4・T6（解消済み。投稿の側は T4b-2。follow の側は、通知の対象になる自分を指す follow の key 1 件の key と hash だけを読む） |
| P-12 | `object_persistence_support.rs` `fetch_private_channel_participants_from_replica` | `channels/participants/` | private channel の参加者数 | Non-goal（本 Issue の固定 AC に含まれない。招待制で件数は小さいが、上限が無い点を #1224 の台帳の上限で扱う） |
| P-13 | `dome_connections.rs`（4 か所）、`dome_hosting.rs`（3 か所）、`dome_delete.rs`（1 か所） | `metaverse/dome-instances/`・提案・選択・接続・削除・layout commit | topic の Dome と提案の総数 | Non-goal（本 Issue の固定 AC に含まれない。Dome の一覧と接続の読み出しは別 Issue で、同じ原則で見直す） |
| P-14 | `crates/cn-indexer/src/ingest.rs` `ingest_scope`（3 か所。変更通知で対象を特定できないときの fallback と初回） | `objects/`・`withdrawals/` など | scope の投稿総数 | Non-goal（CN 側。`ingest_changed_keys` が通常経路。T2 の索引化は効く。全件走査の廃止は CN 側の Issue で扱う） |
| P-15 | `desktop-runtime/src/runtime/sync_live_api.rs` `has_topic_timeline_doc_index_entry`（test と harness 用） | `indexes/timeline/` | topic の投稿総数 | 対象。T2 で key 指定の読み出し 2 回へ置き換えた |

## 段階の順序の変更（T5a を T4 より先に行う）

購読タスクの全件走査（S-1〜S-5）は、窓（新しい側の固定件数）より古い範囲の反映も担っていた。docs の event は broadcast（容量 256）の溢れで取りこぼされ、
まとまった同期（初回の参加、長い離席の後）では、古い範囲のほとんどが全件走査で projection に入る。S-1〜S-5 を先に外すと、窓より古い投稿へ遡れない中間状態ができる。
そこで、取得側の「ページの範囲の照合」（S-6 の置き換え）を T5a として先に入れ、その後に T4（購読タスク）を行う。T5 の残り（live / game、非表示の著者の読み飛ばしの上限、
cursor の条件、V-1・V-2）は T5b とする。

## ページの範囲の照合が読む reaction（T5a）

照合は、新しく反映した投稿 1 件につき、`reactions/<target object id>/` の key だけの上限つきの一覧を 1 回（64 key）読み、見つかった reaction（最大 32 件）の `envelope` の key を
key 指定・上限つき（8 件）で読む。読む量は「1 回の照合が反映する投稿の数（replica 1 つあたり最大 200 件）× 定数」で、対象の reaction の総数にも replica の総 entry 数にも依存しない。
投稿が projection に入るときの 1 回だけで、projection に既にある投稿では読まない。以前は、空のページの全件走査（S-6）が `reactions/` の全 entry を読んでいた。

## 反映の検証が足す読み出し（Issue #1252）

reaction・live session・game room の検証は、prefix の読み出しを足さない。reaction は、上限つきの key の一覧(1 対象あたり reaction 32 件ぶん、key は 64 件まで。上限に達したら reaction id の先頭の文字ごとに 16 回の一覧を足す)で見つけた key ごとに、`envelope` の record を読んで行を作る(P-3・P-4 の prefix の読み出しは削除済み)。
live session と game room は、state 1 件につき `envelopes/<envelope id>` を key 指定・上限つき（8 件）で 1 回読む。key 指定の個別反映は、`state` の key も上限つきで読む。
どれも object 1 件あたり定数で、replica の総 entry 数に依存しない。P-5・P-6 の全件走査では、session の総数ぶんの key 指定の読み出しが足されていた（T5b-1 で走査ごと無くなった）。

## view の生成に残る docs の読み出し（key 指定）

T3 の後も、view の生成の経路に docs の読み出しが 2 か所残る。どちらも key を 1 つ指定した `LocalOnly` の読み出しで、replica の総 entry 数には依存しない
（T2 の索引化の後）。ただし ADR 0052 §2 の「view の生成中に docs を読まない」に反するので、T5b で外す。
V-2 は Issue #1248 で削除し、V-1 は #1239 の AC-6（#1277）で背景の反映へ移した。view の生成は docs を読まない。repost 元の解決（`resolve_repost_source`）と bookmark（`bookmark_post_in_channel`）が
`objects/<id>/state` を読み直していた箇所も、#1248 で検証済みの projection の行を使う形にして、docs の読み出しを無くした。

| ID | 箇所 | 読む範囲・契機 | 比例する総数 | 分類 |
| --- | --- | --- | --- | --- |
| V-1 | `timeline_view_support.rs` の返信先の preview | 解消済み（#1239 AC-6、#1277）。view の生成は projection だけを読む。返信先が projection に無ければ、`reflect_reply_target` を背景へ出し（確認先ごとに 60 秒の間隔、台帳 4,096 件、同時 4 件）、この回の preview は出さない。反映できれば次の取得で出る。画面は preview が無い行の返信先の枠を描かない | — | 解消済み |
| V-2 | `timeline_view_support.rs` `attachment_views_for_projection_row` の fallback | 削除済み（#1248）。署名の無い `state` の添付を表示する経路だった。旧い行（`projection_version < 3`）は migration が消し、docs から反映し直す | — | 解消済み |

## projection 側

| ID | 箇所 | 問題 | 解消する段階 |
| --- | --- | --- | --- |
| Q-1 | `crates/store/src/sqlite/projections.rs` のタイムライン・thread の cursor 条件（`created_at < ? OR (created_at = ? AND object_id < ?)`） | OR 形のため、深いページほど索引の読み飛ばしが増える（遡った深さに比例） | T5b-2（解消済み。cursor の条件を行の値の比較にし、cursor の有無で SQL を分けた。thread は root を 1 行引きして、返信を索引の範囲で読む。query plan の test あり） |
| Q-2 | `projection_support.rs` `filtered_timeline_page` / `filtered_thread_page` | 非表示の著者の行を除いて `limit` 件集まるまで、上限なくページを読み続ける。`limit` が 20 未満のときと非表示の著者があるときは、返す `next_cursor` が行を飛ばす | T5b-2（解消済み。読むページ数の上限 4。`next_cursor` は最後に返した行の位置） |
| Q-3 | `timeline.rs` `list_profile_timeline` | author の全投稿・全 repost をロードしてソートしてからページを切る。`next_cursor` がページの次の行の位置で、次のページがその行を飛ばす | T6-2（解消済み。`indexes/profile/` の索引を cursor から読み、`next_cursor` は最後に返した行の位置。非表示の著者の読み飛ばしは 4 ページまで） |

## 観察（本 Issue では変えない）

- 同じ key に複数の docs 著者の entry があるとき、caller は `Exact` の結果の先頭（docs 著者 id の昇順で最初）を使う。最新の entry ではない。
  T2 で並びを key の索引に変えても、`Exact` の結果の順序（docs 著者 id の昇順）と、prefix 読みで同じ key の最後に反映される entry は変わらない。
  ただし、投稿の envelope（`objects/<object id>/envelope`、#1248）と取り下げ（`withdrawals/<object id>/state`、#1250）の key 指定の読み出しは、先頭の 1 件を使わず、
  上限つき（key ごとに最大 8 record）で検証に通る最初の record を選ぶ（ADR 0052 §2）。
  著者が docs author を示した投稿（#1258、ADR 0053）は、その前に「docs author と key の組」で 1 件読む。同じ key に他の名義の record が何件あっても、読む record は 1 件。
- repost 元、profile の投稿、profile の投稿の返信先の取り下げの確認は、購読していない topic の replica を開いて同期する（T3 より前から、view の生成のたびに起きていた副作用）。T3 で確認は背景の key 指定になったが、replica を開くこと自体は残る。
  開く replica の上限と、取り下げの置き場所は #1224・#1243 で扱う。
- private channel の取り下げは、現在の epoch の replica に書かれる。対象の投稿が過去の epoch の replica にあると、取り下げを反映する側は同じ replica で対象の envelope を見つけられず、
  取り下げを検証できない（T3 より前から同じ。key 単位の反映でも全件走査でも変わらない）。epoch をまたぐ取り下げの扱いは別 Issue で決める。
- iroh-docs の同期と保存は replica の総 entry 数に比例する（ADR 0052 §7）。replica の時間分割は #1243 が所有する。
- UTF-8 でない key の entry は、`IrohDocsSync` の prefix の読み出しと key の一覧（`query_replica_keys`）が飛ばす（#1253）。飛ばした entry は返る件数に入らないので、
  `query_replica_keys` は「`limit` 件を読んで打ち切られたか」を別に返し、時系列の索引の読み出し（`query_time_index_window`・`query_time_index_desc`・`query_time_index_asc`）はそれで「尽きたか」を判定する（#1257。T5a の PR で対応）。

## 完了の確認（T7）

全件走査の入口（S-1〜S-10）と prefix の全件読み（P-1〜P-11、P-15）は、Non-goal（P-12〜P-14）を除いてすべて解消した。
複数の private channel をまたぐページの取得が、許可されない channel の行を読み飛ばす点は #1280 で扱う。
設計上残る、総件数に比例する読み出しは無い。T6 で入れた自分の replica の背景の仕事（自分の follow・block をすべて読む `sweep_own_author_edges` と、
プロフィールの索引の補完 `backfill_own_profile_index`）は、利用者が必要としない全件の読み出しなので外した（AGENTS.md: ユースケース上ユーザーが必要としない
限り同期・復旧はしない。ADR 0052 §6、ADR 0053 §6）。取りこぼし・同期の区切りの後の author の追いつきは、自分を指す follow・block の key だけを読む。
replica と projection の件数を 1,000 / 10,000 / 100,000 にしても、次の操作が docs から読む record と key の数と、projection の読み書きで
SQLite が実行した命令の数が同じであることを、`crates/app-api/src/tests/sync/scale_counts.rs` の test が数で確かめる（所要時間の閾値は使わない。
命令の数は読み書きした行の数とともに増え、B-tree の深さには依存しない。上限を外す mutation と、ページの取得の SQL が索引を使わなくなる mutation で、
量が件数に比例して増えることが検出される）。projection には、時系列の索引の埋め草と同じ object の行を同じ数だけ置く。

| 操作 | 種類 | docs の読む量 | projection の命令の数 |
| --- | --- | --- | --- |
| topic の購読タスクの起動（通知の起点・起動時の窓の追いつきを含む） | 定期処理 | 958 | 17,340 |
| プロフィールを初めて開く（author 購読の起動・通知の起点・起動時の反映を含む） | 表示・定期処理 | 3,192 | 185 |
| 購読タスクの窓の追いつき | 定期処理 | 264 | 15,000 |
| author 購読の追いつき（自分を指す follow・block の key） | 定期処理 | 2 | 13 |
| 新着 1 件の受信（相手の peer から、投稿の entry と本体が届く） | 定期処理 | 3 | 270 |
| タイムラインの新しい側のページ（購読の起動の後。projection から読む） | 表示 | 0（projection が空で初めて開くときの照合は、`range_reconcile.rs` の `head_page_reads_only_the_newest_entries` が 300 件と 1,500 件で読む量が同じことを示す） | 1,703 |
| タイムラインの遡ったページ（埋め草の中ほど） | 表示 | 30 | 3,287 |
| プロフィールのタイムライン | 表示 | 115（測定の author replica には索引の無い旧 record が無い。旧 record のある著者では、`profile/posts/`・`profile/reposts/` の key の上限つきの一覧（各 128 件）と、その行の読み出しが 1 ページごとに加わる。この量も件数によらない） | 45 |
| reaction | 利用者の操作 | 3 | 444 |


# #1225 タイムライン取得と購読タスクの全件走査の有限化（2026-09-20）

- 対象 Issue: #1225（区分 C、Scope revision 2026-09-20-v2）。親の調査は #1221。
- 基準 commit: `9a22c236`。

## 修正前の再現

`crates/app-api/src/tests/sync/hydration_limits.rs` の 5 本を基準 commit の実装で実行し、すべて失敗することを確認した。

| test | 修正前の結果 |
| --- | --- |
| `timeline_refresh_with_a_missing_body_does_not_rescan_or_restart` | 欠損行が 1 件あると、`list_timeline` のたびに `objects/` の全件読みが走る（期待 0 回） |
| `thread_refresh_with_a_missing_body_does_not_rescan_or_restart` | `list_thread` も同じ |
| `one_timeline_call_scans_the_replica_at_most_once` | 空のタイムラインの 1 回の取得で全件走査が 2 回 |
| `recovery_tick_backs_off_when_the_replica_does_not_change` | docs の支援 peer がいて変化の無い replica で、16 秒間に全件走査 5 回（3 秒ごと） |
| `repeated_unresolved_hints_do_not_rescan_per_hint` | 個別反映が 0 件の hint 10 件で全件走査 10 回 |

原因は 2 つあった。

1. `projection_page_needs_hydration` が、本文の欠けた行が 1 件でもあれば `list_timeline_scoped` / `list_thread` に全件走査 2 回・購読再起動・再 sync を起動させていた。
2. 全件走査の戻り値が「replica にある行数」で、全行を無条件に書き直していた。空でない replica では常に `> 0` になるため、recovery tick は
   「進展あり」と判定し続けて backoff をリセットし、欠損の有無に関係なく 3 秒ごとに全件走査と remote 取得を繰り返していた。

## 変更の要約

- 全件走査に指紋を持たせた（`service/hydration_limits.rs` の `ReplicaScanCache`）。`withdrawals/`・`objects/`・`reactions/` は、走査した record の key・content hash・本体の長さから
  作る指紋が前回と同じなら、projection の書き直しを省いて 0 を返す。戻り値は「今回反映した件数」になった。取り下げの反映が途中で終わった走査は記録せず、次回もやり直す。
  `sessions/live/`・`sessions/game/` は件数が少なく manifest の到着で結果が変わるため、毎回反映し直し、record に変化があったときだけ「進展」に数える。
- 欠損した本文は行単位で取り直す（`MissingBodyLedger`）。間隔は 5 秒・30 秒・2 分・10 分（以後 10 分）、1 つの本文につき最大 8 試行、同時実行は 4 本、台帳は 4,096 件まで。
  - 全件走査は、local に無い本文を台帳の間隔の内でだけ取りに行く。1 走査の所要時間が欠損数に比例しなくなった。
  - `list_timeline_scoped` / `list_thread` は、ページの行（権限で絞り込んだ後）の本文が local にあればその場で読んで反映し、無ければ背景 task で取りに行く。呼び出しは待たない。
  - 上限に達した後は、同じ行を指す docs event / hint の個別反映（その場で 1 回試す、既存の挙動）と再起動でだけ取り直す。
  - 1 試行は drop guard（`MissingBodyAttempt`）で表す。取得を待つ走査が購読の再起動などで abort されても「取得中」のまま残らず、失敗として記録される
    （独立監査の指摘で追加。当初は abort されると、その hash を process の終了まで取り直さなくなる欠陥があった）。
- `list_timeline_scoped` / `list_thread` は、欠損を理由に全件走査・購読再起動・再 sync を起動しない。全件走査は「ページが空」か「private channel の現在 epoch が未反映」のときだけ、
  1 回の呼び出しで最大 1 回。`projection_page_needs_hydration` は削除した。
- 購読タスク:
  - recovery tick は、走査しない分岐でも次の期限を置く（毎秒の peer 照会をやめた）。変化が無い間は、全件走査の間隔を再 sync の backoff（3 / 10 / 30 秒）に合わせて伸ばす。
  - hint の個別反映が 0 件でも、replica の内容を指さない hint（`ProfileUpdated`・`Presence`・`Typing`・DM・metaverse など）は全件走査の契機にしない。
    内容を指す hint による全件走査は 3 秒の最小間隔を空ける（最初の 1 回はすぐ走査する）。
- 利用者の操作（repost・reaction・reply・bookmark）と community index の解決は、対象の行が projection に無いときだけ指紋を捨てて必ず反映し直す
  （`hydrate_scope_projection_for_target`）。対象が既にあれば、走査は指紋で省略される。

## AC-5: データモデルの調査と結論

計測（`measure_full_scan_cost`、debug build、in-memory の docs と SQLite、inline 本文の投稿だけの replica）:

| 投稿数 | 反映を伴う全件走査（修正前は毎回これ） | 変化が無いときの全件走査（修正後） |
| --- | --- | --- |
| 100 | 27 ms | 2.4 ms |
| 1,000 | 245 ms | 20 ms |
| 10,000 | 3.4 s | 275 ms |

- 修正前は、10,000 件の topic で 3.4 秒の走査が 3 秒ごとに起動しており、購読タスクが常時走査し続ける計算になる。#1221 の CPU 消費と整合する。
- 修正後は、変化の無い走査が約 1/12 になり、頻度も最大 30 秒に 1 回まで下がる。10,000 件でも 1% 未満の占有で済む。反映を伴う走査は起動時と実際に変化があったときだけになる。
- 結論: 現時点では「topic 全体で 1 replica」のまま、変化検出と行単位の欠損追跡で足りる。replica の分割は行わない。
- 残る限界: 変化の無い走査でも docs の全 entry の読み出し（O(N)）は残る。1 topic が 10 万件規模になったら、走査そのものをやめて docs event だけで反映する形か、
  期間での分割を再検討する。接続と取得の統合設計（#1224）で、復旧の起動条件を 1 か所へ寄せるときの入力とする。

## AC / INVAR と証跡

| 条件 | 証跡 |
| --- | --- |
| AC-1 | `timeline_refresh_with_a_missing_body_does_not_rescan_or_restart`、`thread_refresh_with_a_missing_body_does_not_rescan_or_restart` |
| AC-2 | `one_timeline_call_scans_the_replica_at_most_once` |
| AC-3 | `missing_body_is_retried_on_the_ledger_schedule_and_recovers`、`aborting_a_scan_during_a_body_fetch_does_not_block_later_attempts`、`hydration_limits::tests::attempts_follow_the_delays_and_stop_at_the_limit`・`an_abandoned_attempt_is_recorded_as_a_failure` |
| AC-4 | `recovery_tick_backs_off_when_the_replica_does_not_change`、`repeated_unresolved_hints_do_not_rescan_per_hint`、既存の `public_topic_recovery_keeps_docs_probe_when_live_peer_has_not_delivered_content`・`hint_miss_coalesces_replica_sync_restarts`（無変更で成功） |
| AC-5 | 本書の計測と結論 |
| INVAR-1 | app-api の既存 test 207 本が無変更で成功（新着・取り下げ・リアクション・空のタイムラインからの復旧・private channel の epoch）。`list_timeline_rehydrates_placeholder_from_blob_store`・`thread_open_triggers_lazy_blob_fetch` は、local にある本文をその場で反映する経路で成功 |
| INVAR-2 | 行単位の取り直しの対象は、`allowed_channel_ids_for_scope` と hidden author で絞り込んだ後のページの行だけ。取り下げ済みの行は `content` が空文字で入るため対象にならない。成人向け・advisory の取得判定点（`blob_media_payload`）は無変更 |
| INVAR-3 | 既存 test の削除・弱体化なし。`CountingDocsSync` に再 sync の計数を足しただけ |

## 未確認・残課題

- 2 ノード構成での実機の前後比較は未実施。
- view の生成中に行ごとに呼ばれる取り下げの走査（`profile_post_to_view`・`profile_repost_to_view`・`repost_snapshot_to_view_with_profiles`）は、指紋で反映は省略されるが、
  docs の `withdrawals/` の読み出しは行ごとに残る。
- profile・bookmark の一覧だけに出る行の欠損本文は、行単位の取り直しの対象にしていない（timeline か thread に出たときに取り直す）。
- 行単位の取り直しの上限（8 試行）に達した本文は、その行を指す docs event / hint が来るか、再起動するまで取り直さない。

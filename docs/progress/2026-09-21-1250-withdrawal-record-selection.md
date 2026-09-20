# #1250 取り下げの key に不正な record が先にあっても、著者の取り下げを反映する（2026-09-21）

- 対象 Issue: #1250（区分 C、Scope revision 2026-09-21-v1、基準 commit `bed95a98`）。関連は #1248（同じ形の修正を投稿の envelope に入れた）、#1239（同じ file を変更中）。
- 仕様: [ADR 0052](../adr/0052-scale-independent-timeline-sync.md) §2（逆向きの規則と上限超過の扱いを追記）。inventory: [replica の読み出しの inventory](../architecture/replica-read-inventory.md)（「観察」に追記）。
- 計画: `.claude/plans/2026-09-21-issue-1250-withdrawal-record-selection.md`。

## 調査の結論

- iroh-docs は同じ key の entry を docs author ごとに持ち、key 指定の読み出しは docs author の昇順で全部返す（`exact_bounded_query_limits_the_entries_of_one_key`）。
  public topic の replica は topic id を知る誰もが書ける（`public_replica_secret`）。
- `withdrawals/<object id>/state` を key 指定で読む 4 箇所（`hydrate_object_in_topic`、`hydrate_subscription_event`、`hydrate_subscription_hint`、`schedule_withdrawal_check`）は、
  結果の先頭の 1 件だけを `hydrate_post_withdrawal_from_record` へ渡していた。先頭が取り下げとして検証に通らない record だと `Invalid` になり、後ろにある著者の正しい取り下げは読まれない。
  取り下げ済みの投稿の本文と添付が、その viewer の projection に残る。
- 全件走査（`hydrate_post_withdrawals_from_replica`）は全 record を渡すので、走れば反映される。ただし #1246 以降、利用者の操作からの全件走査はページが空のときなどに限られ、
  ADR 0052 は全件走査の削除を定めているので、救済として当てにできない。
- CN indexer（`crates/cn-indexer/src/ingest/reference_guard.rs`）は、取り下げの key の全 record を順に調べており、同じ形ではない。

## 修正前の再現

`crates/app-api/src/tests/sync/hydration_integrity_contract.rs` の `ShadowingDocsSync`（同じ key に複数の record を返す test double）を使う。

- 起票時: `bac6273c`（tree は `bed95a98` と同じ）に、読めない JSON を正しい取り下げより先に返す test を足し、`hydrate_object_in_topic` の後も行の `content` が
  `Some("words the author took back")` のまま（期待は `Some("")`）で失敗することを確認した。
- 入口ごとの test（下表の INV-1〜INV-4 と TR-5）は、調べる record 数を 1 件（= 修正前の「先頭の 1 件だけ」）にすると 5 件とも失敗し、8 件に戻すと成功する。

## 修正

- `hydrate_post_withdrawal_for_object` を新設した。取り下げの検証と反映は `crates/app-api/src/service/post_withdrawal_hydration.rs` へ分けた
  （`hydration_support.rs` が `oversized-files` の 1000 行の上限を超えないようにするための移動で、`hydrate_post_withdrawal_from_record` も同じ file へ移した）。取り下げの key を `query_replica_exact_bounded` で
  最大 `MAX_WITHDRAWAL_RECORDS_PER_OBJECT`（8）件読み、署名が正しく、その object を対象とする取り下げを候補にする。候補があるときだけ対象の envelope の key を 1 回
  （最大 8 件）読み、`verify_withdrawal_against_records` に通る最初の取り下げを反映する。1 件の確認で読む docs の record は最大 16 件で、replica の大きさに依存しない。
- 別の object を対象とする正しい署名の取り下げをこの key に置かれても、確認を終わらせない（候補から外して次の record を調べる）。
- 4 つの入口を helper へ載せ替えた。取り下げの key を key 指定で読む箇所は helper だけになった。
- `hydrate_post_withdrawal_from_record`（全件走査が record ごとに呼ぶ）は、解析（`parse_post_withdrawal_record`）と反映（`apply_verified_post_withdrawal`）を helper と共有する形に分けた。
  結果は変えていない。取り下げの行（`put_post_withdrawal`）と伏せた行（`put_object_projection`）を書くのは `apply_verified_post_withdrawal` だけで、検証の後にある。

## AC / INVAR と test

test は `crates/app-api/src/tests/sync/withdrawal_record_selection.rs`。test double は `hydration_integrity_contract.rs` の `ShadowingDocsSync`（上限つきの読み出しの記録を足した）。

| 条件 | transition | test |
| --- | --- | --- |
| AC-1・AC-2（INV-1） | TR-1・TR-2 | `invalid_records_placed_before_the_withdrawal_do_not_cancel_the_withdrawal` |
| AC-2（INV-2） | TR-1 | `withdrawal_event_applies_the_withdrawal_behind_invalid_records` |
| AC-2（INV-3） | TR-1 | `withdrawal_hint_applies_the_withdrawal_behind_invalid_records` |
| AC-2（INV-4） | TR-8 | `background_withdrawal_check_applies_the_withdrawal_behind_invalid_records` |
| AC-3 | TR-4 | `withdrawal_records_beyond_the_per_key_limit_are_not_examined`、`withdrawal_at_the_last_examined_position_is_applied_with_two_bounded_reads` |
| AC-4 | — | ADR 0052 §2、inventory の「観察」 |
| INVAR-1 | TR-3 | `invalid_withdrawal_records_alone_do_not_hide_the_post`、既存の `withdrawal_reflection.rs` の 2 件 |
| INVAR-2 | TR-5 | `withdrawal_behind_invalid_records_waits_for_a_missing_target`、既存の `withdrawal_that_arrives_before_the_post_masks_it_when_the_post_is_projected` |
| INVAR-3 | TR-6 | `withdrawal_read_failure_is_an_error_and_does_not_project_the_body` |
| INVAR-4 | TR-7 | 既存の全件走査の test（`posts.rs`、`scale_independence.rs` など）。全件走査の関数は変更していない |

不正な record は 4 種類を並べる: 読めない JSON、取り下げでない envelope、署名が合わない取り下げ、別の object を対象とする正しい署名の取り下げ。
対象の著者と合わない署名つきの取り下げの拒否は、#1248 の `verification_rejects_each_kind_of_mismatch` が純粋な検証として固定している。

## inventory

Issue #1250 の INV-1〜INV-7。INV-1〜INV-4 は helper へ載せ替えて適合。INV-5（全件走査）と INV-7（書き込み側）は変更なしで適合。
INV-6（PR #1247 の `replica_window.rs` の `ensure_index_entries_projected`）は `main` に未 merge。後から入る側が helper を使う。

## 残る限界

上限（8 件）を超える数の不正な record を小さい docs author id で積まれた取り下げは、反映できない（本文が残る側に倒れる）。「上限に達したら伏せる」とすると、
不正な record を積むだけで他人の投稿を隠せるので採らない。key 設計か protocol の変更が要る。Issue #1250 の Non-goal で、ADR 0052 §2 に未決として書いた。

# #1221 R6-B: 動作確認の所見を補修

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R6-B と、2026-09-28 のユーザー決定「R6-A 動作確認の所見の補修」。
R6-A の動作確認(Windows・Linux の実アプリと CLI、ローカル CN)で見つかった 3 件を直す。実機での回復の再確認は R6-A で行う。

| # | 所見 | 由来 |
| --- | --- | --- |
| 1 | follow していない author の表示名が、他のクライアントの timeline で「不明なユーザー」になる。profile を開いても出ない | #1221 R2-C の回帰(`list_timeline` がページの全 author を購読して取り込んでいた処理を外した) |
| 2 | CLI の `fetch_community_node_policies` が `internal_error`(出力が schema に合わない)を返す | #1238 から(CN が返す `policy_kind` を CLI の出力 schema が知らない) |
| 3 | 起動直後の stack の作り直しの間に読込みが失敗した profile 列に、エラー表示が残る | #995 から(列の読込みは失敗後に再取得しない) |

## 受入条件

| ID | 対象・期待結果 | 判定(試験) |
| --- | --- | --- |
| AC-1 | timeline のページの author のうち手元に profile の無いものを、購読せずに背景で profile の key だけ読む。queue(64 件)と台帳(1024 件、同じ author は 10 分読み直さない)は上限つき。scope の lease を増やさない | app-api `timeline_authors_without_a_local_profile_are_read_once_in_the_background`・`the_ledger_skips_recent_authors_and_keeps_its_bound` |
| AC-2 | 表示中の profile 列は、相手の名前が無い間(読込みの失敗を含む)と自分の profile の読込みが失敗している間だけ、表示の定期更新(3 秒)で読み直す | frontend `a failed profile column read recovers on the next visible refresh`・`a restored author profile column re-reads a failed read until the name arrives` |
| AC-3 | CLI の policy 文書の出力 schema に `policy_kind` を加える。試験の mock CN も返す | CLI `explicit_node_consent_enables_requests_and_policy_change_blocks_them_again` |

## 実装

- `crates/app-api/src/service/missing_profiles.rs`: 台帳と queue と背景の worker。worker は最初の依頼で起動し、
  `shutdown` で止める。1 件ずつ `hydrate_author_profile`(`author_state_support.rs`。R5-C の有界な読みで profile の
  key だけ)を呼ぶ。queue が満ちたら残りは渡さず、次の表示で渡る。
- `timeline_view_support.rs` の `page_to_view_with_policy`: 手元の profile を読んだ後、無い author(自分以外)を渡す。
  timeline・thread・community index の結果の表示が同じ経路を通る。
- 表示: 取り込んだ profile は、表示中の timeline の定期更新(3 秒)で名前に反映する。
- `apps/desktop/src/shell/data/useDesktopShellDataEffects.ts`: 表示の定期更新で、AC-2 の条件の profile 列だけ読み直す。
- `crates/kukuri-cli/src/commands/community_views.rs`: `policy_document()` に `policy_kind`(任意)。
- ADR 0055 §1.1 に、背景の profile の読取りと profile 列の読み直しを記録した。

## 検証

- 修正前の再現: AC-1〜AC-3 の試験は、修正を外すと失敗する(AC-1 は背景の読取りが起きず時間切れ、AC-2 は列が
  エラーのまま、AC-3 は実機と同じ `internal_error`)。AC-1 は台帳の判定を外すと「読み直さない」の assert で失敗する。
- ローカル: app-api の lib の試験、CLI の `community_node`、clippy、frontend の typecheck・lint と `src/shell` の試験。
- 全体は PR の CI。

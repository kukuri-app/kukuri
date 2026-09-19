# Issue #1154: 変更通知前の取得先 peer 更新

## 対象

- Scope revision: 2026-09-20
- 基準 commit: `df2fd79eb31762425b2b4ed12bd3b14e9c4ec843`
- リスク区分: C（network / seed の global apply）
- Goal: 全件見直し後に登録されたclientのblob本文・mediaを、変更通知の取り込みで取得できるようにする。

## 実装

`IndexerWorker` は変更通知を既存のdebounceでまとめた後、scopeごとの取り込みを始める前に
`IndexerParticipant::refresh_seed_peers` を1回呼ぶ。これにより、1 batch内のkey数・scope数に
かかわらずbootstrap loaderは1回、docs syncとblob serviceの再設定は各1回になる。

更新に失敗したbatchは取り込まず、runtime errorへ記録する。次の通知batchまたは既存の全件見直しで
同じhelperを再実行する。docs側だけ更新された後にblob側が失敗した場合も、次回は両方へmerge済み集合を
再適用する。operator指定seedのmerge、全件見直し時の更新、取得不能時に新規投稿を索引せず既存entryを
保持する契約は変更していない。

## 修正前の再現

`event_batch_refreshes_seed_peers_before_fetching_blob_text` を実装前に実行した。初回passではpeer Aだけを
適用し、その後peer Bをheartbeat登録して、Bが設定されるまでblob本文を返さないfixtureから投稿通知を
送った。旧実装はBを再取得せず、`event ingest after peer refresh` が30秒でtimeoutした。

## AC / INVAR evidence

| 条件 | 実装・test |
| --- | --- |
| AC-1 / AC-2 | `worker.rs` のdebounce batch直後の更新と `event_batch_refreshes_seed_peers_before_fetching_blob_text`。次の120秒pollを待たずにBをdocs/blobへ適用して索引する |
| AC-3 | 同testで4 keyの通知を1 batchにまとめ、startup後のsetter回数がdocs/blobとも1だけ増えることを確認。`refresh_seed_peers` 1呼出しがloader 1回を所有する |
| INVAR-1 | 同testでoperator seed、A、Bがdocs/blob双方の最終集合に残ることを確認。既存`merge_seed_peers`は無変更 |
| INVAR-2 | `failed_seed_refresh_skips_event_ingest_and_recovers_on_next_batch` で部分失敗時に新規投稿を索引せず既存entryを保持し、次batchで回復することを確認 |

## Surface / sink 確認

CodeGraphとcaller検索で、`refresh_seed_peers` の入口は定期/起動時の`restore_scopes`と今回追加した通知batch、
`restore_scopes` の入口はworker `full_pass`と`ingest_all_supported`であることを確認した。
`load_bootstrap_seed_peers` はexpired登録のDELETEとactive登録のSELECTを1呼出しずつ行う。
`apply_seed_peers` はdocs、blobの順に`set_seed_peers`を呼ぶ。索引mutationは更新成功後の
`ingest_with_backoff`からだけ到達し、更新失敗時の`continue`がこれを支配する。固定inventoryの未分類は0。

## Validation

- 修正前: `cargo test -p kukuri-cn-indexer --test worker_contracts event_batch_refreshes_seed_peers_before_fetching_blob_text -- --nocapture` — FAIL（期待したtimeout）
- 修正後 targeted: `cargo test -p kukuri-cn-indexer --test worker_seed_peer_contracts -- --nocapture` — PASS（2 tests）
- `cargo xtask oversized-files` — PASS
- `cargo xtask cn-check` — PASS。
- `cargo xtask cn-test` — PASS。初回は通常shellからの実行でWindows SDKの`ucrt.lib`を解決できず
  linkerが停止した。Visual Studio 2022の`vcvars64.bat`環境を読み込んで同じcommandを再実行し、
  CN全suiteとdoc testsが成功した。

本番rollout後の数十秒以内という実測は、`docs/runbooks/community-node-production-rollout.md` §5.6に従って
別途確認する。ローカルcontractは、120秒の次回pollより前に通知経路だけで回復することを固定する。

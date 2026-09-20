# #1207 blob 取得の自動再試行の有限化（2026-09-20）

- 対象 Issue: #1207（区分 C、Scope revision 2026-09-20-v2）。親の調査は #1221。
- 基準 commit: `e08743fb`。
- 仕様の記録: [desktop の blob キャッシュと取得の再試行](../architecture/blob-cache.md)、`DESIGN.md` の投稿メディアの節。

## 修正前の再現

| 対象 | 再現 | 修正前の結果 |
| --- | --- | --- |
| 画面: 失敗した hash の再要求（TR-9） | 取得が常に `null` を返す api で timeline を表示し、fake timer で 15 秒進める（3 秒 refresh を複数回またぐ） | 基準 commit `e08743fb` の `apps/desktop/src` では、失敗する 1 つの hash への `getBlobMediaPayload` が 60 秒で 21 回（3 秒 refresh ごとに 1 回）。修正後は同じ test で 3 回以下。計測は一時 test（fake timer で 60 秒進める）を修正前後の tree で実行して行い、恒久 test は `DesktopShellPage.mediaRendering.test.tsx` に置いた |
| backend: 実行中の予約が無い（TR-10 / TR-11） | `RemoteFetchRetryState::try_begin` を同じ key で 2 回呼ぶ | 2 回とも `Ready`。基準 commit の `remote_fetch_retry_state_cools_down_failures_without_blocking_active_fetches` がこの並行を仕様として固定していた。`finish` を呼ばない限り（＝呼び出し側が future を drop した場合）次回も `Ready` になり、クールダウンは残らない |

## 変更の要約

- backend（`crates/transport/src/peers.rs`、`crates/iroh-node/src/remote_fetch.rs`）
  - `RemoteFetchRetryState` に実行中の走査の予約を追加した。同じ対象の要求は実行中の走査へ合流する。
  - 走査は呼び出し側の future から切り離した task で実行し、`finish`（クールダウン）と peer 単位の成否を必ず記録する。
  - 保存先が違う取得（永続 / 一時 / 上限つき一時）は合流させない。同時に実行する走査は 8 本まで。
  - 期限切れのクールダウンは `finish` のたびに捨てる。
  - caller（blob-service、docs-sync、cn-indexer）が使う関数の戻り値と `BlobService` trait は変えていない。引数は `&Arc<_>` になった。
- 画面（`apps/desktop/src/shell/data/`）
  - 取得の要否を attachment の参照の同一性ではなく、hash 単位の試行台帳 `MediaFetchLedger` で決める。
  - 自動取得は 3 試行（待ち 5 秒・30 秒）で止まる。明示再試行は対象 hash だけを 1 試行取り直す。
  - `mediaFetchLatestRef`・`mediaFetchInputRef` を廃止し、`mediaGateEpochRef` は対象外になった hash を剪定する。
- 表示
  - 投稿メディア・DM 添付・画像 viewer に「取得に失敗しました」と再取得の icon button（`MediaFetchFailure`）を出す。
  - 再取得中は button を同じ位置に残し、`aria-disabled` と `aria-busy` で重複操作を止める（focus を失わせないため `disabled` にしない）。
  - 通常 mode で取得不可のメディアを何も描画しなかった従来の挙動は、失敗表示へ置き換えた（開発者 mode 限定の診断文言は廃止）。

## AC / INVAR と証跡

| 条件 | 証跡 |
| --- | --- |
| AC-1 | `docs/architecture/blob-cache.md` |
| AC-2 | `mediaFetchLedger.test.ts`（回数・待ち時間・リセット条件・上限）、`DesktopShellPage.mediaRendering.test.tsx` の `automatic media fetch stops at the attempt limit and later refreshes do not retry` |
| AC-3 | `tests/playwright/media-unavailable.spec.ts`（実 browser で失敗表示と再取得の button、`Available` への復帰）、同 test file の `manual retry refetches only the failed hash and recovers when the blob arrives`、`manual retry that fails returns to the failure display without looping`、Storybook `PostCard` の `MediaFetchFailed` / `MediaFetchRetrying`、i18n の key parity test |
| AC-4 | `media_adult_gating.rs` の `projecting_remote_posts_fetches_attachments_only_on_ungated_display_request`（取得済み hash で remote 取得が増えない）、`remote_fetch.rs` の `joined_callers_share_one_result` |
| AC-5 | AC-2 の結合 test（3 秒 refresh をまたいでも呼び出し回数が増えない） |
| AC-6 | `remote_fetch.rs` の `dropped_caller_still_records_the_failure_cooldown`、`repeated_callers_join_the_walk_in_flight`、`peers.rs` の `remote_fetch_retry_state_*` |
| INVAR-1 | 既存の `media_adult_gating.rs`（表示 OFF で network I/O・local 読み出し 0）、`DesktopShellPage.adultToggleMedia.test.tsx`・`adultContentGating.test.tsx`・`timelineAdvisory.test.tsx`（無変更で成功）。再試行は gate 中の hash を受け付けない |
| INVAR-2 | `remote_fetch.rs` の `store_and_ephemeral_walks_do_not_join`、既存の blob-service `ephemeral_fetch_returns_bytes_without_persisting_them_locally` |
| INVAR-3 | `remote_fetch.rs` の `concurrent_walks_are_bounded`、`peers.rs` の `remote_fetch_retry_state_prunes_expired_cooldowns`、台帳の上限 test |

## 未確認・残課題

- 2 ノード構成での実機の前後比較（画像付き投稿を同期後に投稿元を停止し、`getBlobMediaPayload` の回数と remote fetch の log 件数を比べる）は未実施。
- Windows・Linux の実機での表示確認は未実施。表示の確認は Vitest と Storybook による。
- Linux の外側 timeout（2 秒）で peer 単位の失敗記録に届かなかった点は、走査を呼び出し側から切り離したことで解消した（走査は connect timeout 5 秒まで進む）。
  実 peer を相手にした確認は未実施。
- 本文 blob の取得回数の上限は #1225、接続と取得の統合は #1224 が扱う。

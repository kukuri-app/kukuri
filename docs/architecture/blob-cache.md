# desktop の blob キャッシュと取得の再試行

Issue #1207 の AC-1 / AC-2 として、desktop が blob（画像・動画・本文・manifest など）を保持し取得し直す仕組みを、
実装と test から確認して記録する。仕様の正本は各 ADR と test であり、本書は現状の対応表である。

## 層と保持範囲

| 層 | 保持するもの | key | 保持範囲・破棄 | 根拠 |
| --- | --- | --- | --- | --- |
| 画面: `mediaObjectUrls` | 取得済み bytes から作った object URL（`string`）、自動取得が上限に達した印（`null`） | blob hash | shell の mount 中。unmount で全 URL を revoke。成人向け gate の対象になった hash は URL を revoke して項目を消す。件数の上限と剪定は無い | `apps/desktop/src/shell/slices/media.ts`、`shell/data/useDesktopShellDataEffects.ts` |
| 画面: `mediaRetryingHashes` | 失敗が確定した hash のうち再取得を試行中のもの | blob hash | 再取得の完了まで | 同上 |
| 画面: 試行台帳 `MediaFetchLedger` | hash ごとの試行回数、取得中、次回試行時刻、上限到達 | blob hash | shell の mount 中。上限 2,000 件で、取得中でない古い項目から捨てる。永続化しない | `shell/data/mediaFetchLedger.ts` |
| backend: local blob store | blob bytes | blake3 hash | `<data root>/blobs.db` の `FsStore`（test と一部の構成は `MemStore`）。GC は設定していない（`BlobStoreOptions::new` の既定 `gc: None`）。容量上限・保持期間は無い | `crates/iroh-node/src/node.rs` |
| backend: pin | metaverse 用の tag と in-memory の pinned 集合 | blob hash | tag は store に永続。pinned 集合はプロセス内 | `crates/blob-service/src/lib.rs` の `pin_blob` / `unpin_blob` |
| backend: projection の blob 状態 | 添付ごとの `BlobCacheStatus`（`Available` / `Missing` など） | blob hash | projection store に永続。表示用で、local の有無だけを見る（remote 取得しない） | `crates/app-api/src/service/object_persistence_support.rs` の `best_effort_blob_view_status` |
| backend: remote 取得の retry state | 失敗クールダウン（3 秒）、実行中の走査の予約 | hash（bounded 取得は `bounded:{limit}:{hash}`）。予約は保存先ごとに分ける | プロセス内。期限切れのクールダウンは `finish` のたびに捨てる。blob-service と docs-sync が別々に 1 つずつ持つ | `crates/transport/src/peers.rs` の `RemoteFetchRetryState` |
| backend: peer 台帳 | peer ごとの接続状態、取得の成否、backoff（2〜60 秒）、要求頻度（peer あたり 16 回 / 秒） | endpoint id | プロセス内。削除経路は無い（#1224 で扱う） | `crates/transport/src/peers.rs` の `PeerAddrBook` |

`BlobMediaPayload`（base64 の bytes）は IPC の応答として渡るだけで、backend 側の memory キャッシュは無い。
画面側は応答から object URL を作り、以後はその URL を使う。

## 取得経路と I/O

| 要求 | local にある場合 | local に無い場合 |
| --- | --- | --- |
| `blob_media_payload`（表示要求） | store から読む。network I/O 0 | 成人向け・advisory の対象でなければ `fetch_blob`（remote から取得して store へ保存）。対象で表示 ON なら `fetch_blob_ephemeral`（保存しない）。対象で表示 OFF なら network I/O も local 読み出しも行わず `None` |
| 本文・manifest（`fetch_projection_blob_text` / `fetch_manifest_blob`） | 同上 | `fetch_blob` を外側 timeout（Windows 5 秒、その他 2 秒）つきで待つ |
| LocalOnlyの本文（`fetch_local_projection_blob_text` / `fetch_local_blob`） | local storeのbytesだけを読む | 未取得またはlocal I/O失敗。remoteへfallbackしない |
| 表示用の状態（`local_blob_status`） | `Available` / `Pinned` | `Missing`。remote 取得しない |
| `blob_status` | `Available` / `Pinned` | 内部で `fetch_blob` を呼ぶ（remote 取得を伴う） |
| docs entry の本文 | docs の store から読む | `fetch_bytes_with_cooldown`（docs-sync 側の retry state） |
| CN の scan 用取得 | store から上限つきで読む | `fetch_blob_ephemeral_bounded`（保存しない、大きさ上限つき） |

状態はbytesの読取り成功を保証しない。`Available`確認後の削除・I/O失敗や、実体の無いpin記録もあるため、
LocalOnlyの読取りを状態確認とremote可能な`fetch_blob`の組合せで実装しない。既存の`fetch_local_blob`を使い、
Memory/Iroh/Reloadableの各adapterがlocal sinkを明示する。未対応adapterの既定実装はNoneを返す。取得できなかった本文はviewでもMissingとして扱う（#1243）。

remote 取得の 1 走査は、順位付けした全 peer の接続候補を順に試す。候補ごとに connect 5 秒・転送 15 秒、走査全体で 30 秒を上限とする
（`crates/iroh-node/src/remote_fetch.rs`）。

## 再試行の契約（#1207 で固定）

### 回数の単位

- **1 試行** = 画面からの `getBlobMediaPayload` 1 回 = backend の peer 走査 1 系列（最大 30 秒）。peer 候補ごとの connect は試行に数えない。
- 自動取得は hash ごとに最大 **3 試行**（初回 + 再試行 2 回）。失敗から次の試行までの待ち時間は **5 秒、30 秒**。
- 上限に達した hash は `mediaObjectUrls[hash] = null` になり、投稿メディア・DM 添付・画像 viewer は「取得に失敗しました」と
  再取得の icon button を出す。avatar・カスタムリアクション・通知の actor avatar は既存の代替表示のままで、button は出さない。
  有限化は全 hash に適用する。

数値の根拠: 投稿の docs entry が届いてから blob の提供 peer へ接続できるまでの遅れ（数秒）を 1 回目の再試行で、
relay 経由の接続確立や提供 peer の一時的な不在（数十秒）を 2 回目で拾う。それ以上は利用者の操作に委ねる。
1 試行が最大 30 秒かかるため、上限到達までの最長は約 125 秒である。

### 保存範囲とリセット条件

台帳は shell の data 層に 1 つだけあり、component の mount、Column の追加、offscreen からの復帰、state の参照の変化
（3 秒 refresh・通知更新・advisory 更新）では回数を戻さない。取得は data 層の 1 つの effect だけが起動し、表示側の component は起動しない。

回数を戻すのは次の場合だけである。

1. 利用者の明示再試行。対象 hash だけを **1 試行** 取り直す。失敗すれば失敗表示へ戻り、自動では取り直さない。取得中の hash への再試行は無視する。
2. その hash の attachment の `status` が `Available` 以外から `Available` へ変わったとき（backend が local に揃ったと報告した）。
3. 成人向け gate の対象になったとき（記録を捨てる。gate の解除後に改めて取得する）。
4. api の差し替え（別の backend への接続）と、アプリの再起動。

#1284 で投稿本文と返信先previewの本文も明示再試行の対象にした。本文の個別ボタンは対象行の本文hashだけ、投稿カード右端の操作はその投稿自身と直前の返信先にある欠損本文をそれぞれ1試行する。本文の明示試行は`MissingBodyLedger`の自動cooldown／上限とは別の1試行として記録し、失敗後に自動試行を再開しない。添付は同じカードで失敗済みのhashだけを既存のmedia明示再試行へ渡す。取得済みblobの破棄、返信先の再帰探索、topic／replicaの走査は行わない。

返信先としてだけ表示される行の本文が欠けている場合、その返信を含むページが表示されたときだけ`recover_missing_bodies`へ渡す。試行間隔、最大8試行、同時実行4本、台帳上限4,096件は一覧本体の本文と共有する。表示されていない返信先を巡回するbackground loopは持たない。

### backend 側の契約

- 同じ対象の remote 走査は 1 本だけにする。実行中に届いた要求は同じ結果に合流する。
- 走査は呼び出し側の future から切り離した task で最後まで実行する。呼び出し側が外側の timeout や cancel で待つのをやめても、
  失敗クールダウン（3 秒）と peer 単位の成否は必ず記録される。
- 保存先が違う取得（永続 / 一時 / 上限つき一時）は合流させない。一時取得の bytes を保存経路へ混ぜないためである。クールダウンの key は共有する。
- 同時に実行する走査は retry state ごとに 8 本まで。超過分は順番を待つ。blob の取得（blob-service）と docs entry の取得（docs-sync）は
  別の retry state を持つため、取得できない blob の走査が docs entry の取得を待たせることはない。順番待ちの時間は走査の総予算 30 秒に含めない。

## restart・account 切替

- 再起動で画面側の状態（object URL、失敗の印、台帳）と backend のプロセス内状態（retry state、peer 台帳）はすべて消える。
  local blob store と projection の blob 状態は残るため、取得済みの blob は再起動後も local から返る。
- 失敗の記録は永続化していない。再起動後は hash ごとに改めて最大 3 試行する。
- account 切替は account ごとの db path で runtime を作り直し（`crates/desktop-runtime/src/host/mod.rs` の `switch_account`）、画面は切替の完了後に
  `window.location.reload()` で読み込み直す（`apps/desktop/src/lib/accountSession.ts`）。画面側の状態も backend のプロセス内状態も
  account をまたいで引き継がれない。

## 確認できた欠落（本 Issue では直さない）

- local blob store に容量上限・GC・保持期間が無い。取得した blob は増え続ける。
- `put_blob` は一時 tag で追加するだけで、pin していない blob を保護する仕組みは無い（現状は GC が無いため消えない）。
- 画面側の `mediaObjectUrls` と object URL に件数の上限が無い。長時間の閲覧で memory が増える（#1221 の調査で記録）。
- peer 台帳に削除経路が無い（#1224）。
- 本文 blob の取得は #1225 で有限にした。local に無い本文は hash 単位の台帳（`MissingBodyLedger`）に従い、5 秒・30 秒・2 分・10 分の間隔で最大 8 試行、
  同時実行は 4 本まで。取得側の反映（窓の追いつき・ページの範囲の照合、#1239）も同じ台帳に従う。上限に達した後は、その行を指す docs event / hint の個別反映と再起動でだけ取り直す
  （`docs/progress/2026-09-20-1225-timeline-hydration-finite.md`）。

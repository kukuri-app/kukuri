# ADR 0058: Web の blob と docs の保存を、上流の memory store と kukuri 層の保存 trait で行う

## Status

Accepted（§1〜§6 は Issue #1215 W2 AC-1、§7 は #1216 W3 AC-1）

## Context

ブラウザでは iroh-blobs・iroh-docs の永続 store（`FsStore`、redb の file）が使えない（ADR 0056）。2026-09-30 に #1213 D-3 を改訂し、両者を fork しないことにした。
fork 案は、blobs では外部 store を差す公開 API（上流 PR #259、open）と store actor の自前実装（使う 12 の message、partial・outboard・temp tag）が要り、
上流の GC は全 tag・全 temp tag・全 blob を走査する（`iroh-blobs 0.103.0` の `src/store/gc.rs`）ので、件数に依存しない回収も別に書く必要があった。
docs では redb の `StorageBackend` が同期 I/O で、IndexedDB で実装すると起動時に DB 全体を読むことになる。

native には、同じ責務を持つ kukuri の層が既にある（`docs/architecture/blob-cache.md`、#1221 R5-A・G・I）。

- account の SQLite の remote cache（`crates/store/src/sqlite/remote_cache*`）が、検証済みの remote blob と record、本人の書込みと bookmark の保護（`own_blob:`・`dm_outbox:`・`dm_message:`・`dome_pin:`・`own_docs`・`bookmark:`・`reaction_bookmark:`）を持つ。
  保護参照の付け外しは、DM の ACK・手元の削除、bookmark の追加・解除のときに、projection の更新と同じ SQLite transaction の中で行う（`set_refs_in`・`replace_remote_protected_refs`）。
  remote の投稿 projection と表示用ラベルの根拠も同じ台帳で容量を計数・回収する（`charge_remote_projection`）。
- 非保護分は合計 3 GiB・非利用 7 日・1 処理 128 件で回収し、取得中は 1 MiB 単位で予約する（`REMOTE_CACHE_CAPACITY_BYTES` など）。
- 通常の remote 取得は一時 bytes を返し、consumer が gate を確かめてから cache へ書く。cache の blob は `/kukuri/remote-blob/1`（`crates/iroh-node/src/remote_blob.rs`）で 1 MiB ずつ別 peer へ再提供でき、iroh-blobs の取得が Missing のときの fallback になる（`remote_fetch.rs`）。
- 上流の `MemStore` は wasm で動く（`src/store/mem.rs`、時刻は `n0_future::time`）。

この層は `SqliteStore` の inherent method として、blob-service・iroh-node・docs-sync・desktop-runtime・app-api から直接呼ばれている（method と呼出元 crate は §2）。

## Decision

### 1. 上流の memory store を使う

- Web の iroh-blobs は上流の `MemStore`、iroh-docs は上流（root `Cargo.toml` の patch rev）の `Store::memory()` を使う。どちらも fork しない。wire protocol・hash・取得 gate は native と同じ。
- Web の blob-service は `MemStore` へ書かない。本人の blob（`put_owned_blob`）・remote の blob（`put_remote_blob`）・pin は保存 trait（§2）へだけ書き、取得は一時 bytes の経路（ephemeral）だけを使う。
  これで `MemStore` に残るのは iroh-docs の entry が参照する内容と転送中の temp tag だけになる（その保持の上限は W3 AC-1 が決める）。blob の回収に上流の GC は使わない（docs の閉じた replica の内容の回収にだけ使う。§7）。
- Web が持つ blob を相手へ提供するのは `/kukuri/remote-blob/1` とする。iroh-blobs の取得は `MemStore` に無いので Missing になり、既存の fallback がこの protocol で取りに来る。protocol は変えない。
- native は変えない（`FsStore` と SQLite の remote cache の両方）。

### 2. 保存 trait

- `crates/store` に保存 trait を 1 つ置き、下の操作を持たせる。native は `SqliteStore` が実装し（今の inherent method を移す）、Web は `crates/web-runtime` の IndexedDB が実装する。
  trait の切り出しと native の呼出元の切替は W1 AC-2 が行う（wasm32 の build で `SqliteStore` の直接保持を外すため）。signature はそのとき確定し、下の意味は変えない。

| 区分 | 操作（今の `SqliteStore` の method） | 呼出元 | 意味の所有 |
| --- | --- | --- | --- |
| 予約 | `reserve_remote_cache_bytes`（予約の guard を返す。`empty_remote_cache_reservation`） | iroh-node | W2 |
| 内容 | `put_remote_content`・`get_remote_content`・`has_remote_content`・`remote_content_len`・`remote_content_chunk` | blob-service・iroh-node・desktop-runtime | W2 |
| 保護 | `add_protected_ref`・`put_owned_blob`（app-api は `BlobService::put_owned_blob` 経由） | blob-service | W2 |
| 保護の置き換え | `replace_remote_protected_refs`（参照を `desired` の集合へ置き換える。外れた内容は容量の内なら非保護へ戻し、超えるなら消す） | projection の更新（DM の ACK・手元の削除、bookmark・reaction bookmark の追加・解除）。native は `SqliteStore` の中で同じ transaction | W2（意味）・W4（Web の呼出元） |
| 回収 | `reclaim_remote_cache_step`・`mark_remote_blob_adult`・`forget_adult_remote_blobs_step`・`subscribe_adult_label_evictions` | desktop-runtime・blob-service | W2 |
| record | `put_remote_record`・`get_remote_records`・`remote_record_keys`・`put_owned_record`（`remote_record_cache_key` は key の組立ての関数） | docs-sync・iroh-node・desktop-runtime | W3 |

- Web の projection（ADR 0056 §5、W4 AC-1 が方式を決める）は、DM・bookmark の更新のときに「保護の置き換え」を呼ぶ。projection を IndexedDB に置く場合は、本 ADR の cache と同じ database に置き、
  projection の更新と `refs` の更新を同じ transaction で行う（native と同じ原子性）。remote の投稿 projection の容量の計数も、そのとき同じ database の `meta` に含める。
- file path を受け取る操作（`put_remote_blob_file`・`copy_remote_content_to_file`）は native の build でだけ trait に置く（`cfg(not(target_family = "wasm"))`）。native の呼出元（iroh-node・blob-service）は trait object から呼び、Web の build には現れない。Web は file を使わない（ADR 0056 §5）。
- peer candidate の操作（`put_peer_candidate` など）はこの trait に入れず、別の trait `PeerCandidateStore`（`crates/store`）にする。native は `SqliteStore`、Web は同じ cache の database の `peer_candidates` が実装する（W4 AC-2。ADR 0056 §5）。

### 3. IndexedDB の形（Web の実装）

- account ごとに 1 つの database（名前は `kukuri-cache-v1-` に account の公開鍵の hex を続ける）。account の切替で別の database を開き、別 account の内容を混ぜない。
- object store:
  - `contents`: key は `[kind, key]`。値は長さ・`scope`（成人向けの印）・最後に使った時刻・保護参照の数。索引は「非保護か・最後に使った時刻」（回収の順）と「`scope`・非保護か」（成人向けの回収。保護行で 1 処理の窓が埋まらないよう、native の `scope_key = 'adult' AND is_protected = 0` と同じ条件にする）。
  - `chunks`: key は `[kind, key, 連番]`。値は最大 1 MiB の bytes（`/kukuri/remote-blob/1` の 1 回の単位と同じ）。
  - `refs`: key は `[保護参照, kind, key]`。`[kind, key]` の索引で参照の有無を数える。
  - `meta`: 非保護分の合計 bytes など、1 行ずつの集計。
- 1 つの内容の書込みは 1 つの readwrite transaction にし、`chunks` と `contents` の行と `meta` の集計を同じ transaction で書く。transaction が確定した時点を「完成」とする。
  quota 超過・中断で transaction が失敗すると何も残らない（途中まで書かれた内容を完成と扱わない）。途中の取得（partial）は保存せず、取り直す。
- 起動時に内容を読み込まない。`meta` の集計を 1 行読むだけで、以後は key を指定して読む。
- 実装（W2 AC-2）: `crates/web-runtime` の `IndexedDbCache`。database は 1 つの task が持ち、操作を 1 つずつ行う（native の `remote_cache_gate` と同じ直列化。ADR 0056 §4）。
  `reclaim`・`adult` の索引は非保護の行だけに値を置く。Web の blob-service は `IrohBlobService::with_content_cache` で作り、`MemStore` へ書かない。record の操作の実装は §7（W3 AC-2）。

### 4. 容量・回収・予約

- 非保護分の上限は `min(3 GiB, 起動時の StorageManager の quota の 50%)`。非利用 7 日・1 処理 128 件・取得中の 1 MiB 単位の予約・最後に使った時刻の更新の間隔（1 時間）は native と同じ定数を使う。
- 回収は `contents` の索引を古い順に 1 処理 128 件まで歩き、保護参照のある行を消さない。全行を読んでから選ばない。回収の背景 task は native と同じ owner（desktop-runtime）から起こす。
- 成人向けの印の付いた非保護分の回収も `scope` の索引で 1 処理 128 件まで。
- quota 超過で書けないときは、回収を 1 処理行って 1 回だけ書き直す。それでも書けなければ保存の失敗として返し、取得を延期する（native の容量不足と同じ）。鍵・設定は W4 の別の database にあり、private channel の鍵は projection の保護行（ADR 0061 §9）にあって、どちらもこの回収で消えない（#1215 INVAR-2）。
- 実装（W2 AC-3）: 上限は保存 trait の `remote_cache_capacity` で返し（native は 3 GiB のまま）、取得の予約と cache へ置くかの判定（`remote_fetch`・`remote_blob`・blob-service）もこれを使う。
  書けなかったとき（transaction の中断。quota 超過を含む）の回収は、書く内容の bytes の分を非保護の行の古い順に 128 件まで消す。chunk の数が長さと合わない（破損した）内容は完成と扱わず、行と chunk を消して取り直させる（保護参照は残す）。

### 5. 検証の範囲

- AC-2: 保存 → reload → native との送受信で、同じ hash・完成状態になる（`/kukuri/remote-blob/1` の提供を含む）。
- AC-3: 中断・quota 超過・破損で未完了を完成と扱わない。保存する blob と保護参照の数を増やしても、1 回の読み出し・回収・保持が固定の窓（128 件、1 MiB）に収まり、`MemStore` に blob-service の内容が残らない。
- AC-4: native の回帰（取得 gate、local-only・ephemeral、保護と cache の境界）と、上流 iroh-blobs の版を上げるときの互換検証（native と WASM の build、roundtrip）の手順を `docs/runbooks/dev.md` に書く。

### 6. 採らない方式

- iroh-blobs を fork し、外部 store API と IndexedDB の store actor を実装する: Context。回収を別に書く必要が残り、最終コード量が約 2 倍になる。
- `MemStore` の内容を定期的に IndexedDB へ書き出す: 起動時に全件を戻すことになり、件数に比例する。
- partial を IndexedDB に保存して再開する: kukuri の大きな remote blob の取得は ephemeral で、bao 単位の部分再開を使っていない。

### 7. docs の自分の record（#1216 W3 AC-1）

事実（基準 commit は統合 branch の同じ head）:

- kukuri が使う iroh-docs の API は、author の import と既定の設定、namespace の import・open・close・drop・`list`（native の 1 回だけの回収。#1407）、`set_bytes`・`get_exact`・`get_many`（`Query` の limit）・`del`、`subscribe`（InsertLocal）、private の証明の `export_secret_key`、`DOC_READ_ALPN` の応答である。
  ranger による同期・ticket・`start_sync` は使わない（#1221 R5-H）。旧 store の退役（`legacy.rs`）は native だけ。
- 本人の record の正本は、native でも docs store ではなく保護所有先（`protect_own_record` → `put_owned_record`、参照 `own_docs`）にある。端末バックアップは docs store を含めず、空の docs から本人の投稿を戻せることを test が確かめている。
- 保持分（held）の record を合わせる範囲は経路で違う。
  - 手元の読み出し: `query_local_source`（LocalSourceReader）は全 replica で合わせる。`query_replica_exact_bounded`・`query_replica_with_policy` は private の replica だけで合わせる（`iroh_local_source.rs` の `public_replica_secret` の判定）。
    `query_replica_by_author` と手元の key の一覧（`iroh_sync.rs` の `key_page`）は合わせない。`key_page` は namespace が無いと空を返す（#1407 の「読むだけでは namespace を作らない」）。
  - 相手への応答（`page_read.rs`）: Exact は全 replica で合わせ、Keys は公開 topic の replica だけで合わせる（#1395）。
    private の証明は、要求の namespace の secret で（replica、namespace、query）に鍵をかけたものを確かめるが、要求の replica の文字列と namespace の対応は確かめず、保持分は要求の replica の文字列で引いている。
- iroh の entry の timestamp は kukuri のどこからも読まない。更新の ID と時刻は署名済み envelope の値を使う。

決定:

- 自分の record の保存は §2 の「record」の操作で行う。Web の実装は §3 の database に object store `records` を足す。
  - key は `[replica, key, author]`。値は `DocReadRecord` の bytes・保護の有無・最後に使った時刻。
  - 索引は `[replica, key]`（exact）、`[replica, author, key]`（author を指定した一覧）、「非保護か・最後に使った時刻」（回収の順）。
  - 自分の record は `own_docs` の参照で保護し、remote の record は §4 と同じ規則で回収する。
  - 実装（W3 AC-2）: 別の object store にせず、native と同じく `contents` の行（`kind` が `record`、key は `<replica>\0<key>\0<author>`）と `chunks` に置く。
    record の行だけが `listing`（`[replica, key, author]`。exact と一覧）と `by_author`（`[replica, author, key]`）の索引に載る。回収の索引・容量の計数・保護参照・完成の判定を blob と共有し、同じ処理を 2 つ持たない。
    一覧は値を読まずに、行に置いた record の hash と長さを返す（native の `json_extract` と同じ）。
- 起動時に record を読まない。docs author はアカウント鍵から導出し直し（ADR 0053）、namespace は需要があるときに開く（公開は replica id から導出、private は W4・W5 が保存する capability から登録する）。全 replica の export・import を起動の条件にしない。
- 自分の record（`own_docs` で保護した行）を合わせる範囲を、手元の読み出しのすべての経路（exact・author を指定した exact・key の一覧）と相手への key の一覧で、全 replica（topic・author・author bucket・private）へ広げる。
  他人の record の保持分を合わせる範囲は今のまま変えない（手元の exact の公開 replica では合わせない。上書きされる他人の record の古い版が、書き手本人への読み出しを止めないため。#1395・#1419）。
  一覧は、手元の先頭 `limit` 件と保持分の先頭 `limit` 件を合わせて先頭 `limit` 件を取る（page_read の Keys と同じ方法で、読む量は `2 × limit` を超えない）。exact は今の上限（8 件）のまま。
  §2 の record の読み出しの操作は、自分の record だけに絞る指定を持つ（native は保護参照 `own_docs` の索引、Web は `records` の保護の有無の索引で絞る）。
  保持分は replica の文字列で引くので、namespace が手元に無くても合わせる（namespace を読むためだけには作らない。#1407）。
  Web は起動のたびに docs store が空なので、これが無いと reload の後に自分の author replica・private の record が手元で読めず、相手への一覧からも消える。native の restore の直後にも効く。実装は W3 AC-3 が所有する。
- 相手へ private の保持分を返すのは、要求の replica に手元で登録した capability の secret で証明を確かめられたときだけにする（要求の namespace がその secret の namespace と一致することも確かめる）。
  docs の namespace ではなく登録した capability で確かめるので、drop した replica にも答えられ、証明に使った namespace と別の replica の保持分を返すことはない。実装は W3 AC-3 が所有する。
- memory store の保持: replica は需要の間だけ開く（既存の lease と close）。Web では、需要が終わって閉じた replica を drop し、その replica でこの session に書いた entry の内容の hash を `MemStore` から消す（書いた record は保存 trait に確定しており、保持分として読める）。
  memory に残るのは、開いている replica のこの session の書込みと、転送中の temp tag だけになる。開いている replica の数は既存の需要の上限に従う。
  drop した replica への読み出しは、namespace を作り直さずに保持分で答える（上の 2 項目）。これを AC-3 の reload・peer 不在の試験で確かめる。
  - 実装（W3 AC-3、2026-10-02 改訂）: 上流 iroh-blobs（0.103.0）の blob の削除は `pub(crate)` で、外から hash を指定して消せない。そこで、hash ごとに数えて消す案をやめ、
    Web の memory store に限って上流の GC（`MemStore::new_with_opts` の `GcConfig`、間隔 10 秒）を有効にし、iroh-docs の保護（`ProtectCallbackHandler`。docs store の entry が指す内容を守る）を渡す。
    閉じた replica は docs-sync が drop する（`close_replica` と、上限を超えた idle の close）。drop した replica の内容は、どの entry も指さなくなるので GC が消す。
    GC が読む（mark・sweep）のは memory の tag・temp tag・開いている replica の entry と memory store の内容だけで、Web ではどれもこの session の開いている分に限られ、件数に比例しない。native は GC を使わない（§1）。
- 再構築・再送は、保存 trait の record をそのまま提供する。docs へ書き直さないので、更新の ID と時刻は変わらない。
  実装（W3 AC-3）: 手元の exact・docs author 指定・key の一覧と、相手への key の一覧（topic の replica 以外）が、保存 trait の自分の record（`own_only`）を合わせる（native は保護参照 `own_docs` の索引、Web は `own`・`own_by_author` の索引）。一覧の併合は `merge_record_keys` を共用する。
- 保存 trait への書込みに失敗したら、書込み全体を失敗として返し、偽の保存成功を返さない。native の `protect_own_record` は cache や account が無いと何もせずに成功を返すが、Web には常に保存 trait があるので、この分岐を Web の保護の成功として扱わない。
- 上流 iroh-docs の patch rev を更新するときの互換検証（native の保存互換、WASM の build と動作）の手順を AC-4 で `docs/runbooks/dev.md` に書く。

採らない方式:

- redb の `StorageBackend` を IndexedDB で実装する: backend は同期 I/O で、main thread から非同期の IndexedDB を使うには DB 全体をメモリに写すことになり、起動時に全件を読む。書き戻しの途中で止まると壊れた状態が残る。
- iroh-docs を fork して非同期の store を足す: store・actor・同期 protocol の約 5,500 行に手が入り、使わない同期 protocol まで書き直すことになる。
- docs store の内容を定期的に IndexedDB へ書き出す: 起動時に全件を戻すことになる。

## Consequences

- Web の blob は、iroh-blobs の protocol ではなく `/kukuri/remote-blob/1` で相手へ届く。native から見ると、既存の fallback の経路を通る。
- 保存 trait が native と Web の両方の正本になり、`SqliteStore` の該当 method は trait の実装へ移る。
- 手元と相手への key の一覧が、全 replica で保持分を合わせるようになる（§7）。native でも docs store を失った後の一覧が変わる。
- 上流 iroh-blobs の版を上げるときは、`MemStore` の API と protocol の互換を AC-4 の手順で確かめる。

## Data classification

ADR 0002 の template に従う。native の remote cache（ADR 0048・`docs/architecture/blob-cache.md`）と同じ分類で、置き場所だけが違う。

- Feature 名: Web の blob の保護と cache
- Durable / Transient: 本人の書込み（保護参照つき）は Durable。remote の内容は Cache（回収される）。
- Canonical Source: 本人の書込みは本人の端末の保護所有先。remote の内容は相手の端末（hash で検証）。
- Replicated?: 既存の protocol で提供する。アカウント同期・移行の必須 bundle には入れない（#1211 の任意の履歴転送は別）。
- Rebuildable From: remote の内容は相手から再取得できる。本人の書込みはブラウザのデータが消えると失われ、他の本人端末・相手の保持分から取り直す。
- Public Replica / Private Replica / Local Only: 内容の公開範囲は native と同じ（取得 gate を通ったものだけを保存する）。
- Gossip Hint 必要有無: なし（変更なし）。
- Blob 必要有無: あり（本 ADR の対象）。
- SQLite projection 必要有無: Web ではなし（IndexedDB）。
- 必須 contract: §5 の AC-2〜4。
- 必須 scenario: W8（#1220）の reload・復帰。
- 新しい外部送信: なし（`/kukuri/remote-blob/1` は既存の protocol）。

## References

- Issue #1213（D-3 の改訂）、#1215・#1216（本 ADR）、#1217、#1221（R5-A・G・I・H）、#1395
- ADR 0048、ADR 0056、`docs/architecture/blob-cache.md`
- iroh-blobs PR #259・#86（open、採らない）

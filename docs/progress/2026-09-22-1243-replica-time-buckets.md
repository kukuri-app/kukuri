# #1243: replica の時間分割

## 現在状態

- Issue: [#1243](https://github.com/kukuri-app/kukuri/issues/1243)
- 状態: In progress（基盤#1294は完了。T1の詳細設計とCN#1293を継続）。親の実装・達成監査は未完了。
- リスク区分: C。
- Scope revision: `2026-09-20-v1`（Issue の AC-1〜5、INVAR-1〜4）。
- CN 側の子 Issue: [#1293](https://github.com/kukuri-app/kukuri/issues/1293)（GitHub の sub-issue 関係も登録済み）。
- 最初の基盤差分の子 Issue: [#1294](https://github.com/kukuri-app/kukuri/issues/1294)。段階ごとの監査対象を固定し、親の未達条件と区別する。
- 調査基準 commit: `70574678582d0ce79760819866f3cce63fa3165e`。
- 2026-09-22: ユーザーが下記の段階計画、Issue 関連作業、コミット、PR 作成、マージを承認。
  マージ条件は必須 CI 成功と対象 PR head の独立監査 PASS。承認は本番デプロイを含まない。
- `.codegraph/` は存在するが `codegraph explore` が利用可能な index なしと返したため、`rg` と直接読み取りへ切り替えた。index は作成していない。

## 承認済み計画

詳細要件の正本は Issue と後続 ADR に置く。本記録は作業と証跡の対応を管理する。

| ID | 作業 | 対象 | 受入条件 / 検証 | 条件 | 依存 |
| --- | --- | --- | --- | --- | --- |
| T1 | ADR・データ分類・inventory・移行表 | docs/adr、docs/architecture | bucket の配置、参照、保持、移行、#1224 との境界が確定。全 caller と sink の逆引き | AC-1・2、INVAR-1〜4 | なし |
| T2 | 変更前再現と contract | docs-sync/app-api/CN tests | 履歴10倍の失敗test、境界越え・取り下げ・移行・禁止I/O | AC-3〜5、INVAR-1〜4 | T1 |
| T3 | bucket 型・ID・鍵導出・参照形式・lifecycle | core、docs-sync、iroh-node | private 分離、版識別、scope 検証、ローカル読みと同期開始の分離、停止/再開/削除 | AC-1・3・4、INVAR-2・3 | T2 |
| T4 | CN 追従（子 Issue） | cn-indexer、関連 cn-* | 新旧を読み、切替・再起動・取り下げで欠落/重複/復活がない | AC-2、INVAR-1・2・4 | T3 |
| T5 | client の書き込み・参照・購読・ページ取得 | app-api、desktop-runtime、store | ADR 0052 の3経路を再利用。境界越えと部分失敗・再起動の回帰を検証 | AC-1〜4、INVAR-1〜4 | T3・T4 |
| T6 | 保持上限・回収・旧形式移行 | docs-sync、store、blob-service、desktop-runtime | 有限な回収と再開、自分の投稿/bookmark/参加状態の保護、取得不能時の操作継続 | AC-2〜4、INVAR-1・2・4 | T5 |
| T7 | 規模・移行・実接続の結合検証 | crate tests、harness/scenarios、必要な desktop tests | 履歴10倍で常時同期entry/起動時読み/同期対象数が増えない。private・3通信経路の回帰 | AC-3〜5、INVAR-1〜4 | T4〜6 |
| T8 | 正本同期と独立監査 | docs、対象 PR head | 全条件の証跡、未分類/不適合0、独立監査PASS、CI成功、merge後一致確認 | 全条件 | T7 |

## 現行コードで確認した境界

1. `crates/docs-sync/src/replicas.rs` の `topic_replica_id` / `author_replica_id` は累積 replica を返す。
   `private_channel_epoch_replica_id` は epoch を分けるが時間は分けない。
2. `post_replica_kind` は topic の接頭辞以後の全体を topic ID として解釈する。
   後ろに bucket を足すだけでは `ReplicaPostScope::for_replica` が topic 不一致になる。
   既存 topic ID は `::` を含められるので、新形式との曖昧な判別も禁止する。
3. `public_replica_secret` は `channel::` 以外を公開導出する。新 private 形式を追加するときは、
   ID の解釈と秘密の導出を同時に更新しないと private namespace が公開になる。
4. `IrohDocsSync::ensure_replica` は handle が無ければ namespace を import して `doc_start_sync` を呼ぶ。
   query もこの helper を通る。`LocalOnly` は現行では主に entry 本体の取得 policy であり、
   bucket の open/sync の許可を代用するものと見なせない。
5. `remove_private_replica_secret` は秘密と map の handle を除き、live task を abort する。
   namespace の永続削除・本文の回収・保存予算の機能ではない。
6. `persist_post_object` は state/envelope/timeline/thread 索引を同じ replica へ別々に書く。
   途中失敗からの再開と境界時刻の固定が必要。取り下げも現在は同じ replica の target ごとの key。
7. author replica は profile/posts だけでなく、profile/latest・follow/block・custom reaction asset・
   live/game 用 manifest/preset も共有する。profile/posts だけの移動で author 全体の上限を達成した扱いにしない。
8. `ScopeReplica::from_scope` は public を `topic::<id>`、private を `channel::<id>` へ写像する。
   CN の private secret 登録もこの旧形式に結び付く。`ingest.rs` の置換だけでは CN は新形式へ参加しない。

## inventory の列挙方法

以下を production と tests/scenarios に分け、入口から sink と sink から caller の双方を確認する。
テスト内の helper 使用は本番入口には数えず、対応する移行・回帰証跡として分類する。

```powershell
rg -n 'topic_replica_id|author_replica_id|private_channel_epoch_replica_id|private_channel_replica_id|post_replica_kind' crates harness --glob '*.rs'
rg -n 'open_replica|apply_doc_op|query_replica|subscribe_replica|restart_replica_sync|register_private_replica_secret|remove_private_replica_secret' crates --glob '*.rs'
rg -n 'source_replica_id|ReplicaId::new|topic::|channel::|author::' crates harness --glob '*.rs'
```

計画の INV-1〜4 は具体化し、INV-5〜8 を追加する。全 member の列挙と適合判定は未完了であり、未分類0とは報告しない。

| ID | 入口 group | sink / guard | TR | 担当 Task |
| --- | --- | --- | --- | --- |
| INV-1 | 投稿・返信・repost・reaction・取り下げ・session、persist_* | docs/blob/projection/hint、署名/bucket/audience | 1・2・7・10 | T2・5 |
| INV-2 | 起動・購読・SyncFinished・Lagged・hint・遡り | open/sync/key反映、窓/作業集合/LocalOnly | 3・4・8 | T2・5 |
| INV-3 | CN participant・worker・ingest・source resolution | 同期/索引、許可scope/新旧互換 | 2・5・8 | T4 |
| INV-4 | 更新・restart・restore | schema/参照/移行位置、データ保全 | 5・6・9 | T5・6 |
| INV-5 | author 購読・profile・follow/block・アセット | author docs/索引、最新状態/署名者/上限 | 3・6・10 | T5 |
| INV-6 | 境界・resume・reconnect・peer更新 | 差分sync/task停止、休止bucketを起動しない | 3・7・8 | T3・5 |
| INV-7 | 容量到達・cache回収・bookmark保護 | docs/blob/projection削除、保護参照/再開 | 6・9 | T6 |
| INV-8 | epoch切替・退出・再参加・成人向け/同意guard | 鍵登録/取得/sync、未許可I/O禁止 | 8 | T3〜7 |

## 状態遷移

| ID | sequence | 期待結果と禁止する副作用 |
| --- | --- | --- |
| TR-1 | 境界前の投稿→境界後の返信/reaction | thread がつながる。元bucketに無期限に追記しない |
| TR-2 | 古い投稿の取り下げ→購読者への反映/新規clientの遡り | 本文/添付を表示しない。到着順逆転/確認情報不足/CN de-indexも扱う |
| TR-3 | 古い履歴10倍→参加/restart/sync開始 | 窓と読み出し数が不変。休止replicaを全列挙しない |
| TR-4 | 遡り→peer不在/空bucket連続/cancel | 取得量/試行/open数が有限。続き位置と操作を維持 |
| TR-5 | CN更新→client更新→書込み切替→途中失敗/restart | 投稿の欠落/二重反映なし。旧版の範囲/移行終了を明示 |
| TR-6 | 旧replicaのみ→更新/restore | 投稿/bookmark/参加状態を保全。起動時全件移替えなし |
| TR-7 | 未来/過去の申告時刻・時計逆行・長期offline | 時刻/bucketを検証。任意bucketの無制限openなし |
| TR-8 | epoch切替/退出/失効/同意不足、mixed scope | 未許可sync/取得/書込み0。別scopeの成功で迂回しない |
| TR-9 | 容量上限→回収中断→restart | 保護データを保全し、有限batchで再開 |
| TR-10 | 継続sessionの境界通過、author変更後の新規参加 | 継続/最新状態を取得。過去全件再生なし |

## 検証

- 初回 `cargo xtask check`: 成功（2026-09-22、空のworktree targetからbuild）。実行中にcontractと基盤の編集を進めたため、
  厳密な変更前commitの全体結果としては扱わない。Rust clippy、Tauri check、frontend lint/typecheckを実行。
- 実装後: path 別マトリクスに従う。`check` / `test` / `oversized-files`、`cn-check` / `cn-test`、
  `app-api-slow-test`、`e2e-smoke`、public/multi-device/private connectivity の関連 scenario。
- UI/IPC を変える場合は ADR 0014 の成果物と desktop/Tauri の該当検証を追加する。
- 時間閾値を件数非依存の証明にしない。実Irohとtest doubleの回数・entry数の双方を証跡にする。
- 設計の独立評価を実施中。PR head の完了監査とは別であり、マージ条件の代わりにならない。

## 設計段階の独立評価（2026-09-22）

別コンテキストが基準commit・Issue・ADR0052/0053・現行コードから評価。判定はINCONCLUSIVE（設計確定前）。
ユーザーの追加承認が必要なscope変更は現時点で見つかっていない。具体的指摘と処置:

- 取り下げは元bucketの対象別最新state＋現在bucketへの通知とし、二重書込みをoutboxで再開する。
- 伝播の完全保証や不正候補だけでの非表示を追加しない（ADR0053の維持）。
- authorの対象別stateだけでは未知targetを発見できないため、既存の各512件窓と同じ順序の有限rosterを追加する。
  制御領域へ全edgeや過去envelopeを積まず、索引とLIMITで更新する。
- 保存予算をcacheと保護データへ分け、本人の投稿1件のためにbucket全体を保護しない。
- 旧replicaの常時同期には終了点を持ち、restoreでも終了済み状態へ戻る。
- private/未知形式のIDを公開secret導出へ流さない。ID解釈とsecret導出を同じcontractで固定する。

これは達成監査ではない。上記の実装とcontractの成功はまだ示していない。

## 最初の基盤差分

- 日単位の `TimeBucket`、canonicalな `BucketReplica` の生成/解析、epochからのprivate secret導出を追加。
  現行の旧ID生成helperとwriterの保存先は変更しない。
- 新形式のscopeを投稿検証へ渡し、署名済みcreated_atとbucketの不一致を拒否する。
- `LocalOnly`/key列挙のローカルopenと明示syncを分ける。peer再適用はsync_requestedだけへ適用する。
- close/revokeはowner保持の有限JoinSetで完了させる。背景restartは停止済み/ローカルのみを再開しない。
- caller/sinkと局所transitionは [基盤inventory](../architecture/replica-bucket-foundation-inventory.md)。
- 親のAC-3〜5は未達。時間bucketへ切り替えるwriter、CN、作業集合、回収・移行はまだ入っていない。

### failing-before / passing-after

| contract | 変更前の結果 | 変更後の結果 |
| --- | --- | --- |
| `bucket_post_scope_is_decoded_before_integrity_validation` | scopeがNoneで失敗 | 成功 |
| `private_or_unknown_bucket_cannot_derive_a_public_secret` | private新IDから公開secretを導出して失敗 | 成功 |
| `local_only_bucket_lookup_does_not_start_replica_sync` | 実iroh-docs status.syncがtrueで失敗 | 成功 |
| `bucket_post_must_match_its_signed_creation_time_and_scope` | 別時刻bucketの投稿がOkとなり失敗 | 成功 |
| `bucket_close_retries_leave_failure_without_repolling_the_event_task` | leave失敗後の再closeでJoinHandle二重poll panic | Optionでtakeする修正後に成功 |

最初の3件は `cargo test -p kukuri-docs-sync --lib bucket -- --nocapture`、4件目は
`cargo test -p kukuri-app-api --lib bucket_post_must_match -- --nocapture` で確認。
docs-sync全体の途中検証は58成功、計測用1件ignore。追加したlifecycle fault/cancel contractを含む最終検証は別途記録する。

- 最終差分の `cargo xtask check`: 成功。fmt、non-CN clippy、Tauri compile、frontend lint/typecheck。
- `cargo test -p kukuri-docs-sync --lib bucket`: 12成功（所有前cancel test追加前。追加後は全体testに含める）。
- `cargo xtask test`: 成功。Rust 1,338成功・5 skip、doctest成功、frontend 246 files / 1,984成功。
  所有前cancelを含む最終の基盤contractもこのrunに含む。
- private bucket鍵のgolden追加後: 完全名指定のtargeted test 1件成功、対象test fileのrustfmt check成功。
  最初の短いfilterに `--exact` を付けたrunは0件だったため証跡には使わず、完全名で実行し直した。
- `cargo xtask app-api-slow-test`: 438成功、doctest成功（実Iroh、seeded DHT、relay-supported同期を含む）。
- `cargo xtask e2e-smoke`: `desktop_smoke_post_persist` 6 steps / pass。
- `cargo xtask scenario community_node_public_connectivity`: 15 steps / pass、connected=true、peer_count=1。
- `cargo xtask scenario private_channel_invite_connectivity`: 11 steps / pass、connected=true、peer_count=1。
- `cargo xtask oversized-files`: 成功。既存baselineの上限を増やしていない。
- `git diff --check`: 成功。UI/IPC形状変更は無く、desktop-ui-check / tauri-testはこの基盤差分には適用しない。
  基盤差分にはCNのsource変更は無く、cn-check/cn-testは#1293で実施する。CIと監査の確定結果は次節。

### 基盤のマージ・完了

- PR [#1297](https://github.com/kukuri-app/kukuri/pull/1297)、head `b443751bd098a47d420eee9bc2b6d25b472d30d9`。
- head独立監査: [PASS](https://github.com/kukuri-app/kukuri/pull/1297#issuecomment-5771688041)。BF-1〜7は7適合/不適合0/未分類0。
  別コンテキストでbucket test 13件と投稿検証1件を独立実行して成功。
- 全13CI成功後、2026-09-22に `dba0881da72f8bca974f3e9d76c4d13c434894e0` へsquash merge。
- merge後に#1290/#1296のdeltaを[独立監査してPASS](https://github.com/kukuri-app/kukuri/pull/1297#issuecomment-5771813426)。
  対象21pathの20pathはblob一致、残るtests/sync.rsは他test helperの可視性/re-exportのみ。関係する新callerもguardを維持。
- #1294はCompleteへ更新してClose。#1243のAC-3〜5の達成根拠には使わない。

独立コードレビューの所見も実装中に修正した（PR headの最終監査とは別）:

- Doc::close RPCの途中失敗でclosed handleを残す問題: 停止の所有task化、mapからの隔離、応答喪失test。
- stale restartが停止後に再同期する問題: registry lock内で既存active handleだけを再同期。
- private secret削除後のcancelで同期だけ残る問題: secret削除もownerの同じtaskへ移す。
- leave失敗後のJoinHandle二重poll: 消費済みtaskを隔離handleへ残さない。

## 後続段階で使う実装上の確認

- author rosterを生成するとき、既存の `Store::list_follow_edges_by_subject` / `list_block_edges_by_subject` は使わない。
  SQLite実装は `fetch_all` で、並びもupdated_at優先。rosterのtarget key順・LIMIT付き入口を別に追加する。
  followの `(subject_pubkey,target_pubkey)` 主キーを範囲読みへ利用できる。
- reaction IDの現行導出は物理 `source_replica_id` をhashに含む（core/reactions.rs）。時刻bucketだけを差し替えると
  解除/再有効化が別reactionになる。T5で論理scopeと更新eventの物理bucketを分け、旧ID互換をcontractにする。
- upstream iroH `Doc::leave` は `kill_subscribers=false`、`drop_doc` はtrueである。
  基盤のcloseは保存namespaceの削除ではない。T6のnamespace回収は、永続entry/秘密だけでなくengine側metadataも
  回収する `drop_doc` まで含める。停止APIだけで保存/metadataの上限を達成したとは扱わない。

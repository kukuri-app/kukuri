# #1243 / #1293: 公開indexのsource locator

## 対象と現在状態

現行の作業順序・判断・達成状態は [#1221本文](https://github.com/kukuri-app/kukuri/issues/1221) のP1で管理する。
旧子IssueのCloseは管理統合であり、本差分の未完了条件を消さない。

- In progress。承認済みT3〜5 / #1293 AC-1・2、親INVAR-1・2の読取り準備。
- 区分C、現行は#1221 Scope revision `2026-09-22-consolidated-v2`のP1。旧SL-1〜4を継承する。
  初期基準 `e652c3df46718d81318ea2462869cbed79863145`、main追従後の基準 `02ab5196334f9b3ffeaa0c7456b22b9be98cda56`。
- source locatorをCNの応答→UI→app-apiへ渡し、公開投稿を保存済みの指定replicaから署名検証して解決する。
- writer・private epochの切替・remote取得owner・GCはこの先行差分に含めない。親の完了条件からは除外しない。

## 固定inventoryと契約

| ID | 入口 / helper / sink | guard・遷移 | 検証 |
| --- | --- | --- | --- |
| SL-1 | CN search/discovery/recommendation → index_query_response → IndexEntryView | public bucketだけoptional sourceを返す。legacy/privateはfield欠落を維持し既存resolverへ進む。locatorは権限の証明でない | handlerのred/green、protocol wire互換 |
| SL-2 | CommunityIndexWorkspace.resolveInputForEntry → IPC input | fieldをそのまま渡し、canonical本文・権限をUIで生成しない | workspace test、生成TSと型検査 |
| SL-3 | resolve_community_index_posts → public source検証 → bounded hydration → projection | topic/版/種別照合がread前。署名とbucketを検証。未知/不一致で禁止I/O0、欠落はunresolved | bucket_locators、既存private/scale contracts |
| SL-4 | cache hit・再表示 → withdrawal確認 / view生成 | 保存済みcanonical sourceを優先。取り下げをLocalOnlyで再確認。repost/返信を含め背景remote/open/subscribe0 | cache後の取り下げ、背景処理の負例 |

sourceなしは旧経路を維持する。sourceありの取得はdocsと本文のLocalOnlyに限定し、ネットワーク作業集合の
ownerを別に増やさない。取得不能でも既存の未解決表示を返す。1 requestは既存の100件上限を維持する。
sourceはcanonicalな旧public IDまたはv1の同じpublic topicのbucketに限る。private locatorはまだ受け入れない。

UI分類はデータ受渡しの不具合修正。対象は「見つける」の利用者で、目的は検索結果を署名済み投稿として
解決すること。frontend差分は既存入力へのoptional fieldの転送と、canonical本文の取得状態の保持。
既存PostCardの文言・入力・focus/scroll契約に構造差分はない。専用の見た目のbefore/afterやnative入力確認は非該当とし、
componentの転送/操作contractとdesktop-ui-checkで既存surfaceの回帰を確認する。
未解決時は既存の検索previewと操作制限を維持し、不正な1件で他の正常な結果を消さない
（DESIGN §4.2 / §10、ADR0014の小変更に適用する確認範囲）。新規layout/token/localeは導入しない。

順方向・逆引きのmember:

- SL-1: `index_search` / `index_discovery` / `index_recommendations`の既存query gateから`index_query_response`へ集約。
  `IndexEntryView`のproduction構築はこの関数だけ。harness/testの構築はNoneを明記し、旧応答を固定する。
- SL-2: `CommunityIndexWorkspace.resolveInputForEntry` → `DesktopApi.resolveCommunityIndexPosts` →
  Tauri `commands::posts::resolve_community_index_posts` → runtime
  `content_profile_api::resolve_community_index_posts` → app-api。
- SL-3: `valid_public_index_source`がnew locator経路の読取りを支配。`resolve_public_index_source` →
  `hydrate_object_in_topic_with` → `load_post_with_hint` / `VerifiedPost` → withdrawal確認 → projection。
  sourceを含むgroupの失敗は`failed_entries`に保持し、その結果の表示だけを止める。
- SL-4: `refresh_local_index_reference_withdrawals` → bounded withdrawal helper、
  `page_to_view_with_policy` → `row_to_view_with_cache` → `reply_preview_for_object_id_with_policy` /
  `repost_snapshot_to_view_with_profiles`。LocalOnlyはremote schedulerへ到達しない。
  既存の`page_to_view`と`reply_preview_for_object_id`、profile/bookmark側の呼出しは従来policyを渡す。
  body/blob・docsのremote取得、open/subscribe、projection/withdrawal mutationが照合対象のsink。

## 再現・検証

- app-api: bucketに署名済み投稿があってもsourceが無視され、`None`を返すredを確認。指定bucketからの解決へ修正しgreen。
- CN handler: 応答のsource_replica_idが`Null`であるredを確認。publicの保存済みlocatorを返すように修正しgreen。
- 不正topic/private/author/未知版/非canonical ID: docs read/open/subscribe 0の負例は成功。
- frontend CommunityIndexWorkspace: sourceの転送を含む30件成功。
- frontendのview adapterがcanonicalな`Missing`を`Available`に置き換えるredを確認した。
  LocalOnlyでは本文blobの欠落が正当な結果になるため、canonicalな取得状態をそのまま共有PostCardへ渡す。
  index textを本文の代替にはしない。
- 独立予備reviewで、cache hit時の取り下げ未確認とview生成の背景remote起動を検出。修正前にそれぞれ
  withdrawal未反映・remote docs呼出し1回でredを確認し、LocalOnly再確認とview policy伝播でgreen。
- cachedな返信先/repost元も表示前に高々2参照の取り下げをLocalOnlyで確認する。cacheの正本sourceを優先し、
  cacheが無ければ従来refで分かる1つの保存先だけを読む。全bucket探索はしない。
  返信previewの本文が残るredを確認し、両参照形式で本文非表示・remote0のgreenを確認。
- bucket_locators 8件成功。本文非localの返信、repost、返却後の背景taskも含めremote/open/subscribe0。
- 別scopeのcacheに当たる不正結果が、同じgroupの正常な結果まで未解決にするredを確認。
  新locator経路の失敗を結果ごとに分離し、正常な結果を維持するgreenを確認した。
- producerはlegacy/privateのlocatorを省略する。legacyにfieldが付くredを確認し、修正後のhandler 2件は成功。
- IPC型は`cargo xtask ipc-types`で生成。ローカル必須check/test、正式head監査、CIは最終差分で行う。

`ed749b1f`のローカル検証（下記監査FAILの修正前）:

- `cargo xtask-lite ipc-types` / `check`: 成功（Rust静的検査、Tauri compile、frontend lint/typecheck）。
- `cargo xtask-lite rust-test`: 1,367成功、5 skip。doctestも成功。
- `cargo xtask-lite app-api-slow-test`: 実Irohを含む463成功。
- `cargo xtask-lite cn-check` / `cn-test`: 成功。cn-testは734 reported passed、Postgres/Valkey gate有効。
  専用compose project `kukuri-1243-locator-validation`（port15445/16392）を用い、終了時に削除した。
- `cargo xtask-lite desktop-ui-check`: lint/typecheck、Vitest 249 files / 2,002 tests、Storybook build、
  browser 371件、visual smoke 44件が成功。Windowsのvisualはsnapshot比較が無効で、描画・操作smokeの証跡。
  #992のtestが固定pathへ再生成する既存説明画像2枚は、この差分へ含めず元の内容へ戻した。
- `cargo xtask e2e-smoke`: desktop_smoke_post_persist、6 steps成功。
- `cargo xtask-lite oversized-files` / `git diff --check`: 成功。baseline増加なし。
  oversized初回は並行実行中のxtask.exeを置換できず失敗し、他commandの終了後に再実行して成功した。
- PR headの正式独立監査とCIは次の工程。parent全体の完了はこの検証で判定しない。

## 独立監査の指摘とLocalOnly bytes境界の修正（main追従前の履歴）

- `ed749b1f`は[独立監査FAIL](https://github.com/kukuri-app/kukuri/pull/1303#issuecomment-5774403993)。
  SL-1/2/4適合、SL-3不適合。local status確認後にremote可能な`fetch_blob`へ進む経路が残っていた。
- `Available` / `Pinned`の宣言後に本体が読めないfixtureで、remote-capable fetch 1回（期待0）のredを確認。
- 必須trait method `BlobService::read_local_blob`を追加。Memoryはmap、Irohはlocal storeの`get_bytes`だけを読む。
  ReloadableBlobServiceも同APIへ転送する。既定実装や`fetch_blob`へのfallbackは置かない。
- `fetch_local_projection_blob_text`は専用APIをtimeout付きで使い、読めなければ未取得として返す。
  本文が未解決なら、statusがPinned/AvailableでもviewをMissingに保つ。
- 上記fixtureはgreen。実Irohの既存2ノードcontractにも、receiverのpinだけがあってbytesはsenderだけが持つ
  ケースと、存在するlocal bytesの読取りを追加して成功。通常remote取得を対照にしてpeerの取得可能性も確認する。
- 新sinkのproduction実装はMemory/Irohの2つ、転送はReloadableの1つ。test double 18実装も明示的に
  local storage・local不可・local読取り失敗を区別して実装した。検索式は`impl.*BlobService`と`read_local_blob`。
  production callerは`fetch_local_projection_blob_text`（object hydrationとreply targetのLocalOnly分岐）。
  Memoryの通常fetchも同じlocal map読取りへ委譲する。Irohの通常fetchのremote経路は変更しない。
- 修正差分の必須検証とSL-3/影響callerのdelta監査を再実施する。変更のないfrontendゲートは上記証跡を再利用する。

## private replica再接続で検出したdocs actorのpanic

- LocalOnly修正後の全`rust-test`で、`friend_plus_share_freeze_rotate_connectivity`が2回失敗した。
  新しいshareのimport時にdocs actorが終了し、上流`net/codec.rs`の`progress.unwrap()` panicを記録した。
  同じcaseの単独実行は成功しており、単独成功を全体検証の代替にはしない。
- `BobState::run`はprogressをtakeしてから非同期処理を行い、エラー時には戻さない。
  呼出し側はエラーでも`into_outcome`を呼ぶため、空のprogressでpanicする。
  #1294固定AC-4のclose/revoke/retryから到達するExisting-gapとして、独立確認後に#1294をReopenした。
  #1297による回帰とは確定していない。情報漏えいもこの失敗からは観測していない。
- [上流PR #114](https://github.com/n0-computer/iroh-docs/pull/114)の
  `e7233d14853cb4db9966e30050bac1e689cdeec8`へrootとstandalone Tauriの両workspaceを固定する。
  このPRは採用時点で未merge。成功時にだけoutcomeを返すことで、失敗時のunwrapを除去する修正。
- registry 0.101.0の元commit `091e8cac47bbc49cdb84b0bfed227cc163b61dfe`からは2 commit。
  先行する`b53c3179f2daf6bcd90f30ce80cdd9572502c6a1`（上流#110）は、不正gossip messageのdecode失敗を
  当該messageの破棄に留め、受信loopを終了させない修正と回帰test。製品sourceの差分はこのgossip処理と
  `net.rs` / `net/codec.rs`。上流Cargo.tomlの変更はない。
- 両Cargo.lockはiroh-docsのsourceとchecksumだけを変更し、両workspaceの`cargo metadata --locked`が成功。
  `cargo update`が生じさせた無関係な依存edgeの変更は含めない。
- 全体Rust検証、slow test、CN検証、Tauri検証、接続scenarioと独立delta監査を新しい依存で行う。
  upstream修正が正式releaseされた後のpin解除は、同等修正の包含確認と依存更新時の検証を条件とする。
- 修正後の`cargo xtask rust-test`: 1,368成功、既存skip 5件、doctest成功。
  `friend_plus_share_freeze_rotate_connectivity`と、docs-syncのclose応答喪失・leave失敗retry・
  close/revokeのcaller cancel・所有前cancelの4契約は、固定先の新依存でいずれも成功。
  suiteの並列度・timeout・skip対象は変えていない。
- `cargo xtask check`も成功。その後の`app-api-slow-test`はcompile中にユーザーの
  #1224先行・#1243停止の指示で中断した。新依存でのslow/CN/Tauri test・接続scenario、
  固定headのdelta監査とCIは未完了。未commitの修正を保持し、#1303はmergeしない。
- その後、ユーザーが#1224前に既知の手戻り部分の修正を依頼。上記の既存修正の検証を再開した。
  新依存で`app-api-slow-test`は464件成功、`cn-check`も成功。停止処理とtask所有の分離は別worktree・別PRで進め、
  この不具合修正へ構造整理を混ぜない。writer切替や共通ownerの設計・実装は再開していない。
- CN通常testは734 reported passed。続くdoctestは別worktreeとtargetを並行共有した際に
  E0463で失敗し、直列でのCN doctest再実行は成功。別profileにも古い成果物が残り、xtaskのbuildで
  IndexEntryViewのfield不足となったため、対象2crate（cn-protocol/docs-sync）の生成物だけをcleanした。
  製品コード変更なしで再buildでき、`tauri-test`は77件成功。以後、別worktreeのtarget共有で並行buildしない。
- 今回の先行整理は別PR #1305に限定する。#1303の修正はローカルに保持し、PRは保留。
  新依存でのe2e-smoke・接続scenario、修正後headの正式delta監査・CI・mergeは、#1243再開時に残る。

## 直前のCN準備PR

- [#1300](https://github.com/kukuri-app/kukuri/pull/1300): 独立監査PASS、全14CI成功、上記基準commitへmerge。
- 監査対象16ファイルはmerge後も一致。
- UIの既存authorTrustGate待機testが初回だけ失敗。ローカル4件成功後に失敗jobだけを再実行し成功。
  UIコード変更・test除外はしていない。[merge照合](https://github.com/kukuri-app/kukuri/pull/1300#issuecomment-5773269562)。

## #1221 P1としての再開・main追従

- ユーザーが意思決定を要する時点までの自律進行を承認。main `02ab5196`へrebaseし、
  ローカルのsource commitは`acd62c3a`。未commit修正の再適用時の7ファイル競合を解消した。
  元差分はautostash `07304e03`にも保持されている。
- mainの#1301に既存`BlobService::fetch_local_blob`とキャンセル可能な表示用取得があるため、
  未公開の`read_local_blob`案は追加せず、既存APIへ統一する。Memory/Iroh/Reloadableの本番実装は
  mainのまま使用し、既定のNone応答もremoteへ進まない。#1301の表示取得・キャンセル契約を維持する。
- 必須trait追加のためだけに作ったCN test doubleの変更は除き、LocalOnly本文helperのcallerで必要な
  fixtureだけを更新する。pinのみの実Iroh負例とAvailable/Pinned stale状態のremote 0契約は残す。
- mainのsession処理・表示API・停止処理と統合したため、以前の全suite結果を新しい差分の結果とは扱わず、
  対象contractと関連する全体gateを再確認する。正式headの独立監査・CI・mergeまでP1を継続する。

## CN回収差分のP1統合と検証方針

- #1293は管理上Closeし、未commit差分49ファイルを独立バックアップに保全した。
  既存の公開wire `source_replica_id`に統一する。版/scope/bucketはcanonical replica ID、
  object/authorは既存欄で照合できるため、未公開の並行wire `SourceLocator`は同時導入しない。
  private epoch/manifest参照の拡張は#1221 P2/P4に残す。
- 回収した2つの不具合を現在差分でも先に再現した。不存在bucket10件でnamespace 0→10、
  古い検索投影からwrong-source/wrong-authorが返る。compile失敗は再現証拠に含めない。
- `LocalSourceReader`で新public locator経路を既存namespaceのexact-key読取りに限定する。
  `DocsSync::query_local_source`はMemory/Iroh/Reloadableと専用fixtureで実装し、通常queryへのfallbackはしない。
  Irohは既存32枠のlifecycle ownerを使い、registry guardでclose/revoke/restartと直列化する。
  import/sync/subscribe/secret登録をせず、一時Docを必ずcloseする。未対応adapterはエラー。
  pinned iroh-docsがRPCに変換する明示的なOpenError::NotFoundだけを空結果へ正規化し、他のI/O/actorエラーは保持する。
- cache hitと関連参照のwithdrawal確認も同じreaderを使う。namespace削除後も検証済みprojectionは保持し、
  namespaceを再作成しない。既知の取り下げ・署名・topic/channel照合は従来の共通経路を維持する。
- `SurfaceableEntry`に確定済みsource/author/timeを載せ、query gateで古い投影のmetadataを補正する。
  supported scope・安全性・private検索のgateを変更しない。
- 回収CN差分のprivate leave/epoch registry/manifest locatorは未導入のprivate拡張に属するためP2/P4へ残す。
  不正scopeとcached sourceの扱いは#1303の既存contractを再利用し、並行resolverを重ねない。
- 新user指示: ローカルは変更に関係する検証だけ。全体testは差分を固定したPRのCIで実行する。
  中断したslow testを成功扱いにせず、通常PR CIにない必要suiteは既存の手動nightlyを同じheadで実行する。

統合後の関連検証:

- app-api `bucket_locators`（iroh-integration-tests有効）11件成功。不存在namespace増加0、
  namespace削除後のcache表示と増加0、LocalOnlyのremote 0を確認。
- docs-sync `local_source::tests`2件、`lifecycle::tests`4件成功。既存sync ownerを保持し、closingの隔離も維持。
- cn-indexer `query_contracts`7件成功。確定済みsource/author/timeと既存の公開/非公開・安全性gateを確認。
- cn-coreの関連Postgres2件成功（integration gate有効、専用project `kukuri-1221-p1-targeted`、port15449、終了時に削除）。
- frontend CommunityIndexWorkspace/キャッシュ本文状態の2files・54tests成功。
- 変更した5crateのclippy（all-targets、warnings拒否）、oversized-files、diff checkが成功。baseline増加なし。
  正式head独立監査と全体CIはPRで確認する。全体scopeの再探索や微修整を繰り返さない。

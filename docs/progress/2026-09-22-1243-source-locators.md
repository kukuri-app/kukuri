# #1243 / #1293: 公開indexのsource locator

## 対象と現在状態

- In progress。承認済みT3〜5 / #1293 AC-1・2、親INVAR-1・2の読取り準備。
- 区分C、Scope revision `2026-09-22-v1`。基準 `e652c3df46718d81318ea2462869cbed79863145`。
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

最終ローカル検証:

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

## 直前のCN準備PR

- [#1300](https://github.com/kukuri-app/kukuri/pull/1300): 独立監査PASS、全14CI成功、上記基準commitへmerge。
- 監査対象16ファイルはmerge後も一致。
- UIの既存authorTrustGate待機testが初回だけ失敗。ローカル4件成功後に失敗jobだけを再実行し成功。
  UIコード変更・test除外はしていない。[merge照合](https://github.com/kukuri-app/kukuri/pull/1300#issuecomment-5773269562)。

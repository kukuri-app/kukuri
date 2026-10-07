# ADR 0063: 公開 blob の保持端末の発見（Mainline DHT と kukuri の補助 index）

## Status

Accepted（Issue #1632、Scope revision `2026-10-07-r2`）。依存の fork（AC-1〜AC-3）と native の公開参照・告知・取得（AC-4）を §1〜§6 に、Community Node の保持端末の検索と補助 index の process（AC-5）を §7 に固定した。Web の取得（AC-6）は、その成果で §8 に足す。

## Context

公開投稿の本文・添付は、手元と少数の既知の端末（`PeerAddrBook::ranked_peers_for` の最大 4 件）に無ければ取れなかった。#1596 の PoC は、Mainline DHT の告知と補助 index（UDP の送信元 address → 署名つき endpoint ID）で未知の保持端末を見つけ、既存の検証つき P2P 取得へ進めることを Testnet で確かめた。同時に、共有の index client は呼出しを落としても要求を送り続ける（1,200 bytes）こと、n0-mainline の検索は受信先ごとに取り消せず保持する応答に上限が無いことも確かめた。

参加者・投稿・blob は件数の上限なく増える（AGENTS.md「件数に依存しない処理」）。発見は全参加者・全 blob の一覧を作らず、少数の候補を期限つきで扱う。

## Feature Data Classification

- Feature 名: 公開 blob の少数候補の再利用と、Mainline 上の保持端末の発見。
- Durable / Transient: 公開参照の索引と告知の予定は account ごとの派生の保存（`kukuri.db`）。DHT と補助 index の記録、見つけた候補、進行中の要求、告知の成功は期限つきの一時情報。
- Canonical Source: 検証済みの公開記録と、実際に返せる保持（本人の保護つきの保存物・remote cache）。DHT の候補は保持・権限の証明ではない。
- Replicated?: Mainline に infohash（BLAKE3 hash から導く）と保持端末の UDP の到達情報、補助 index に署名つきの endpoint ID の record を告知する。内容は既存の P2P で取得する。
- Rebuildable From: 公開記録の行・現在の保持・取込みの位置から小分けに作り直す。再起動・復元の後は前の告知の成功を信頼しない。
- Public Replica / Private Replica / Local Only: 公開由来だけが告知・検索の対象。private channel・DM・秘密値・cache の一覧は送らない。
- Gossip Hint 必要有無: 不要（既存の投稿通知と端末の学習を維持）。
- Blob 必要有無: 既存の SDK と cache の配信を使う。BLAKE3/Bao の検証と、保存前の確認を維持する。
- SQLite projection 必要有無: 公開参照の索引と告知の予定の表を足す（migration `20261007000000_public_blob_discovery`）。
- 必須 contract / scenario: Issue #1632 の T1〜T12。

## Decision

### 1. 公開の根拠（D3）

- 対象は、公開 topic の検証済み投稿の本文・添付・repost の snapshot の添付・リンクプレビュー画像、検証済み profile の画像、公開 topic の有効な custom reaction の asset。live session・game room・Dome の資産は対象外とし、DHT の告知・検索をしない。
- 公開参照の索引 `public_blob_refs(source_kind, source_id, blob_hash)` を、公開記録を書く transaction で記録ごとに置き換える（`post`・`link_preview`・`profile`・`reaction`）。投稿の取り下げと projection の回収は、その投稿の `post`・`link_preview` を外す。同じ hash を別の公開記録が参照している間は公開のまま。
- custom reaction の公開参照は、reaction を書くときの対象の投稿の行で決める（対象の投稿が後から届いても見直さない）。
- DM・private channel・pin/保護だけの保存物は公開の根拠にしない。`own_blob:` などの保護参照は、公開の判定にも告知の目印にも使わない（優先度だけに使う。§2）。
- 導入前の行は、告知の task が種類ごとに rowid の位置から 1 回 128 行ずつ取り込む（`public_blob_ref_backfill`。投稿・profile・reaction。終えた種類の行は消す）。リンクプレビュー画像は手元に表の行が無いので取り込まず、record を書く・読み直すときに載る。導入後に書く行は書込みの transaction で載るので、位置を巻き戻さない。

### 2. 告知の資格・予定・上限（D4）

- 予定 `public_blob_announcements(blob_hash, own, next_at)` には、公開参照と返せる保持（remote cache の `blob` の行で、保護か 7 日以内の利用）の両方がある hash だけを、1 端末 512 件まで載せる。満杯なら本人の blob（`own_blob:<hash>` の保護参照）を先に、次に cache の利用が新しいものを残し、優先の最も低い 1 件と入れ替える（予定は上限の件数しか読まない）。
- 公開参照・保持が変わる書込み（記録の置換、cache への保存、cache の行の削除・回収）は、同じ transaction で予定を合わせる。告知の直前にも、保持が読み出しの期限内かを利用時刻を変えずに確かめ、期限の切れた保持（回収の前のもの）と、他の経路で消えた保持（成人向け表示の OFF での削除等）の予定を外す。告知のために保護参照を足さず、3GiB・7 日の保持の条件を変えない。
- 告知の task（node ごとに 1 つ）は、補助 index の client が組み上がってから動く。開始時（起動・stack の作り直し・復元の後）に予定のすべてを「すぐ」にし、時刻の来たものを同時 2 件まで告知する。取得と同じ実行枠（node の 8 実行）の Background の lane で受け付け、1 件 30 秒で打ち切る。成功したら約 10 分後（hash ごとに 0〜60 秒ずらす）、失敗したら 30 秒後に更新し、失敗の後は task も 30 秒待つ。予定に時刻の来たものが無ければ、5 秒ごと（次の時刻が近ければその時刻）に見直す。
- 告知は fork の `Announcer` で 1 件ずつ行う（`get_closest_nodes` の後に `announce_peer`）。最初の告知で、端末の endpoint ID を DHT の UDP の送信元 address と結び付けた署名つき record の補助 index への登録が始まり、約 30 分ごとに更新される（補助 index 側の保持は既定 1 時間）。補助 index がまだ record を持たない間の告知は失敗として扱う。
- 更新を止めた告知は期限で DHT から消える。即時に全 DHT から消すとは扱わない。

### 3. 取得（D1）

- 通常・表示・file の取得は、`fetch_bytes_from_remote` の 1 つの経路で行う。既知の候補（`ranked_peers_for`：hash 別の取得元と端末の台帳の最大 4 件）を先に試し、手元か既知の相手から取れれば DHT を検索しない。
- 既知の候補で取れず、node が発見を持ち、hash が公開参照を持つときだけ、DHT で保持端末を検索する（`Resolver::resolve_stream`）。stream からは重複を含めて 16 件まで読み、自分と重複を除いた最大 4 端末を hash 別の取得元として覚える（10 分。次の試行で先に使う）。端末の枠（既知と合わせて 1 要求 4 端末）が残っていれば、見つかった順にこの要求で試す。
- 既知だけで枠を使い切った要求では、残りの時間で見つけた候補を覚えるだけにし、既存の次の試行（5/30/120 秒）で先に使う。最後の試行の後に覚えた候補は、明示の再試行で使う。新しい再試行は作らない。
- 待機・既知の試行・検索・住所の解決・転送は、受付で決めた 30 秒を共有し、延長しない。表示の取得の取消（表示の終了）・期限・node の停止は stream を落とし、DHT と補助 index への要求も取り消す。通常の取得は ADR 0055 §1 のとおり待つ呼出しが消えても受付の期限まで続く（検索も同じ）。
- 見つけた端末への接続は、endpoint の住所解決（同じ DHT の `DhtAddressLookup`）を使う。DHT の候補を保持・権限の証明に使わず、取得した bytes は既存の BLAKE3/Bao の検証と保存前の確認を通る。

### 4. DHT の共有・寿命・設定（D2・D6）

- native は node の寿命に合わせて DHT を 1 つ組み立て、住所の公開・解決（`DhtAddressLookup::builder().dht(..)`）、補助 index の client、検索、告知で共有する。node の停止は、発見を外して task を止め、endpoint を閉じて（住所の公開・解決も止まる）DHT の socket を手放す。node を別に保持したままでも手放す。
- 補助 index の client は、`PublicBlobIndex` で見つけ方を受ける。本番は kukuri の鍵で署名した最大 2 台の一覧（`ListKey`。`kukuri_transport::KUKURI_PUBLIC_BLOB_INDEX`）で、試験は server を直接渡す（`Servers`）。一覧が引けなければ 30 秒ごとにやり直し、それまでは検索も告知もしない。一覧の鍵の発行と server の配置は、本番反映をまとめる別 Issue で行う。鍵が無い間は、設定がオンでも発見を使わない。
- 設定「公開コンテンツの発見」（`DiscoveryConfig::public_blob_discovery`）は既定でオン。項目の無い既存の profile もオンで始める。オンで補助 index の一覧があれば、Community Node の利用中・`static_peer` でも DHT を組み立てる。オフでは今までの discovery mode の挙動（ADR 0008）に戻る。環境変数で discovery を固定した起動では切り替えられない（seed と同じ）。
- 切替（`set_public_blob_discovery`）は設定を保存してから接続の再適用へ進み、DHT の使用か発見の有無が変わるときは stack を作り直す（古い node の停止で関連する要求・告知を止めてから、DHT・補助 index・endpoint を組み直す）。
- Web は DHT を使わない（ブラウザは UDP を使えない）。設定の切替も表示しない。

### 5. 依存の fork と上限（AC-1〜AC-3）

上流へは PR を出さず、KingYoSun 配下の fork を固定する（root と `apps/desktop/src-tauri` の `[patch.crates-io]`・依存）。

| fork（固定 rev） | 変更 | 上限・取消 |
| --- | --- | --- |
| `n0-mainline`（`7571f18f`、v0.7.1 起点） | 検索の受信先・保持する応答・応答待ちを有限にし、受信先ごとに取り消す | 受信先 64・受信先が保持する応答 16・進行する検索 256・応答待ち 1,024・期限切れを含めて持つ要求 2,048。受信先を落とすとその受信先だけを外し、最後の受信先が無くなった検索を止める。自分の id と put の対象の検索は止めない |
| `iroh-content-discovery`（`f6e2864a`、b4493ba 起点） | index 要求の呼出しごとの取消、保留の上限、1 回の告知 API（`Announcer`） | 進行表・保留 各 512（超えたら Busy）。取り消した公開は後から届く token で Put を送らない。resolver は照会の queue 64＋1 batch・同時照会 16 |
| `iroh-address-lookups`（`e2740b75`、0.6.0 起点） | 組み立て済みの DHT を住所の公開・解決へ渡す（`Builder::dht`） | 住所の公開の task が lookup を持つ循環参照を解き、lookup を落とすと自前の node は止まり、渡された node は持ち主が落とすまで動く |

告知の同時数を 2 に絞るのは、上限の計数に入らない put（1 件で近い node 群へ送る）が多数同時に応答待ちになると、2,048 件の表から期限内の要求が消えうるため（AC-1 の監査の記録）。

### 6. 法務

外部送信表示・プライバシーポリシー・データの流れの一覧を、Mainline への告知・検索と補助 index への登録・照会に合わせて改訂し、Legal bundle version を 9 へ上げて再同意を求める（2026-10-07）。

### 7. Community Node の保持端末の検索（D5・D6・D7）

- `POST /v1/blob-providers/search`（`kukuri_cn_protocol::BLOB_PROVIDER_SEARCH_PATH`）。要求は `{hash, budget_ms}`、応答は `{candidates: [{endpoint_id, relay_urls, direct_addrs}], partial}`。認証（bearer）と必須の同意を求め、未認証は 401 `AUTH_REQUIRED`、未同意は 403 `CONSENT_REQUIRED`、hash が 64 桁の 16 進でない・`budget_ms` が 0 は 400 `INVALID_BLOB_PROVIDER_SEARCH`、検索を提供しない node は 404 `BLOB_PROVIDER_SEARCH_NOT_CONFIGURED`。
- 検索は operator config の capability `public_blob_search`（`features.public_blob_search`、全 profile で既定は無効。D7）を有効にした node だけが提供する。有効にした node の生成文書は、プライバシーポリシーに取扱いデータ（依頼された公開 blob の hash と、見つけた保持端末の endpoint ID・到達情報。応答まで保存しない）を、外部送信表示に送信先（Mainline DHT・kukuri が運用する補助 index）を載せ、同意の revision が変わる（利用者はその node で再同意する）。manifest の `capabilities` にも出る。
- 提供には補助 index の一覧の鍵（`KUKURI_PUBLIC_BLOB_INDEX`。native と同じ鍵・同じ `PublicBlobIndex::connect`）も要り、鍵の無い build で capability を有効にした config では `cn-user-api` を起動しない（開示と提供を食い違わせない）。提供の有無は、bootstrap の応答でこの node 自身の `resolved_urls.public_blob_search` が示す（旧 node の応答には無く、false と読む）。node は DHT（client）を 1 つ組み立て、補助 index の client へ背景でつなぐ。つながるまでの検索は候補なしの `partial` を返す。
- 1 検索は `Resolver::resolve_stream` から重複を含めて 16 件まで読み、重複を除いて署名つきの住所 record（同じ DHT の `DhtAddressLookup`、公開はしない）を同時 4 件まで引き、relay URL を持つ候補だけを最大 4 件返す（直接の address だけの候補は返さない）。候補ごとの relay URL は 4 件、直接の address は 8 件まで。期限は `min(budget_ms, 10 秒)` で、過ぎたら集めた分で打ち切り `partial` を立てる。候補が空でも保持端末が無いことの証明ではない。
- 受付は、進行中の検索が全体 32 件・端末（bearer の endpoint ID、無ければ公開鍵）ごとに 8 件（client の取得の実行枠と同じ）まで。超えた要求は待たせずに 429 `BLOB_PROVIDER_SEARCH_BUSY`（`Retry-After: 5`。取得の次の試行と同じ）。全体 32 件は、1 検索の DHT の検索（保持端末 1・住所 record 最大 4）が DHT の進行する検索の上限（256）に収まる数。期限・応答・HTTP の切断で handler の処理が落ちると、stream・DHT の受信先・補助 index の要求を取り消し、受付を戻す。端末ごとの数の表は、全体の上限と同じ 32 件まで。
- node は iroh の endpoint を持たず、blob の取得・保存・size の確認をしない（読むのは DHT・補助 index・住所 record だけ）。受け取った hash を保存しない。一般の native へ探索を依頼せず、`cn-iroh-relay` の責務は変えない。候補は保持・権限の証明ではなく、client は既存の接続と hash の検証で取得する。
- 補助 index の server は `cn-addr-index`（`udp-addr-index` の `Server` を自分の Mainline DHT の socket に付ける別の process。blob・投稿は持たない）。relay の image に同梱し、compose の profile `addr-index` で起動する。port は DHT と共有の `60125/udp`。保持する record の数（既定 2,000,000）・送信元 IP ごとの record の数（16）・送信元 IP ごとの毎秒の照会数（50）を環境変数で変えられる（record の期限は 1 時間で固定）。record は送信元の address で引くので、送信元を書き換えて転送する edge 構成では提供しない。kukuri が運用する server だけで動かし（D6）、その最大 2 台を、fork の `iroh-index-list`（一覧の鍵を持ち、10 分ごとに更新し続ける）で署名つきの一覧として公開する。server の配置と鍵の発行は、本番反映をまとめる別 Issue で行う。

## Consequences

- 告知・予定の読取り・取込みの 1 回の処理量と、検索の 1 要求の読取り・試行・保持は、公開記録・保持・候補の総件数に比例しない（store の VM step・query plan と、20/200/2000 件の候補の試験）。
- 端末が公開 blob を保持していることは、その IP address・port とともに DHT の参加者から観測されうる。検索した infohash も観測されうる。設定でオフにできる。
- 補助 index の一覧の鍵が配布されるまで、本番では発見が働かない（住所の公開・解決の DHT の扱いも今までどおり）。Community Node は検索の capability を有効にできない（無効の node は 404 を返し、bootstrap で提供しないと示す）。
- 検索を提供する Community Node は、検索した hash から導いた infohash と自分の IP address を DHT の参加者へ、見つけた端末の IP address・port を補助 index へ送る。
- 復元した端末は、backup に含まれない保護されていない大きな cache の file の hash を、7 日の期限か回収まで告知しうる。取得側はその端末を欠損として 10 分間選ばない。

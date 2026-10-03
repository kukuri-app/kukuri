# ADR 0051: 公開投稿のリンクプレビュー取得境界

## Status

Accepted

## Context

Issue #1174では、投稿本文中の外部URLをlink化し、OGP metadataを投稿カードに表示する。WebViewから任意URLやremote imageを直接取得すると、CORS／CSPだけでなく、投稿を表示した端末からprivate network、loopback、metadata service等へ到達するSSRF、redirect／DNS rebinding、無制限response、cookie／Referer送信、private contentの閲覧漏えいが起こり得る。

現行の投稿本文は`SmartReferenceText`が内部route、topic、access token、mentionを扱い、外部browser起動はTauriの`open_external_url`が絶対HTTP(S)とReady状態をsink側で検証する。profile／mediaは外部画像URLをview contractへ持たず、WebView CSPも汎用remote imageを許可しない。この境界を維持したままpreviewを追加する必要がある。

## Decision

### 1. 対象surface

- 本文中の資格情報を含まない絶対`http://`／`https://` URLは、`PostCard`内でlinkとして表示する。内部referenceとmentionを優先し、invalid URLは通常textのまま残す。
- 自動OGP取得は、app-level legal consent後のReady状態で、公開投稿（`channel_id == null`）のprimary contentが表示可能・settledで、Timeline／Profile／Bookmarks、Thread、Community Indexのcardがviewport内にある場合だけ行う。
- previewはprimary contentの先頭URL 1件だけとする。private channel、DM、Composer参照preview、adult-content gate、trust collapse、withdrawn、missing、local pending／syncing／failed、document hidden、viewport外では取得しない。
- inline linkとpreview cardは、元URLを既存`open_external_url`からOS browserへ開く。`og:url`、redirect後URL、image URLをnavigation先にしない。

### 2. 取得境界

- WebViewはpage HTMLとremote imageを取得しない。Tauri command `fetch_link_preview`だけがpage HTMLと任意OGP imageを取得し、sanitized metadataとbounded raster `data:` imageを返す。
- WebView CSPの`img-src`へ汎用`http:`／`https:`を追加しない。raw HTML、script、style、iframe、SVG、event attributeを描画・実行しない。
- requestはcookie store、Authorization、Referer、system proxyを使用しない。固定の最小User-Agentと`Accept`だけを送る。URL以外のpost／topic／account情報をheaderやqueryへ追加しない。

### 3. URL、DNS、redirect

- absolute HTTP(S)、資格情報なし、空白／制御文字／backslashなし、4,096 bytes以下、scheme既定port（HTTP 80／HTTPS 443）だけを許可する。
- `localhost`、single-label、`.localhost`、`.local`、`.internal`、`.home.arpa`と、loopback、private、CGNAT、link-local、documentation、benchmark、multicast、unspecified、reserved addressを拒否する。
- DNS応答が空、またはpublic／non-publicの混在なら拒否する。検証済みaddressをHTTP clientの名前解決へ固定し、実接続先addressが検証集合に無い場合も拒否する。
- redirect auto-followは無効にし、最大3回をapplication loopで処理する。各`Location`を現在URL基準で解決し、URL／DNS／実接続先を再検証する。HTTPSからHTTPへのdowngradeは拒否する。

### 4. resource budgetとcache

| 項目 | 上限 |
| --- | --- |
| connect timeout | 3秒／request |
| request total timeout | 6秒／request |
| redirect | 3回 |
| HTML body | 512 KiB |
| image body | 1 MiB |
| title／site／description | 200／100／500文字 |
| 同時取得 | 4件 |
| distinct URLのin-flight待機 | 32件 |
| process cache | 128 entryかつ16 MiB |
| success／failure TTL | 10分／60秒 |

- cache keyはfragmentを除いたnormalized original URL。queryはrequestの一部なので保持する。
- 同一URLのin-flight requestは共有する。cacheとin-flight stateはprocess-memoryだけで、restart、account data、DB、localStorage、backup、diagnostic reportへ残さない。
- page取得失敗、非2xx、非HTML、oversize、metadata無しはtyped `unavailable`とする。image取得だけの失敗はtext previewを維持する。UIは無期限skeletonや空cardを表示せず、inline linkへfallbackする。

### 5. metadata

- `og:title`、`og:description`／`description`、`og:site_name`、`og:image`系をHTML tokenizerで読む。titleは`<title>`、siteはfinal page hostへfallbackできる。
- control／余分な空白を除き、上限で切る。最低限titleが無ければpreview unavailableとする。
- imageはfinal page URL基準で相対解決し、pageと同じ取得guardを通す。PNG、JPEG、GIF、WebPのdeclared MIMEとmagic bytesが一致する場合だけdata URLへ変換する。SVG、HTML、AVIFその他はv1では表示しない。

### 6. consentと開示

自動previewは、リンク先pageとOGP image hostに対する新しい外部送信である。legal bundle version 8で、送信先、表示を契機とすること、IP address、HTTP／TLS metadata、path／query、保持主体を開示する。cookie、Referer、account／topic／post情報、他本文を送らないこと、private channel／DMを自動取得しないことも明記する。version 7以下では再同意前にruntimeを開始せず、invoke gateによりpreview commandも拒否する。

### 7. 投稿者の record（Web の表示と中継。2026-10-03、#1220 W8 AC-2d・AC-2f）

Web クライアントは §2 の取得境界を持たず、ブラウザからリンク先・画像を取得しない（ADR 0060 §3）。代わりに、投稿者本人の native が取得した preview を投稿者の署名つき record にして投稿と同じ replica に置き、Web はそれを読む（2026-10-03 ユーザー判断）。

- 書き手と契機: 投稿者本人の native だけが書く。§1 の表示条件で `fetch_link_preview` が成功し、対象が自分の公開投稿で、その本文に preview の URL があるとき、背景で書く。外部への取得の契機・回数・送信内容は増やさず、取得の結果と表示も変えない（書込みの失敗は表示に影響しない）。投稿者でない native による書込みと、投稿の時点での取得は行わない。native の閲覧は record を表示に使わない（中継のために読む。下記）。
- 置き場所と形: 投稿の source replica（時間 bucket では投稿と同じ bucket。ADR 0054 §2）の `link-previews/<object id>/state`。値は kind `link_preview` の署名つき envelope で、`target_object_id`・`url`・`title`・`description`・`site_name`・任意の `image` を持つ。`url` は本文に書かれた先頭の URL の文字列のまま。
- 画像: §5 の画像（1 MiB 以下の PNG・JPEG・GIF・WebP）を content-addressed blob にし、record から hash・MIME・bytes 数で参照する。blob は添付と同じく本人が書いた blob として保持する。
- 1 投稿 1 回: 自分の docs author の record がその key に既にあれば書かない。取得し直しても書き換えず、取得のたびに blob を増やさない。
- 読む側（Web の表示と、Web・native の中継）: §1 と同じ表示条件で、投稿の行が持つ投稿者の docs author（ADR 0053 §2）と key の組で 1 件だけ読む。手元に無ければ、投稿者の検証済み宛先と topic の参加者から最大 4 件、全体 30 秒で読む（ADR 0054 §4 の対象参照と同じ）。key の列は走査せず、他の名義の record は何件あっても読まない。
- 検証: envelope の署名が投稿者のもの、`target_object_id` が表示中の投稿、`url` が表示側で本文から抽出した先頭の URL と一致し、文字と画像が §4 の上限内であること。画像は record の bytes 数までだけ取得し、bytes 数と先頭の bytes の形式が record と一致するときだけ data URL にする。
- 結果: 検証に通れば native と同じ card を出す。record が無い・検証に通らない・投稿者の docs author が分からないときは、inline link（URL だけ）を示す。画像だけを取れないときは文字の card を出す。
- 読取りの上限（2026-10-03、#1220 AC-2f）: record の読取りは §4 の取得と同じく同時 4 件・待機 32 件まで。待機の上限を超えた読取りは待たずに断り、画面は URL だけを示す。断った・失敗した読取りの結果は持たず、次の表示で読み直す。
- cache: 画面は読取りの結果を §4 と同じ上限（128 件かつ 16 MiB、成功 10 分・失敗 60 秒）で memory に持つ。
- 中継（2026-10-03 ユーザー判断、#1220 AC-2f）: record は投稿とは別の record のまま、参加者による中継（ADR 0054 §4 の #1395）の対象にする。record を読んで検証した端末（Web と native）は、record（投稿者の名義）と画像を手元の remote cache（ADR 0055。3 GiB・非利用 7 日）に保持し、既存の docs の読取りと blob の提供で他の参加者へ提供する。読む側は、保持した画像を先に使う。native も §1 の表示条件で `fetch_link_preview` を呼ぶとき（同じ契機）に投稿者の record を読んで保持するが、表示は自分の取得の結果のままにする。投稿の反映と一緒に record を読むことはしない。
- 開示: record は公開投稿と同じ範囲（その replica を取得する peer と、索引に参加する Community Node）へ複製される。legal bundle の版は上げず、再同意を求めない（2026-10-03 ユーザー判断）。データ分類（`docs/legal/link-preview-data-classification.md`）と突合表（`docs/legal/app-data-flow-inventory.md`）を更新する。cn-indexer は `link-previews/` を索引の契機にしない（共有 replica の key 種別表で無視）。

## Consequences

- 公開投稿の表示はリンク先から観測され得るため、previewの利便性と引き換えにlegal再同意が必要になる。
- private contentと非表示contentのURLは自動取得しない。利用者がinline linkを明示操作した場合だけ既存browser経路を使う。
- OGPはtransientな補助表示であり、取得不能でも投稿のcanonical contentとP2P同期は影響を受けない。
- site固有embed、永続cache、proxy service、複数previewが必要になった場合は、別Issueで送信先・保持・security boundaryを再決定する。
- §7 の record は投稿者の native が表示した後にだけ書かれる。投稿者が自分の投稿を表示していない、取得に失敗した、旧版の native で投稿した場合、Web は URL だけを示す。投稿者の native が居なくても、record を読んだ参加者が居れば、その参加者から読める（中継）。

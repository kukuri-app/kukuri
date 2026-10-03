# Feature Data Classification: 公開投稿のリンクプレビュー

ADR 0002（`docs/adr/0002-feature-data-classification-template.md`）とADR 0051に基づく分類。

### Feature Data Classification

- Feature 名: 公開投稿の外部URL link化とOGP preview
- Durable / Transient: URLは既存投稿本文の一部としてDurable。抽出segment、取得中状態、cacheはprocess内Transient。投稿者本人のnativeが自分の公開投稿について取得したOGP metadata（URL・title・description・site）とraster imageは、投稿者の署名つきrecordとimage blobとしてDurable（ADR 0051 §7、#1220 AC-2d）
- Canonical Source: URL文字列は既存`PostView`本文。previewは取得時点のリンク先HTTP responseのOGP／HTML metadata。Webが表示するpreviewは投稿のreplicaの`link-previews/<object id>/state`（投稿者の署名つきenvelope）と、それがhashで参照するimage blob
- Replicated?: 投稿者本人の公開投稿のrecordとimage blobだけを、その投稿と同じ範囲（replicaを取得するP2P peer、索引に参加するCommunity Node）へ複製する。その投稿を表示した参加者（native・Web）は、読んで検証したrecordとimage blobを手元のremote cache（3 GiB・非利用7日）に保持し、他の参加者へ提供する（中継。#1220 AC-2f）。他人の投稿のpreviewを投稿者以外が書くこと、private channel／DMのpreview、HTML response本体は配布しない
- Rebuildable From: 投稿本文とリンク先responseから再取得可能。recordは投稿ごとに1回だけ書き、取得し直しても書き換えない。recordが無い・検証に通らない場合はinline linkだけで動作する
- Public Replica / Private Replica / Local Only: 投稿本文の既存分類を維持。nativeの取得結果のcacheとrecordの読取り結果のcache（128件かつ16 MiB）はLocal Only transient。投稿者のrecordとimage blobはPublic Replica（公開投稿と同じreplica）で、中継のために保持した写しはremote cacheの上限と回収に従う
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 投稿者のrecordの画像だけ（1 MiB以下のPNG・JPEG・GIF・WebP）
- SQLite projection 必要有無: 不要
- 必須 contract: `fetch_link_preview`のtyped outcome、frontend external URL segment、public／visible／Ready gate、SSRF／redirect／size／MIME guard、既存`open_external_url`。recordの書込みは自分の公開投稿で本文にURLがあるときだけ、読取りは投稿者のdocs authorとkeyの組で1件、同時4件・待機32件まで、投稿者の署名・URLの一致・上限の検証（`read_link_preview_record`）。検証に通ったrecordと画像だけを中継のために保持する
- 必須 scenario: harness scenarioは追加しない。Tauri unit／IPC gate、frontend component／browser、Windows Tauri実機で許可URL、禁止URL、redirect、timeout、oversize、cache、表示gate、外部browser起動を確認する

## 外部送信と保持

- 送信先: 公開投稿の先頭URLのlink先hostと、そのpageがOGP imageとして指定したpublic host
- 契機: app-level legal consent後、対象の公開・表示可能・settledな投稿cardがviewport内で表示されたとき
- 送信・観測され得る項目: IP address、HTTP／TLS request metadata、URL path／query、固定User-Agent、preview閲覧の発生
- 送信しない情報: cookie、Authorization、Referer、公開鍵、account／topic／channel／post ID、他の投稿本文、private channel／DMのURL
- 端末内保持: sanitized metadataとbounded data imageをprocess-memory cacheへsuccess 10分／failure 60秒。128 entryかつ16 MiB上限。process終了で消える
- 第三者保持: link先、image host、通信経路事業者の方針に従う。kukuriから一括削除できない

## 投稿者のrecord（Web の表示と中継。ADR 0051 §7）

- 書き手: 投稿者本人のnativeだけ。上の契機で取得に成功したとき、背景で書く。外部への取得の契機・送信先・送信内容は増えない
- 複製される項目: URL（本文に書かれた文字列）、title、description、site、画像のblob（hash・MIME・bytes数）、投稿者の公開鍵と署名。公開投稿本文と同じ範囲へ複製され、受信したpeerやCommunity Nodeにcopyが残り得る
- 閲覧者（Web）: リンク先・画像hostへは送信しない。投稿者のrecordを、投稿者の端末やtopicの参加者からP2Pで読む
- 中継（#1220 AC-2f）: 公開投稿を表示した参加者（native・Web）は、投稿者のrecordを読んで検証し、recordと画像を手元のremote cache（3 GiB・非利用7日）に保持して他の参加者へ提供する。nativeの表示は従来どおり自分の取得の結果で、recordは表示に使わない
- 説明: legal bundleの版は上げず、再同意を求めない（2026-10-03 ユーザー判断）

## Security boundary

URL／DNS／実接続先と全redirectをTauri側で検証し、public-routable addressへconnectionを固定する。WebViewへremote image URLやraw HTMLを返さず、PNG／JPEG／GIF／WebPだけをbounded data URLとして返す。詳細と上限はADR 0051を正本とする。

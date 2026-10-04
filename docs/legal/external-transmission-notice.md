# kukuri 外部送信表示

最終更新日: 2026-10-04

施行日: 2026-09-19

Legal bundle version: 8
正文言語: 日本語

本表示は kukuri デスクトップアプリと、インストール不要の Web クライアントの外部送信を説明するものです。各 Community Node が行う外部送信は、その Node の manifest から開ける外部送信表示・プライバシーポリシーを確認してください。

## 1. 管理主体・問い合わせ先

- 管理主体: KingYoSun
- 問い合わせ先: ops@kukuri.app
- 運営主体の氏名・住所は、上記窓口へ請求いただいた場合に遅滞なく回答します。

## 2. 現在行われる外部送信

| 送信先 | 送信契機 | 目的 | 送信・観測され得る項目 | 保持の考え方 |
|---|---|---|---|---|
| Web クライアントの配信元 | Web クライアントを開くとき・読み込み直すとき（HTML・JS・WASM 等の取得） | Web クライアントの配信 | IP address、HTTP／TLS request metadata、User-Agent、要求した path、Referer（リンク元の page が送る場合）。アカウントの鍵、投稿、URL の `#` 以降（移行の招待を含みます）は送りません | 配信元の運営者と通信経路事業者の方針に従います |
| 公開投稿の先頭 URL のリンク先と OGP 画像配信先 | アプリ同意後、公開・表示可能・送信確定済みの投稿 card が画面内に入り、先頭 URL の preview を初めて必要とするとき。同一 URL は一時 cache 中に再送しません。Web クライアントは送信しません（投稿者が書いた preview の記録を P2P で読みます） | 投稿内リンクの site、title、説明、任意画像の preview 表示 | IP address、HTTP／TLS request metadata、URL path／query、固定 User-Agent、preview 閲覧の発生。cookie、Authorization、Referer、公開鍵、account／topic／channel／post ID、他の投稿本文は送りません。private channel／DM、折りたたみ中の内容は自動取得しません | kukuri は sanitized metadata／画像を process memory に success 最大10分、failure 最大60秒、合計128件かつ16 MiBまで保持し、再起動後へ残しません。相手方と経路事業者の保持は各主体の方針に従います |
| GitHub Releases | Direct／NSIS・Linux版だけで、アプリ同意後の起動時、30分ごとの自動更新確認、手動確認、更新download。Microsoft Store版はkukuri内から送信しません。Web クライアントも送信しません | Direct配布の署名済みPreview updateの確認・取得 | IP address、HTTP／TLS通信に必要なrequest metadata、更新確認に必要なapp／platform情報 | GitHubと通信経路事業者の方針に従います。kukuriはDirect版の結果とerrorをruntime stateと診断表示に使用します。Store版の更新はMicrosoft Store／Windowsへ委譲します |
| Mainline DHT | `seeded_dht` を有効にして接続先を探索するとき。Web クライアントは使いません（ブラウザは UDP を使えないため） | P2P endpoint の発見 | endpoint ID、署名済み address record、通信元 IP address 等 | DHT 参加者に分散して扱われるため、kukuri が一括した保持・削除を制御しません |
| P2P 接続相手 | topic、profile、public post、private channel、DM、live／game／Dome 等へ参加するとき | 選択した audience 内の同期・表示・添付取得・real-time 通信 | 公開鍵、endpoint／IP address、対象範囲の署名済み metadata・本文・reaction・添付。private channel／DM は対応する capability／暗号鍵の範囲。Web クライアントとの WebRTC の接続の交渉では、接続の候補（端末の IP address とポート）も届きます | 受信端末が copy を保持し得ます。送信後の完全な遠隔回収・一括削除は保証できません |
| 構成された iroh relay | Direct P2P の接続補助、または Direct P2P と Relay Supported P2P が成立しない場合 | hole punching／endpoint assist、必要時の Relay Fallback | IP address、endpoint ID、接続 metadata。Relay Fallback では暗号化された実データ traffic。Web クライアントは WebSocket で接続します | relay 運営者の方針に従います。relay URL があるだけでは実データが relay を通ったことを意味しません |
| 構成された iroh relay と同じ host の STUN（3478 番） | Web クライアントとの WebRTC の接続を交渉するとき（Web クライアントと、Web クライアントと接続するデスクトップアプリ）。relay を使わないとき（relay を無効にした設定、Community Node に同意していないとき等）は送りません | 接続の候補にする、外から見える IP address とポートを知る | 認証の無い binding の要求と、通信元の IP address とポート | relay と同じ Community Node の基盤の運営者の方針に従います |
| 利用者が保存・同意した Community Node | manifest／policy 取得、認証、consent、bootstrap／rendezvous、検索・発見、明示した indexing と索引状況の確認、成人向け表現の推定の照会、信頼評価の照会、ブロック・ミュートの提供（同意した Node だけ）、通報、tester feedback 等 | 選択した Node capability の提供 | 公開鍵、proof、endpoint 情報、topic rendezvous key、検索 query、対象識別子（索引状況の確認では自分の申請対象と確認したい topic／channel。非公開チャンネルの索引対象判定は明示確認後の所属証明を伴います）、成人向け表現の推定の照会では表示した投稿の ID と添付ファイルの識別子（推定の採用を選んだ Node だけ。本文・閲覧履歴・social graph は含みません）、ブロック・ミュートの提供では、相手の公開鍵・操作の種別と状態・操作時刻・あなたの署名（その Node の任意同意文書に同意した場合だけ。投稿本文・閲覧履歴は含みません）、信頼評価の照会では表示した投稿と引用元の作成者の公開鍵、および表示した live / game 一覧の主催者の公開鍵（採用順位に選んだ Node だけ。本文・閲覧履歴・ブロック / ミュート一覧は含みません）、通報／feedback の入力項目等。機能ごとに異なります | 当該 Node の privacy／外部送信／retention 文書に従います。Node 運営者と kukuri 運営者が同一とは限りません |

現行配布物は `https://api.kukuri.app` を Community Node の初期候補として含みます。これは固定接続先ではなく、利用者は削除または他の Node へ置換できます。実際の relay と Node の開示 URL は、その時点の配布設定、利用者設定、取得した Node manifest から確認できます。

## 3. 診断レポート

診断レポートは、利用者が設定画面でコピーまたは書き出すまで端末外へ自動送信されません。レポートには、アプリ版、OS／WebView（Web クライアントではブラウザ）の platform／user agent、接続状態、delivery／discovery／active path、peer／topic 件数、未読通知件数、更新状態、OS notification 状態、Node URL／session phase／retry／error が含まれ得ます。

既定のレポートには、秘密鍵、認証 token、private channel capability secret、invite／share token、DM 本文、local database path を含めません。利用者がレポートを GitHub その他へ添付した後は、選択した送信先の取扱いに従います。

## 4. 送信しない情報と現行の分析方針

- アプリ同意記録と18歳以上の自己申告記録は端末内にだけ保存し、network へ送信しません。
- 生年月日、公的身分証、公式な年齢確認情報は収集しません。
- Web クライアントは、第三者の script・analytics を読み込まず、cookie を使いません。
- 現行の kukuri アプリは、既定で行動分析、広告 tracking、自動 crash report 送信を行いません。将来導入する場合は、送信開始前に本文書とプライバシーポリシーを改訂し、必要な同意を求めます。

## 5. 変更履歴

- version 8 補記（2026-10-04）: インストール不要の Web クライアントの外部送信として、配信元からの取得、STUN、relay の WebSocket、WebRTC の接続の候補を追記しました（#1220）。デスクトップアプリに加わる送信は、Web クライアントとの接続の交渉での STUN だけです。送信先は relay と同じ host の運営者、目的は接続の補助、項目は IP address とポートで、relay の行の範囲内のため、版・施行日は変更していません。
- version 8（2026-09-19）: 公開投稿の先頭 URL の preview を表示するため、リンク先と OGP 画像配信先へ自動で送信され得る項目、非対象内容、process-memory cacheの上限を追加しました（#1174）。#1190で、GitHub Releasesへの更新確認はDirect／NSIS・Linux版だけで、Microsoft Store版はStore／Windowsへ委譲してkukuri内から送信しない配布差を明記しました。この補記は外部送信を増やさないためbundle versionを変更しません。
- version 7（2026-09-18）: Community Node の送信契機に「信頼評価の照会」を追加しました。採用順位に選んだ Node へ、表示した投稿と引用元の作成者の公開鍵、および表示した live / game 一覧の主催者の公開鍵を送ります。本文・閲覧履歴・ブロック / ミュートの一覧は送りません（#1061）。
- version 6 補記（2026-09-18）: Community Node の送信契機に「ブロック・ミュートの提供」を追記しました（#1061）。送信は、その Node が公開する任意同意文書へ利用者が個別に同意した場合だけ行い、同意を取り消すと保存済みの記録の削除を要求します。同意済みの Node capability の範囲内で、送信先・目的・保持主体は変わらないため版・施行日は変更していません。
- version 6（2026-09-16）: Community Node の送信契機に「成人向け表現の推定の照会」を追加しました。推定の採用を選んだ Node へ、タイムライン・スレッド・通知等に表示した投稿の ID と添付ファイルの識別子を送ります（#1056）。
- version 5 補記（2026-09-11）: Community Node の送信契機に「索引状況の確認」（自分の索引申請の状態と、対象が索引対象かの確認。#975）を追記しました。送信先・目的・保持主体は変わらず、同意済みの Node capability の範囲内のため版・施行日は変更していません。
- version 5（2026-09-03）: 利用規約の全面改訂に合わせ、legal bundle の版・施行日と責任主体の用語を同期しました。送信先、目的、送信項目、保持主体の実質的な変更はありません。
- version 4（2026-09-02）: 実装上の更新確認、DHT／P2P／relay、Community Node、診断レポートを送信先・目的・項目・保持主体ごとに整理し、管理主体、Node 別責任、削除限界を追加しました。

# ADR 0018: channel-first sidebar と unified epoch lifecycle

## Status
Accepted

## Date
2026-03-29

## Base Branch
`main`

## Related
- `docs/adr/0012-topic-first_progressive_community_filtering_draft.md`
- `docs/adr/0013-social-graph-foundation-draft.md`
- `docs/adr/0014-uiux-dev-flow.md`
- `DESIGN.md`
- `docs/adr/0020-pairwise-dm-v1.md`

## Note
- pairwise DM は channel / epoch lifecycle の対象外であり、global pairwise surface として `0020` の contract を優先する。
- desktop shell の配置と state ownership は [`0031`](0031-variable-span-column-workspace.md) が本 ADR を部分的に置き換える。channel が topic 配下の scope であること、Join / Share、epoch lifecycle、auto distribution / auto apply、private channel の domain / transport contract は本 ADR を維持する。一方、常設 left rail、全画面で単一の active topic / channel、単一 primary workspace switch、Thread / Author 専用 detail pane stack は `0031` を優先する。

## Context
- 現行 desktop shell では `channel` が `topic` の子である一方、UI 上は `Timeline / Channels / Live / Game / Profile` の並列 workspace として扱われていた。
- 実際には channel の切り替えは workspace ではなく scope の切り替えであり、`Timeline / Live / Game` の表示範囲と create target を変えるだけである。
- この不一致により、user は `topic -> channel -> workspace` の関係を短期記憶で補完する必要があり、`View Scope` / `Compose Target` / `Channels` tab / `Join Invite` / `Join Grant` / `Join Share` / `Create Invite` / `Create Grant` / `Create Share` / `Freeze` / `Rotate` などの冗長 control が増えていた。
- `0012` は 2026-03-20 時点の current main を固定する ADR として有効だが、そこにある `invite_only private channel data` の「hard-private baseline として継続利用」、`join grant / epoch control` の audience 別 split、desktop の `Channels` workspace と manual `Freeze` / `Rotate` は今回の UX 方向と衝突する。
- 参加者体験の観点では、新 epoch の share だけでなく invite も既存 participant には自動配布・自動適用される必要がある。owner が rotate や re-share のたびに participant へ manual join を要求する flow は許容しない。

## Decision

### 1. channel は topic 配下の選択状態として扱う
- `channel` は独立 workspace ではなく `topic` の child selection とする。
- primary route は `#/timeline`, `#/live`, `#/game`, `#/profile` に固定する。
- `#/channels` は廃止し、アクセス時は `#/timeline` へ normalize する。
- route/search param の正本は `topic` と optional `channel` とし、`timelineScope` と `composeTarget` は public route contract から外す。

### 2. channel switch は左サイドバーへ移す
- left rail を `status/settings -> ADD TOPIC -> CHANNEL -> Tracked Topics` に再編する。
- `Tracked Topics` は single-expand accordion とし、active topic 行の子要素としてその topic の joined channel 一覧を出す。
- topic 行の選択は public scope、channel 子要素の選択は private scope を表す。
- channel list の primary UI は `label + audience badge + selected state` に限定し、`epoch` / `sharing_state` / `joined_via` は debug か diagnostics に退避する。

### 3. Join / Share を unified action にする
- UI 上の import action は 1 つの `Join` に統一する。token kind の判定は runtime が内部で行い、invite / grant / share へ振り分ける。
- UI 上の export action は 1 つの `Share` に統一する。audience kind に応じた invite / grant / share の選択は runtime が内部で行う。
- `Join Invite`, `Join Grant`, `Join Share`, `Create Invite`, `Create Grant`, `Create Share` の個別 button は primary UI に置かない。
- `Freeze`, `Rotate`, `Close Sharing` の standalone button は primary UI に置かない。

### 4. 全 audience を epoch-aware lifecycle に揃える
- `invite_only`, `friend_only`, `friend_plus` のすべてを epoch-aware private channel として扱う。
- signed object の正本は `channel-policy`, `channel-participant`, `channel access token`, `epoch handoff grant` に揃える。
- `PrivateChannelRotationGrant*` は audience 非依存の epoch handoff grant 契約へ一般化する。
- owner の write/share 前には必要に応じて auto cutover を実行する。
  - `invite_only`: write/share 前に current epoch を seal し、new epoch へ rotate する。
  - `friend_only`: stale participant を評価し、必要時のみ rotate する。eligible mutual participant にだけ handoff する。
  - `friend_plus`: write/share 前に current epoch を seal し、new epoch へ rotate する。

### 5. existing participant への auto distribution / auto apply を正本にする
- owner が rotate または share したとき、current participant には新 epoch handoff grant を自動配布する。
- この自動 handoff は audience 別に次の意味を持つ。
  - `invite_only`: current participant 全員に新 epoch access を自動配布する。manual re-invite は不要。
  - `friend_only`: current participant のうち current policy を満たす相手にだけ新 epoch access を自動配布する。
  - `friend_plus`: current participant 全員に新 epoch access を自動配布する。
- participant 側は background refresh, scoped read, joined channel list refresh, restart restore のいずれからでも同じ redemption path で新 epoch を自動適用する。
- 既存 participant に対する invite 再配布も同じ auto distribution / auto apply contract に含める。user-facing には `Join` 再入力を要求しない。

### 6. legacy invite_only は互換移行しない
- 既存の legacy `invite_only` channel は epoch-aware へ in-place migration しない。
- upgrade 後に legacy `invite_only` participant が継続参加するには fresh invite が必要とする。
- 新規作成される `invite_only` channel は常に epoch-aware 初期 epoch を持つ。

### 7. 非公開チャンネルのランデブー鍵を現在の世代秘密へ結び付ける
- 非公開チャンネルの通知用話題は、通信層が実際に購読する `hint/private/<channel_id>` を派生入力にする。`hint/` を付ける前の `private/<channel_id>` は使わない。
- `hint/private/<channel_id>` のランデブー鍵は、現在の世代の `current_epoch_secret_hex` を `private_topic_rendezvous_key_hex_secret` に渡して作る。
- 現在の世代秘密が見つからない非公開通知用話題は送信対象から除外し、`public_topic_rendezvous_key` へ後退させない。
- 世代切替後は旧鍵の更新を止め、新鍵だけを送る。旧鍵はコミュニティノードの有効期限により最大45秒で失効する。
- 旧方式の公開情報由来の鍵と新方式の鍵は併送しない。併送すると失効済み参加者が旧鍵で在席情報を追跡できるためである。
- 新旧クライアントが混在する間は、同じ非公開チャンネルの参加者でもコミュニティノード経由の発見が成立しない場合がある。参加者のクライアント更新を移行条件とし、サーバー側の別名解決は設けない。

### 8. 鍵更新の担当端末（#1219 W6）
同じ owner の account を複数の端末で使っても世代を分岐させないため、新しい世代を作る端末を channel ごとに 1 つにする。

- 端末 ID は、その端末の iroh endpoint ID とする。endpoint の秘密は端末ごとに作り、backup（ADR 0048）・account 同期（ADR 0061）で移さない。同じ account で共通の docs author（ADR 0053）は使わない。
- 担当の記録（`PrivateChannelController`）は次の欄を持つ。

| 欄 | 意味 |
| --- | --- |
| `device_id` | 担当端末の endpoint ID |
| `generation` | 担当の世代。作成で 1、担当が移るごとに 1 増やす |
| `transfer_to` | 引継ぎ中の移譲先。あれば旧担当は停止済みで、移譲先が次の世代で有効になるまで、どの端末も新しい世代を作らない（遷移は #1219 AC-4） |

- channel を作成した端末が generation 1 の担当になる。記録は参加状態の capability と一緒に端末へ保存する（参加の行。ADR 0061 §9）。
- 新しい世代（epoch の ID と secret の生成、旧世代の凍結、handoff grant の配布）は `rotate_private_channel` の準備段階だけで作る。owner の account で、記録の `device_id` が自端末かつ `transfer_to` が無い端末だけが進む。同じ account の別端末、記録が無い、引継ぎ中のときは、何も書かずに `PrivateChannelControllerPending`（code `PRIVATE_CHANNEL_CONTROLLER_PENDING`）を返す。W8 #1220 はこの code で「チャンネルオーナーが参加処理を行うまで保留になる」旨のダイアログを出す。
- この判定を通る入口: 明示の rotate、共有前の auto rotate（`invite_only`・`friend_plus`）、参加者の変更（`friend_only` で資格を失った参加者がいる）による write・共有前の auto rotate、それらの再試行。鍵更新を伴わない閲覧・投稿・`friend_only` の grant の共有は判定を通らず、どの端末でも行える。
- 担当が不明な channel（記録の無いまま受け取った自分の channel）は、担当の記録が届くまで鍵更新を伴う操作を保留する。担当を移譲せずに失った channel は、復旧・強制移譲をせずに作り直す（#1219 S5）。
- 移行: 担当の欄が無い本変更前の保存の自分の channel は、読み込んだ端末を generation 1 の担当にする（それまでの単一端末の利用を保つ）。本変更前に同じ channel を複数の端末へ写していた場合の重複は扱わない。

鍵更新の操作（#1219 AC-2）:

- 担当の判定を通ったら、まず新しい世代の epoch id と secret を作り、その世代の鍵の行（ADR 0061 §9）を「この端末の鍵更新が終わっていない」印（元の世代、配布の cursor）付きで 1 行書く。この世代の ID が操作 ID で、docs へはまだ何も書かない。
- 段: 旧世代の凍結 → 新しい世代の metadata・Open policy・owner の参加 → 確定（参加の行の現在の世代を進め、世代の鍵の item を account 同期へ書く）→ handoff grant の配布。確定を配布より前に行い、確定と account 同期への記録が済むまで印の cursor を空（`NULL`）に保つ。
- 再開: 確定の前に失敗・再起動したら、同じ入口の再送（担当の判定を通る）が、現在の世代から予約した確定前の世代を見つけて同じ世代を再開する。新しい世代を別に作らない。各段は同じ操作でやり直しても同じ世代を書く。確定の後に残った段（account 同期への記録と配布の続き）は、再送を待たずに背景で進める。
- 配布: 宛先は参加者の表（ADR 0055）を公開鍵の順に 128 件の page で読み、cursor を印に置く。page は次の公開鍵を索引で 1 件ずつ引く（参加者が redeem のたびに残す過去の世代の行の数に比例しない）。受付（rotate の呼び出し）は確定と最初の 1 page まで行い、残りは DM outbox の再送 owner の tick ごとに 1 操作 1 page ずつ進める。page の途中で失敗したら、その page を初めからやり直す（同じ相手への grant は docs の同じ key を上書きし、outbox の重複は ACK で消える）。
- 終わり: 配布の終端で印を消す（完了の記録を残さない）。退会した channel の操作も印を消す（鍵の行は退会で消える）。
- friend_only の資格: channel の owner と mutual でない参加者を資格喪失とする（関係が完全に無くなった参加者を含む。友達の友達の経路は使わない。2026-10-02 ユーザー判断）。資格喪失の印は参加者の行に持ち、参加者の行の書込みと follow の edge の書込みで更新する（edge の書込みでは、両端の公開鍵が参加する channel を索引で 1 件ずつ引き、現在の世代の行だけを直す）。現在の世代の参加者数と資格喪失の数は、(channel, epoch) ごとに保つ 1 行を読む。投稿・共有の前の auto rotate の判定と view は、参加者の数に比例して数えない。
- Web の背景の配布は、DM outbox の再送 owner を Web で起動したときに動く（現在は未起動。受付の 1 page は動く）。
- 段の判断（確定前か、確定の後に残った段か）は、参加の行の現在の世代で行う。メモリの参加状態は確定の後に lease の task が読み直すまで古く、確定前と取り違えると、同じ端末の同時の鍵更新の 2 つ目が予約した世代を確定せずに印だけ消す（#1219 AC-3 で修正）。

担当でない端末からの依頼（#1219 AC-3）:

- 共有・書き込みの前の auto rotate で保留になった端末（担当でない・担当が不明・引継ぎ中）は、依頼の item `channel/<channel id の hex>/rotation`（値は `{"from_epoch_id":"<その端末の現在の世代>"}`）を account 同期へ書く。書き込みは write の判定を通るすべての操作（投稿・リアクション・取り下げ・live/game・Dome の操作）で、投稿と同じに扱う（2026-10-03 ユーザー判断）。明示の rotate（画面には無い）と、表示のための「書けるか」の判定（コミュニティの索引の検索結果。`PrivateChannelOwnerAction::WriteCheck`）は、依頼を書かず待たずに保留を返す（同日のユーザー判断。担当の端末では、この判定も変更前から鍵を更新する）。
- 担当の端末は、取得した依頼を merge するとき、その channel の参加の行が参加中で、現在の世代が `from_epoch_id` と一致するときだけ、上の判定（`prepare_private_channel_rotation`）を通して鍵を更新する。担当でなければ何もしない。依頼の版は `(updated_at, op_id)` で採り、採った版の再受信でもやり直す（鍵更新が途中で失敗したら、差分の取得の再送で再開する）。既に進んだ世代からの依頼・同じ依頼の再受信では、新しい世代を作らない。依頼の完了の記録は持たない（依頼は channel ごとに 1 件で、元の世代で古さが分かる）。
- 依頼した端末は、担当の `device_id` が本人の端末の候補（account の hint topic の rendezvous の候補、ADR 0061 §10）にいて `transfer_to` が無ければ、現在の世代が進むのを 15 秒まで待つ。進めば共有（token の発行）・投稿をその世代で続ける。候補にいない・期限までに進まなければ `PrivateChannelControllerPending` を返す（2026-10-03 ユーザー判断）。どちらの場合も依頼は残り、担当は online になって取得したときに処理する。
- 新しい世代は、担当が書く世代の鍵の item（確定の段）で依頼した端末へ届く。依頼した端末は鍵の行を保存してから現在の世代を進める（ADR 0061 §9）。
- 担当の記録の item は、担当になった端末が書く（作成。旧保存の移行の分は時刻 0 の行にして、送り直しで書く）。担当の item は key の順で参加の item より先に読まれる（周回・作り直し）ので、受け手は参加の行が無いときも、手元の replica の記録より上の規則で新しければ採って自分の replica に置き、参加の行を作るときに手元の replica から 1 件読んで採る。同じ版は採らない（行の無い端末どうしの往復を止める。ADR 0061 §10）。

担当の移譲と復元（#1219 AC-4）:

- 移譲は、担当にしたい owner の端末 B の操作（channel の設定の「この端末で行う」。`take_private_channel_controller`）で始める。B は、旧担当 A の `device_id` が本人の端末の候補（AC-3 と同じ）にいて `transfer_to` が無いときだけ、依頼の item `channel/<channel id の hex>/controller-request`（値は `{"to_device_id":"<B>","generation":<A の記録の世代>}`）を account 同期へ書き、B が担当になるのを 15 秒まで待つ（結果は `taken`、期限切れは `waiting` で依頼は残る）。候補にいなければ何も書かずに `not_connected` を返す。旧担当の端末での確認・端末の一覧・旧担当が接続していないときの奪取は無い（2026-10-03 ユーザー判断）。
- A は依頼を merge するとき、参加の行の記録が自分で `transfer_to` が無く、世代が依頼の `generation` と同じで、`to_device_id` が自分でなければ、停止の記録 `{A, g, transfer_to: B}` を参加の行と account 同期へ書く。既に同じ停止を参加の行に書いていれば（行の後・account 同期の台帳の前で止まった後の再受信）、同じ記録を書き直す。それ以外（既に移った世代からの依頼・別の移譲先への引継ぎ中）は何もしない。停止の後、A は上の判定で新しい世代を作らない。
- B は `transfer_to` が自分の記録を merge するとき、それを取り込む代わりに、有効化の記録 `{B, g + 1, null}` を同じ排他の中で参加の行へ書き、その後に account 同期へ書く（取得した停止の記録は自分の replica へ置かない）。途中で止まれば、停止の記録の再受信で有効化をやり直す（既に有効化を参加の行に書いていれば、同じ記録を書き直す）。停止から有効化が届くまでは、どの端末も保留する。
- 引継ぎの途中で B が戻らなければ、その channel は保留のままになり、作り直す（S5）。始めた引継ぎの取消しは無い。
- 鍵更新の判定（`prepare_private_channel_rotation`）は、予約の前に参加の行の記録を排他の中で読んで行う（メモリの参加状態は移譲の直後に古い）。
- backup（ADR 0048）から復元した端末は endpoint の秘密を持ち越さないので、別の端末 ID になる。復元の準備が account の db の隣に印（`<db>.controller-claim`）を置き、その account の最初の起動の背景 task が、自分が owner の参加中の channel を 64 件ずつ読み、記録を `{この端末, g + 1, null}`（記録が無ければ世代 1）にして参加の行と account 同期へ書き、終えたら印を消す（2026-10-03 ユーザー判断）。途中で止まれば次の起動でやり直し、既にこの端末が担当の channel は世代を進めずに同じ記録を書き直す。旧担当は account 同期で新しい世代の記録を採って保留になる。
- 遷移（停止・有効化・復元の引取り）は、参加の行 → account 同期の台帳 → 自分の replica の順に書く。台帳の後に止まれば台帳の送り直しで届き（ADR 0061 §10）、行の後・台帳の前に止まれば、上のやり直し（依頼・停止の記録の再受信、次の起動の引取り）が同じ記録を書き直して届ける。差分の取得は item の merge を終えてから cursor を進めるので、merge の途中で止まった item は次の取得で再び merge される（#1219 AC-4 監査 B-1）。
- 古い backup の復元（backup の後に担当を別の端末へ移していた）では、同じ世代で端末の異なる記録が 2 つになりうる。下の規則で `device_id` の大きい方に決まる（復元した端末が担当にならないこともある）。より新しい世代の記録が届けば、それに従う。
- 画面: owner の端末の channel の設定は、別の端末が担当なら「共有リンクの作成と、参加者が変わったときの新しいアクセスの配布は、あなたの別の端末で行っています。」と「この端末で行う」（処理中・接続していない・応答待ちを示す）を、引継ぎ中なら保留と、移す先の端末が使えないときの作り直しの案内を出す。この端末が担当・担当が不明・owner でないときは出さない。「担当」という語は画面に出さない（開発中の独自の語で、利用者には意味が分からない。2026-10-03 ユーザー判断）。

account 同期（ADR 0061 §2）の記録の契約:

- `channel/<channel id の hex>/controller` の値は上の記録の JSON（`{"device_id":"…","generation":1,"transfer_to":null}`）。書くのは担当端末と、#1219 AC-4 の引継ぎの遷移（停止・有効化）と復元の引取りだけ。
- 採否は記録の中身で決め、`updated_at`・`op_id` では決めない（#1218 INVAR-3）。`generation` の大きい方を採る。同じ `generation` で `device_id` が同じなら、`transfer_to` の無い記録より有る記録を採る（旧担当の停止は戻らない。両方の `transfer_to` が異なる値なら手元を保つ）。同じ `generation` で `device_id` が異なれば、`device_id` の大きい方を採る（どの順で受けても、どの端末でも同じ記録に決まる。#1219 AC-4 で、受けた記録を採らずに手元を保つ旧規則から変えた）。
- `channel/<channel id の hex>/epoch/<epoch id の hex>` の値は `PrivateChannelEpochCapability`（`{"epoch_id":"…","namespace_secret_hex":"…"}`）。受けた鍵は追加で保持する。現在の世代は、新しい世代の replica にある owner 署名の policy の `previous_epoch_id` が手元の現在の世代と一致するときだけ進める（参加者の handoff の redeem と同じ検証）。
- 例外: 本人の端末から届いた鍵の item は、その端末が上の検証（または owner の鍵更新の確定・参加）を通して現在の世代にした世代である。そのため参加中の channel では、`previous_epoch_id` を辿らずに、開始時刻が最も新しい鍵の行の世代を現在の世代にする（ADR 0061 §9。新しい端末・作り直した端末が世代の数だけ辿らない）。このため新しい世代の epoch id の時刻は、今の時刻と、現在の世代の開始時刻 + 1 ミリ秒の大きい方にする（開始時刻の順を鎖の順と一致させる）。

## Implementation Contract

### Desktop shell
- left rail に `CHANNEL` section を持たせ、channel 作成、token 入力、`Join`、selected channel 向け `Share` を置く。
- `Timeline` / `Live` / `Game` は同じ topic/channel selection を正本にして list/create を scope する。
- `Profile` は active topic の public self timeline を表示し、current channel は復帰用 state にのみ使う。

### Route / state
- `PrimarySection` は `timeline | live | game | profile` に限定する。
- invalid path と legacy `#/channels` は `#/timeline` に replace normalize する。
- private selection の保存は `selectedChannelIdByTopic` を正本にし、API 呼び出し時だけ `public` または `channel:<id>` へ変換する。

### Runtime / frontend API
- frontend/runtime には `importChannelAccessToken(token)` と `exportChannelAccessToken(topicId, channelId, expiresAt)` を追加する。
- import result は `kind` を持つ discriminated union とし、`topic_id`, `channel_id`, `channel_label`, `epoch_id` を共通 field とする。
- export result は `kind` と `token` を返すが、UI copy は `kind` を primary label に露出しない。
- `takePrivateChannelController(topicId, channelId)`（`take_private_channel_controller`）は `taken | not_connected | waiting` を返す。`JoinedPrivateChannelView.controller` は owner の端末で `this_device | other_device | moving | unknown`、owner 以外は `null`（#1219 AC-4）。

### Domain
- `invite_only` を含む private audience はすべて `channel-policy` と `channel-participant` に参加する。
- auto rotate と明示の rotate は、予約した新しい世代を操作 ID とする 1 operation として扱う（#1219 AC-2。§8 の「鍵更新の操作」）。
- participant redeem は read path から呼べる idempotent operation とする。

### 非公開チャンネルのランデブー
- `AppService` は参加中チャンネルの現在世代から派生済みランデブー鍵だけを実行時へ渡し、世代秘密そのものをコミュニティノード更新処理へ渡さない。
- 定期更新処理は公開話題だけを `public_topic_rendezvous_key` へ渡し、`hint/private/` で始まる話題には現在世代から派生済みの鍵だけを使う。
- 明示的な世代切替が成功した場合、接続中のコミュニティノードセッションは次のランデブー更新を直ちに実行対象へ戻す。

## Consequences
- channel を workspace と見なす実装は以後の正本ではなくなる。`Channels` tab や route を前提にした UI は削除対象になる。
- user は `topic -> channel -> workspace` を視覚的に追えるようになり、`Timeline / Live / Game` の scope change が left rail selection と一致する。
- owner control は減るが、runtime の epoch orchestration は増える。rotate/re-share の correctness は UI ではなく domain test と harness で守る必要がある。
- `invite_only` も epoch-aware になったため、old invite token と old epoch capability を前提にした test は fresh invite 前提へ更新が必要になる。
- この ADR と衝突する場合、`0012` にある次の前提より本 ADR を優先する。
  - invite-only を legacy hard-private baseline とする前提
  - audience 別に invite/grant/share/export UI を分ける前提
  - manual `Freeze` / `Rotate` を user-facing control とする前提

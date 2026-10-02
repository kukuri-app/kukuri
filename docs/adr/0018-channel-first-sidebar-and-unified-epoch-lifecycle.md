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

account 同期（ADR 0061 §2）の記録の契約:

- `channel/<channel id の hex>/controller` の値は上の記録の JSON（`{"device_id":"…","generation":1,"transfer_to":null}`）。書くのは担当端末と、#1219 AC-4 の引継ぎの遷移だけ。
- 採否は記録の中身で決め、`updated_at`・`op_id` では決めない（#1218 INVAR-3）。`generation` の大きい方を採る。同じ `generation` で `device_id` が同じなら、`transfer_to` の無い記録より有る記録を採る（旧担当の停止は戻らない）。同じ `generation` でそれ以外の食い違い（`device_id` が異なる、または両方の `transfer_to` が異なる値）があれば、受けた記録を採らずに手元の記録を保つ。
- `channel/<channel id の hex>/epoch/<epoch id の hex>` の値は `PrivateChannelEpochCapability`（`{"epoch_id":"…","namespace_secret_hex":"…"}`）。受けた鍵は追加で保持する。現在の世代は、新しい世代の replica にある owner 署名の policy の `previous_epoch_id` が手元の現在の世代と一致するときだけ進める（参加者の handoff の redeem と同じ検証）。
- 手元に参加の無い channel（行が無い、または退会の tombstone）の最初の現在の世代は、本人の端末の `channel/<channel id の hex>/membership` の item が示す世代とする（ADR 0061 §9）。その後は上の検証でだけ進める。

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

### Domain
- `invite_only` を含む private audience はすべて `channel-policy` と `channel-participant` に参加する。
- auto rotate は handoff grant の配布までを 1 operation として扱う。
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

# ADR 0055: 必要な通信の受付・実行・停止を一つのownerで管理する

## Status

Proposed。#1221のP2。基準は `984a491a3430f1da105bfe8ff49a877cbab4690f`。
実装済みの仕様は参照先ADRとコードで確認する。R3-Aのnode共通受付、R3-Bの表示再試行、R3-Cのapp-api保存境界は本番経路へ接続済みである。通知/DM・reader・旧同期の移行は後続条件として残る。
作業状態・AC・INVAR・採用判断の台帳は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) に集約する。

## Contextと決定の境界

同じpeer台帳の型を利用していても、gossip/docs/blobが別々のインスタンスと再試行を持つ。
現在のremote fetchは同時実行8件を制限するが、待機taskと要求台帳は制限しない。
topic/private/author/DMの購読、CN自己修復、UIの再読込みも別々に追加の作業を起動する。
頻度の低下だけでは登録対象と失敗の増加に対する上限を作れない。

2026-09-23のユーザー決定は次の二つである。

- D2: **現在の受信対象範囲を維持する**。画面外のtopic/DMを暗黙に通知対象から外さない。
- D10: **更新案内後、bucket切替時に旧版との新着相互運用を終了する**。期限付き互換期間は設けない。
  保存済み投稿、bookmark、参加状態、未送信outboxは保全する。

以下はその範囲を実現する技術案である。通知の完全配送、全followerの発見、全過去履歴の復元、
CNの必須化、通知一覧の共有replica化は追加しない。既存の認証・同意・audience・成人向け取得制限を維持する。

## 1. 需要とowner（D1・D5）

account runtimeが一つの `NetworkWorkOwner` を所有する。純粋な受付・選択・状態遷移は
`kukuri-transport`、実I/Oの組立てとendpoint世代は `kukuri-iroh-node` / `desktop-runtime` に置く。
app-apiは意味上の需要と検証済みscopeを渡し、gossip/docs/blobは選択結果を適用して観測を返す。
ownerは秘密鍵や投稿本文を持たず、capabilityの識別子と世代だけを持つ。

| 需要の理由 | 寿命 | 終了時の動作 |
| --- | --- | --- |
| 表示中のtimeline/thread/profile、表示中session manifest | UI observerの生存期間。非表示通知またはobserver破棄で終了 | 最後の表示需要が消えたら表示専用取得を取消す |
| 投稿・返信・DM等の明示操作 | 操作1回の期限。永続outboxの1回の試行は別需要 | 待機者が消えても受理した送信・通常取得の結果を記録する |
| 参加中のlive/game/Dome | 既存sessionの参加・draining・退出契約 | ウィンドウ非表示だけで参加を終了しない |
| 通知の受信 | account runtimeの寿命 | 対象ごとの常時購読ではなく§4の受信入口を維持する |
| 直近操作の補助、遡り、取り下げ確認 | 対象・期限付きの有限な要求 | 期限後は休止し、必要時に再登録する |

参加/follow/通知対象という永続的な利用者の意思と、現在の通信leaseを分ける。
登録履歴を起動時に全件読んでleaseへ展開しない。対象への操作・受信時に索引から個別確認する。
休止は退出・unfollow・既読・データ削除ではない。

## 2. 受付・容量・公平性（D3・D4）

要求キーは `(account, scope/capability generation, protocol, object/replica,
operation mode, persistence policy, byte limit)`。
同じblob hashでもLocalOnly、表示専用、一時取得、保存取得、別private epochは無条件に合流しない。
受付前にscopeと入力bytesを検査し、重複なら既存要求へ上限付きの待機者を追加する。
同じ値の需要更新は期限・世代が変わらない限りI/Oを起動しない。

受付結果は `Admitted / Joined / Deferred(retry_at) / Denied(reason)` を区別する。
満杯時にtaskをspawnしてSemaphoreで待たせない。実行枠を得た要求だけをownerのtask集合へ入れる。
UIには保存済み内容と取得待ち/取得不能を返し、空の結果を完全な履歴と解釈しない。
永続outboxはDBに保持し、due索引から実行枠分だけ読む。受付拒否や期限切れで未送信内容を捨てない。

予算は件数とbytesの両方を持つ。初期値は性能達成の根拠ではなく、調整可能な有限の制御値とする。
次の値は実装contractで境界を検証してから採用する。既存製品の有効な本文・添付上限を縮める値ではない。

| 資源 | 初期案 | 根拠・満杯時 |
| --- | --- | --- |
| 意味上の同期対象 | 64 | 既存session表示対象64を起点に共通枠化。保留対象をUIへ返し、全scopeを展開しない |
| docs同期handle | 128 | 64対象×現在/直前2bucketを上限に配分。author/古いbucketもこの枠を使う |
| remote取得実行 | 8 | 既存の8をaccount全体に統一。各serviceに8ずつ持たせない |
| 新規接続試行 | 2 | 現行gossip warmupの並列2を共通化 |
| gossip購読 | 81 | 表示等64、account受信1、短期送信先16。内訳も全体上限内 |
| 待機要求 / 待機者 | 256要求、1要求64待機者 | metadata合計4MiBも同時に制限。本文をqueueへ複製しない |
| 学習peer / peer状態 / 失敗記録 | 各1,024件、各4MiB | 期限索引から古い非実行記録を回収。全件sort/retainを通常経路に置かない |
| close/revoke中の操作 | 32 | P1の所有付き停止を再利用。停止未完了のhandleは128枠からも除外しない |

接続先は1要求につき最大4peerを選び、稼働接続はaccount全体32を初期案とする。
上流が内部で学習・保持するpeerと接続にも上限/解放を適用できることを確認するまでは、
外側の表だけでNET-AC-2を達成としない。

R3-Aでは、通常blob取得・表示用blob取得・公開docsページ取得をnode単位の同じ受付へ接続する。
公開docsは予算検査済みrequestとpeerを含むidentityで同一要求だけを合流し、別namespace・key・peerは合流しない。
app側の欠損本文・返信先・session表示はpermitを取った後だけtaskを起こし、満杯では次の需要へ延期する。
旧namespace定常syncとその内部peer保持はR5-Hの撤去対象であり、この接続だけで全通信I/Oの上限達成とはしない。

表示・操作、送信、背景の3laneを4:2:1の巡回で選ぶ。空laneの枠は他へ貸す。
重い転送が全枠を占有し続けないよう本文/manifestとmediaの実行内訳も計数する。
期限は受付時から単調時計で計り、待機時間を含む。通常blobは現行30秒、接続5秒・転送15秒を
残時間で切る。session/call等の既存deadlineを延長しない。期限切れは再試行成功と扱わない。
正当な大きい添付はstreamingと既存の種類別制限で扱い、queueのmetadata制限を本文長の制限に流用しない。

## 3. 差分・停止・回復（D6・D7・D9）

#1221 G3-3の2026-09-24決定により、下記の低水準namespace同期案は新bucketのreaderへは採用しない。
CN readerは公開と登録済みprivate epochを同じ対象key/件数/bytes指定のQUIC読取りへ移し、
private providerはepoch capabilityの証明を要求する。clientのauthor/private制御参照と保護cache回収は
同じB案で後続接続する。旧`start_sync`はlegacy writerとの移行期間に限る。既存の停止所有権と
LocalOnlyの禁止I/O境界は方式変更後も維持する。

R5-Bでclientの通常public/private投稿ページ、CN source、対象ID・session参照を同じ有界QUIC readerへ
接続する。1操作は最大4 bucket・局所200行・30秒、1応答1MiB・1record64KiBとし、privateは
当該channelの有限peer候補と保持中epoch capabilityだけを使う。旧writer形式は同じpage/exact
読取りで扱い、author/private制御参照はR5-C、writer切替と旧sync撤去はR5-Hへ残す。

- `peer -> 使用中lease` と `scope -> resource` の逆引きを持つ。peerの追加・削除・アドレス変更は
  そのpeerを利用する有界な対象だけへ適用し、全登録topic/replicaを再走査しない。
- gossipの追加は現行 `join_peers` を利用する。削除APIがない場合は**影響するtopicだけ**を
  ownerが停止・再joinする。sender/receiverとそのcloneをownerが解放する。
- docsは公開 `SyncHandle` と `net::connect_and_sync` / `net::handle_connection` を使い、
  ownerが選んだpeerへの1回の同期futureと、namespace別の受信許可を管理する。
  `Doc::start_sync`は保存peerを追加するため、この高水準APIのleave/startを選択先限定に流用しない。
  P2の実Iroh contractで、保存済みpeerへの接続0、未許可namespaceの反映0、取消時の接続終了、
  ローカル再open後のrecord保持を確認した。先行草案の「上流API修正が必須」は高水準APIだけに基づく
  判断だったため、この公開APIを使う構成へ修正する。
- storage actorと既存署名/同期wireを再利用し、native LiveActorの自動peer選択・gossip再同期・
  eager downloadへ作業を二重登録しない。低水準のsubscriptionから既存ReplicaNoticeへの変換、
  同期結果/必要blobの取得、close時のcancelとstoreの停止fenceは共通ownerのadapterが所有する。
  local docs APIにnative同期を暗黙起動する入口を残さず、移行時に全callerを照合する。
  この構成の本実装と、irohの内部address台帳の回収はP3の前提確認に含める。
- 全稼働対象の作り直しを許すのはaccount切替/終了、endpoint再構築、明示的なnetwork設定変更で
  既存endpointを再利用できない場合だけ。対象は現在の有界なlease集合であり保存済み全履歴ではない。
- observationはpeer・protocol・scope・endpoint世代を持つ。gossip neighbor成立、docs syncの観測、
  対象blobの検証済み転送をそれぞれの成功とする。seed適用の`Ok`、別protocolの接続、投稿がないことを
  回復判定に使わない。NotFoundと接続不能を区別し、休止対象のpeer 0は正常な休止である。
  `remote_info`の`TransportAddrUsage::Active`は保持中のpath情報であり、QUIC接続の生存判定にも使わない。
  接続終了はconnectionを保持しない`WeakConnectionHandle::closed`等の実際の終了観測を使う。
- Direct P2P → Relay Supported P2P → Relay Fallbackの順を維持する。
  CNの認証/同意と接続の健康状態は別に保持し、あるnodeの成功で他nodeのgateを開かない。
- queued要求の需要消失は削除、表示専用の最終需要消失はQUIC futureもcancelする。
  通常取得の待機者cancelはownerの実行と成否記録を消さない（#1207）。失効/終了はそれより強い停止理由。
- private失効・account切替はguard世代を先に無効化し、予約・実行・完了反映を拒否する。
  close/revoke自体は受理後にcallerがcancelしてもownerが完了する（P1）。失敗したleaveは隔離し枠を保持する。
  古いtaskが新しいhandleを消したり、旧世代のbytesを保存したりすることを禁止する。
- ownerの状態lockを保持したままnetwork I/Oを待たない。command受付、worker完了、需要更新、
  次のdeadlineだけで進める。timerはdeadline索引の先頭を待ち、起動遅延分を連続実行しない。

### 実装前に固定する境界と負例

#1221の固定head監査では、登録中のcancel、旧世代の遅い終了、権限確認後の別helperによる保存、
ページ処理中の継続挿入、IPC出力schemaの更新漏れが繰り返し見つかった。
次のPRでは変更した入口からsinkまでを先に辿り、**該当する**境界をcontractと失敗testで固定する。
全PRへ無関係なtestを追加したり、inventoryの全行を毎回読み直したりはしない。

- **所有と世代:** taskをspawnする場合は同じ非await区間でownerへ登録する。受付終了を先に通知し、
  登録待ち・実行中・終了待ちのどこでcallerがcancelされても、taskと解除待ちのleaseを失わない。
  同じaccount/topicの旧ownerは新世代のrouteやhandleを解除・奪還できない。
  比較と登録/解除は同じlockまたは同等の原子的境界で行う。
  `superseded`とtransport/endpoint再構築による終了を別の理由として扱い、前者は再登録せず、
  後者は新世代の空きrouteに限って復旧する。旧世代の実行中futureも停止する。
- **権限と反映:** `入口 → helper → sink`にある全awaitと、外部送信、復号済みblob保存、
  projection/通知/outbox変更を列挙する。scope・mutual・private epochとowner世代を
  I/O前だけでなく、取得後および各永続化・外部送信の直前に確認する。
  添付などの下位helperがcallerの最後のguardより前に保存しないことを負例で確認する。
  失効中のfutureは結果を破棄し、保護outboxを成功扱い・容量削除しない。
- **有限な窓と公平性:** `LIMIT`を付けただけで完了としない。無関係な履歴を10倍にしたときの
  読取り/待機回数と、処理中に毎回新規rowが増えるときのcursorの終端・古い未処理rowへの再訪を確認する。
  削除、同一時刻のtie、再起動後も索引順と再開位置を保つ。
- **公開契約の伝播:** Rust DTOやstore schemaを変えるPRは、CLIの厳格な出力schema、生成TS型、
  fixture、UI表示、migration/goldenを利用箇所ごとに逆引きする。
  型検査だけでなく実際のCLI出力validatorと表示上の意味（例: 上限値を`64+`と示す）を確認する。

既存の負例は`stale_same_account_lease_cannot_unsubscribe_a_new_receiver`、
`app_account_listener_reclaims_vacant_route_after_stack_rebuild`、
`revoked_mutual_during_attachment_fetch_never_persists_plaintext`、
`new_outbox_rows_cannot_starve_older_unacked_rows`、
`dm_status_and_conversation_outputs_accept_the_bounded_count_flag`を参照する。

既存不具合は修正前に失敗testで再現する。新しい経路は許可/拒否、cancel、世代置換、再構築、
連続挿入のうち該当する状態を実装前のcontractに置く。ローカルでは変更関連のtestだけを実行し、
全体の確認はPR CIで行う。独立監査はこの事前確認の代わりではなく、固定headで別に実施する。

## 4. 画面外の通知とDMの受信（D2）

[ADR 0023](0023-local-notification-inbox-v1.md) の通知一覧・未読は端末SQLiteを正本とする。
OS通知のenabled/種別/preview設定はtoastの設定であり、ローカル通知一覧の対象を削らない。
kindはmention/reply/repost/quote_repost/direct_message/followedのまま。
自己投稿、query hydration、再起動時の過去読込みを新着通知に変えない。followedは現行の観測範囲を維持し、
全follower発見に拡張しない。

採用候補は**accountごとに一つの暗号化された受信route**。送信側は新しい投稿・edge・DMの確定時に、
必要な受信者だけへ通知参照を送る。topicの全参加者や全followerへ送る方式ではない。
受信者のpublic keyからrouteを導出する。ただしroute IDだけからendpointが判明するとは扱わない。
送信にはaccount署名付き `ReceiveEndpointBinding`（版、account、route、endpoint ID、発行・失効時刻）を使う。
接続時は署名のaccount/routeと実際のQUIC endpoint identityを照合する。
2026-09-24の#1221の決定では、CNなしでaccount公開鍵だけから未知endpointを発見する新機能は要求しない。
送信側はmanual ticket、configured/bootstrap seed、既存接続・既知peer、または同意済みCNの候補を有限窓から選ぶ。
チケットのみへ狭めず、候補は接続時の短命bindingでaccount/routeと実QUIC endpoint identityを照合した後だけ採用する。
検証済み宛先は対象別の有限cacheへ保存する。著者replicaへendpoint locatorや短命bindingを保存しない。
接続時交換はmanual ticketとDHTで得たpeerにも適用する。peerの自己申告やunsigned Presenceから
accountの対応を確定しない。DHTは既知endpoint IDのaddress解決だけを担う。
CN使用時は自分のaccount routeと短期送信先routeをtopic rendezvousへ差分登録し、返却peerへ
bindingを確認する。rendezvous応答自体をaccountの証明にしない。設定nodeのauth/consentを維持する。
CNのtopic presenceは15秒の時間窓を直近4個だけ候補探索し、topic-peerを45秒、窓keyを60秒で失効させる。
窓の時刻は共有Valkeyの`TIME`を使い、複数CN API hostの時計差を候補漏れへ持ち込まない。
Valkeyへの接続・応答は各2秒で打ち切り、失敗で全件走査や即時retryへ拡大しない。
各窓から最大16件を標本し、topic所属期限とpeer情報を最大64件だけ検証して最大8件を返す。
Redis `SRANDMEMBER`の正のcountは[返却数に比例する操作](https://redis.io/docs/latest/commands/srandmember/)であり、
topic全参加者の`SMEMBERS`は使わない。候補の完全列挙や応答だけによるaccount認証は行わない。
bucketへの追加とTTL設定は[Valkey互換の`MULTI/EXEC` transaction](https://redis.io/docs/latest/develop/using-commands/transactions/)で
一体に実行する。caller取消や応答喪失時にTTLなしの新bucketを残さない。
CN不使用時は、ticket・seed・既存接続・既知peerの到達情報から候補を選んでbindingを交換する。
account公開鍵しかなく、到達情報がない相手のendpointは新たに探索しない。全author/全peerを探索しない。
binding未取得/期限切れ/宛先不在は未解決として延期し、接続成功や配送成功を捏造しない。
実際に成立したgossip接続は既存のaccount別learned台帳へ記録し、docs/blobで学習したpeerも宛先候補に再利用する。
manual ticket、seed、3種のlearned候補はaccount別cursorで巡回し、1試行は最大4候補・同時2照合までとする。
候補全体を複製・整列せず、照合済み宛先はaccount別に最大1,024件、署名の期限を超えず最長10秒保持する。
未認証候補を配送先へ渡さない。配送失敗時は該当account/endpointのcacheを失効させ、次の試行で再照合する。
この窓はCNからの候補取得とoutbox再試行を代替しない。
endpointを再生成した側は接続時の短命bindingと利用中CNのpresenceを新endpointへ切り替える。
端末を協力的に外すときは、自端末の候補とCNの該当presenceを削除し、古い候補を現行stateへ戻さない。
更新がまだ到達していない間の完全配送は保証しないが、peer再発見で未完了送信を再試行する。
DHTへ通知本体を置かず、CNに通知一覧や必須の保存queueを作らない。

**2026-09-24の判断:** 短命bindingの著者replicaへの周期保存に加え、2026-09-23の変化時更新locator案も失効した。
CNなしのaccount-only発見を要件から外し、著者制御stateの保存・読取り・回収を新設しない。
複数端末の既知候補とCN候補は有限窓とcursorで再訪する。候補不在やbinding検証失敗では
配送を成功扱いにせず、保護outboxを維持する。送信側のroute購読も短期leaseとして上限内で開く。

受信capsuleには署名された対象参照または既存DMの暗号化frameへの参照を入れ、
account鍵宛の暗号化で包む。送信者・受信者・参照先・版・期限を暗号化内部の署名対象に束縛する。
private参照はさらにepoch鍵で保護し、外側を復号できるだけではscope/hashを取得できないようにする。
epoch選択はcapabilityから導出する識別子の索引で行い、全epochの試行復号をしない。
公開routeのmetadataは新たな外部送信であり、下記分類と外部送信台帳へ反映する。

参照は権限証明でも通知種別の確定値でもない。受信側はscope参加・capability・署名・元投稿・
reply/repost/mention/edgeの現行条件を確認してから、既存の検証・保存・通知生成へ渡す。
private blobのhashを公開routeの中継peerへ問合せない。検証済みの送信元providerまたは同じepochの
許可peerに取得先を限定する。元内容が確認不能なら通知を捏造せず未解決として期限内で延期する。

DMは [ADR 0020](0020-pairwise-dm-v1.md) の暗号frame、署名ACK、mutual gate、local transcript、
削除tombstoneと永続outboxを再利用する。輸送routeの変更でmessage IDや暗号frameを再生成しない。
ACK未確認のDMを短期通知queueの期限で捨てない。相互follow失効中は送信/再試行せず、履歴を残す。
通常の通知参照はbest effortの期限付きcacheであり、DMの永続配送待ちとは別扱い。

旧DM outboxはbody/message ID/暗号frameを変更せず、**輸送先だけaccount routeへ再解決する**。
これは投稿のdocs書込み先を変更しない規則の例外である。ACKも送信者account routeへ返し、
既存署名ACKのsender/recipient/message照合を保つ。再起動/途中中断は同じmessageで再開し、
旧pairwise受信と新route受信が重なっても既存message IDとtombstoneへ収束する。
送信側の既知peer候補はpeer別outboxページ（最大64行）で1回だけbindingを照合する。
未解決なら保護rowを残し、照合済み宛先には同じ暗号frame hashとmessage IDをsealed offer内で直接知らせる。
初回の画面操作はこの宛先照合を待たず、既存のbackground再送がaccount routeへ進める。
受信側はbinding照合済みprovider endpointへ署名ACKをsealed offer内に直接収めて返し、送信側account routeでも既存の
sender/recipient/conversation/message照合を通す。旧pairwise ACKも移行中は維持し、両routeの重複ACKは
最初に記録した配達時刻を保持する。ACKのofferにはACKを返さず、未ACKのDM outboxは受信確認まで消さない。
送信offerとACK offerの1回の待機は各2秒で打ち切り、失敗は保護outboxの次回再送へ委ねる。
同一account runtimeが同時に発行するDM/ACK offerは最大4件とし、満杯時は待機列を作らず延期する。
DM outboxの周期再送は相手ごとの購読taskから分離し、account runtimeの1 ownerが2秒ごとに処理する。
端末内のdue索引から未試行の古い順を最大3行、期限到来済み再試行の古い順を最大1行選ぶ。
各行は試行時刻を永続更新してから送信可否を確認し、送信不可/失敗でも保護rowを消さない。
これにより継続する新規送信が古い再試行を押し出さず、古い失敗が新規送信を塞がない。
索引の先頭4行以外や全peer/全outboxの読取り、相手ごとのretry timerは通常tickで行わない。
ownerは再起動時のprojection復元後に取得し、shutdown/予期しないowner dropで処理中の照合・送信を取消す。
旧pairwise hint送信も1回2秒で打ち切る。保留したpeerのためにaccount ownerの残りのlaneや
同じ行のaccount offerが無期限に止まらないようにし、失敗しても保護outboxを維持する。
更新済み端末同士の未完了DMを保全するための移行であり、旧版との互換期間は設けない。

private rotation/freeze/失効には投稿通知と別の制御capsuleを使う。
現在のhandoff grant作成時に、旧epochの受信者別grantをaccount宛に配送待ちへ登録する。
制御capsuleは**既知の旧epochで保護**し、旧epoch/受信者/署名済みgrant参照を束縛する。
新epochだけで暗号化して、まだその鍵を持たない参加者へ送らない。
受信側は旧epochの個別grantを確認し、既存の復号・owner署名・policyの前後epoch・audience検証を
通した後に世代を更新する。grantの存在だけで参加を許可しない。
新epochに移れない/既知の失効状態なら新規取得0のまま保持し、公開取得や全epoch試行で救済しない。
offline中の連続rotationは既存の受信者別grantをcursor付きで1回に1遷移ずつ処理する。
永続grantと制御配送のdue索引は途中停止から再開し、全参加者分taskを一斉生成しない。
新epochの投稿が先着した場合も制御経路でgrant確認を待ち、欠損を通知/退出へ変換しない。

DocEvent経由と受信route経由の同じeventは既存通知IDへ収束させる。
reply > quote_repost > repost > followed > mentionの優先度、既読状態、toast二重発火禁止をcontractで固定する。
旧保存済み通知を全件書き換えず、対象sourceの索引lookupで新旧dedupeを処理する。

この節のwire bytes、暗号domain、capsule上限、通知cache期限、受信rate制限は
次の実装contractで固定する技術残件。対応する二端末contractが通るまで既存受信経路を削除しない。
公開routeによる到達、private epoch隔離、offline DM再開の実証前にP2完了としない。

### 4.1 endpoint bindingのwire契約

P2の実現性確認として `core::ReceiveEndpointBindingV1` と
`transport::ReceiveBindingProtocol` を実装した。P3ではnodeのRouterへ空の
`ReceiveBindingSlot`を登録し、account鍵のload後にだけ有効化する。要求時に短命bindingを署名し、
更新timerを持たない。stack再構築では同じaccount鍵を新nodeへ先に導入し、旧nodeの停止開始時に
新規要求を拒否する。bindingの登録だけで通知/DM受信経路を置換しない。

- routeは `receive::v1::<hex>`。hexはBLAKE3の
  `b"kukuri:account-receive-route:v1\0" || accountの32byte公開鍵` の小文字hex。
- wireはversion、account、route、endpoint_id、issued_at_ms、expires_at_ms、signatureを持つJSON。
  公開鍵/endpoint IDは小文字hex64桁、署名は小文字hex128桁。未知field/未知版を受け入れない。
- 署名は固定順JSON配列
  `["kukuri:receive-endpoint-binding:v1", version, account, route, endpoint_id, issued_at_ms, expires_at_ms]`
  のUTF-8 bytesをSHA-256にし、既存account鍵でSchnorr署名する。
- bindingの最大寿命は300,000ms、発行時刻の未来許容は60,000ms、失効時刻は排他的。
  署名が正しくても失効後の利用を許可しない。複数端末は同じrouteに別endpointのbindingを持てる。
- 交換のALPNは `/kukuri/receive-binding/1`。双方向streamへrequest `[1]` を送りFIN、
  responseは最大1,024byteのbinding JSONとFIN。受信上限はdeserialize前に適用する。
- serverは同時2要求まで、超過は待機せず接続を閉じる。1要求は2秒で終了する。
  これはprotocol処理枠であり、上流のQUIC handshake/全接続数の上限を証明するものではない。
- clientはownerの選択候補1件へ接続し、受付時からのdeadline内で照合する。内部retryを持たない。
  照合先はwire中のendpoint IDの自己比較ではなく、QUICが認証した `Connection::remote_id()`。
  結果・失敗・caller取消のいずれでもこの短期接続を閉じる。
- 固定bindingを交換するproof handlerの更新は同一account/endpointの新しい発行時刻だけ。
  本番slotは要求時署名で失効を避け、同一nodeへの別account導入を拒否する。account切替/endpoint再構築は
  runtimeの世代切替でslotごと置換する。未知accountのlistenerを旧accountのslotへ混ぜない。

この交換が確認するのはaccountとendpointの対応であり、投稿scope・private能力・通知条件は
後続の受信guardで別に確認する。外部送信は公開bindingだけで、秘密鍵・private参照・通知本文は含めない。

### 4.2 暗号化された受信参照のwire契約

`SealedReceiveOfferV1`は最大2,048byteのJSON。既存gossipの4,096byte上限を増やさない。
account routeではこのJSONを専用frameとして使い、旧topic用`GossipHint`へlocatorを詰め込まない。
gossip topic IDはrouteのUTF-8 bytesのBLAKE3で、既存topic通知の`hint/`接頭辞は付けない。
一つの受信routeへpublic source、DM、private source、epoch controlの参照を届ける。

- 外側はversion=1、一回限りのsecp256k1公開鍵、24byte nonceの小文字hex、ciphertextの小文字hex。
  ciphertextにAEADの16byte tagを含め、平文は最大880byte。decode前と個別fieldの両方を制限する。
- 受信者のaccount公開鍵とのECDHは既存のx-only parity正規化を再利用する。
  HKDF-SHA256のsaltは `b"kukuri:receive-offer-key:v1"`、IKMはECDH共有値、infoは次のAAD。
  AADは固定順JSON配列 `["kukuri:receive-offer:v1", 1, ephemeral_pubkey, recipient]` のUTF-8。
  XChaCha20-Poly1305で暗号化し、nonceとephemeral鍵は生成ごとに更新する。
- 内側はversion、sender、recipient、reference、issued_at_ms、expires_at_ms、signature。
  blob参照のreferenceはprovider_endpoint_id、payload_hash、payload_bytes、scopeを持つ。
  scopeのkindは `public_source/direct_message/direct_message_frame/direct_message_ack/private_source/epoch_control`。
  `direct_message_frame`はdm_id、message_id、frame_hashを持ち、provider_endpoint_id以外のblob参照fieldを省く。
  `direct_message_ack`はdm_id、message_id、acked_at、ACK署名を持ち、provider/blob参照fieldを省く。
  省いたfieldは0byte/空値だけを許可し、inline scopeがblob参照を同時に持つ形を拒否する。
  endpoint/hash/key IDは32byteの小文字hex。未知field/版を拒否する。
- 署名は固定順JSON配列
  `["kukuri:receive-offer:v1", version, sender, recipient, reference, issued_at_ms, expires_at_ms]`
  のSHA-256に対するaccount Schnorr署名。referenceのfield順は上の順、scopeはkind、epoch_key_idの順。
  公開鍵暗号を作れるだけでsenderを名乗れないよう、復号後に署名も検証する。
- offerの寿命は最大5分、未来許容1分、失効は排他的。期限切れofferは配送済み/既読と扱わない。
  DM outboxと永続grantの再試行は、内容のIDを保って新しいofferを発行する。
- blob参照のpayload_bytesは参照manifestの実byte数で1〜65,536。本文・DM frame・添付の全体サイズではない。
  inline DM/ACKはpayload_bytes=0と空payload_hashを要求し、新たなmanifest blobを保存しない。
  取得側は宣言サイズと上限を両方検査し、宣言と異なるbodyを採用しない。
  既存の本文/DM/添付をこのmanifest上限へ縮めない。

公開通知のmanifestはversion=1と署名済みpost/repostまたはfollow envelopeを持つ。
post/repostには書込みreplica・本文（署名済みpayload refとのhash/長さ照合）を添え、返信時は署名済み親投稿で返信先の著者を検証する。
送信先は投稿内の明示mention、返信先著者、repost元著者、follow対象に限り、全follower探索をしない。
送信はaccount所有の1 worker・最大64件の待機窓で進め、表示・投稿操作を宛先数や接続待ちで止めない。
manifestは1GiB/非利用7日のapp所有cacheから再提供し、受信側は署名・public scope・対象を確認して既存の通知IDと保存sinkへ合流する。

private manifestはさらに `PrivateReceivePayloadV1` としてepoch内で暗号化する。
元の参照manifest平文は最大16,384byte、JSON wireは最大65,536byte。
key IDはBLAKE3 keyed hashの用途 `b"kukuri:receive-epoch-key-id:v1\0"`、暗号keyは別用途
`b"kukuri:private-receive-payload:v1\0"` を用いる。keyはepoch secret、入力は用途の後に
channel/epochの順でそれぞれのUTF-8 byte長（big-endian u32）とbytesを連結する。各IDは1〜1,024byte。
AADは `["kukuri:private-receive-payload:v1", 1, epoch_key_id]` のJSON bytes。
24byte乱数nonceとXChaCha20-Poly1305を使い、channel/epoch/secretが違う復号を拒否する。
制御manifestは既存の受信者別暗号化grantを参照し、新epoch secretを旧epochの共通平文にしない。

復号済みofferの署名はI/O許可ではない。受信者はprovider binding、scope参加、DMのmutual、
private能力と世代を確認してから、そのproviderへmanifestの取得を要求する。
このwire部品はnetwork・store mutationを行わず、共通ownerと保存への組込みは後続工程が所有する。

## 5. 保存・再起動・bucket切替（D8・D10）

[ADR 0054](0054-time-bucketed-docs-replicas.md) のlocator・writer・回収契約を共通ownerへ接続する。
旧#1293のstructured locator草案は履歴であり、公開P1の `source_replica_id` を無条件に置換しない。
private/manifestに必要な情報だけを版付きで追加し、旧locator不明時に全bucketを探索しない。

- 起動時はaccountと移行状態を点読し、表示需要と保護outboxのdueページだけを復帰する。
  cached peerと失敗期限は件数/bytes上限内で読み、古いendpoint世代を成功として復元しない。
- GCは保護object/依存blobの参照を確認して最大128件ずつ進め、進捗を永続化する。
  namespaceに本人投稿があるという理由だけで全remote cacheを保護しない。
- 移行順序はCN reader → client reader/受信route/common owner → writer切替。
  readinessを確認せず新形式へ書かず、CN不使用のP2P経路も成立させる。
- 更新案内に旧版との新着相互運用の終了を明記する。切替後に新規操作を旧形式へ二重書込みしない。
  既存docs outboxは記録済みの宛先・署名IDで完了し、成功済み片側を重複投稿しない。
  DMの輸送先は§4に従って移行し、旧pairwise topicへ送り続けない。
- backupには切替状態・保護outbox・移行cursorを含める。restore後も旧常時同期へ戻さない。
  rollbackは新形式を解釈できる版に限り、旧形式しか解釈しない版での書込みを拒否する。

## 6. 周期処理の登録規則

通信・取得・復旧の周期処理を機能ごとに新設しない。理由・scope・期限・取消・容量・成功観測を持つ
owner要求として登録する。OS通知dispatch、UI描画、音声/videoのmedia clockは通信retryのownerではない。
これらの既存timerを維持する場合も、新たなnetwork workはownerへ渡す。
CN indexerは別processなので独立したownerを持つが、同じ受付・停止・差分の契約を使う。
local通知のevent転送taskもaccount runtimeが所有する。shutdownでは終了を待ち、明示shutdownを
経ずruntimeが破棄された場合も中止する。旧accountのNotifyを保持する待機taskを残さない。

OS通知dispatchはlocal inboxの全件読取りを行わない。新規INSERTにだけ単調な永続sequenceを付け、
64件の索引ページを処理してからcursorを進める。既存rowはmigration時にsequenceを付けず、
初回起動・account切替・restoreでは現在のheadをbaselineとして過去toastを抑える。
同一`received_at`の件数に依存するID集合は保存しない。ページ間でaccount切替guardを解放し、
quiet/read/self/種類設定と成人向けpreview gateを従来どおりOS表示の前に適用する。

## 7. 状態遷移と検証

以下のtest名は追加予定のcontract識別子であり、成功済みの証拠ではない。
inventoryの各行から同じ契約へ接続し、差分を実装する段階で実test名と証跡を記録する。

| ID | sequence | 許可/禁止する結果 | contract |
| --- | --- | --- | --- |
| NW-1 | 同じ表示需要を再送、登録履歴だけ10倍 | 同じleaseへ合流、対象外の走査/I/O/spawn増加0 | `unchanged_demand_has_no_io` |
| NW-2 | 異なる要求を枠以上に登録 | queue件数/bytes以下、超過はDeferred、待機task 0 | `admission_bounds_waiting_work` |
| NW-3 | queue待機中に期限切れ/失効 | I/O 0、旧世代の完了保存0 | `expiry_and_revocation_dominate_io` |
| NW-4 | 表示の最終observer離脱/通常取得の待機者離脱 | 前者はstream停止、後者は所有と成否記録維持 | 既存session cancel / #1207 + 統合contract |
| NW-5 | peer1件変更、無関係なtopicは休止 | 影響対象だけ変更、休止/無関係のrestart 0 | `peer_delta_is_scoped` |
| NW-6 | CN apply成功、gossip不通、blobのみ成功 | gossip backoff維持、未同意nodeへのI/O 0 | `recovery_requires_protocol_observation` |
| NW-7 | close途中cancel、leave失敗、endpoint交換 | owner完了/隔離枠維持、旧世代再反映0 | P1 lifecycle + owner統合contract |
| NW-8 | 非表示topic通知、offline旧DM、同一sourceの重複受信、endpoint更新、CNなし | 署名bindingで到達、outboxの新輸送先/ACK維持、二重toast 0 | `account_receive_preserves_scope` |
| NW-9 | private参照を公開routeで受信、未許可epoch/偽署名、非表示中のrotation | 秘密metadata露出0、不許可provider取得0、旧epoch grantからのみ更新 | `sealed_receive_requires_scope` |
| NW-10 | 移行各段階の中断/restart/restore/rollback | 保護データ保持、冪等再開、旧常時同期0 | CN/client移行contract |

P3では表示→受付→接続/取得→停止を先に一往復させ、同じ契約で旧管理経路を順次撤去する。
P4でwriter/private/CN/保存の切替を完成する。各段階は登録件数10倍に対する処理回数をassertする。
局所実装中のローカル検証は関連contractのみ、全体はPR/CIで行う（ユーザー指示）。
P2の技術残件とinventoryの未分類を解消してから設計をAcceptedにし、実装と混同しない。

## Feature Data Classification

| 項目 | 分類 |
| --- | --- |
| Feature 名 | 需要別の通信管理とaccount受信route |
| Durable / Transient | lease/task/接続観測は一時。利用者の意思・保護outbox・移行位置は永続。失敗cacheは有限 |
| Canonical Source | 投稿/edgeは署名済みdocs/blob。通知未読/DM transcript/outbox/設定は既存の端末store |
| Replicated | 通知一覧・lease・未読は共有しない。署名済みsourceと既存暗号DMを送受信する |
| Rebuildable From | cacheは検証済みsourceと明示需要から。通知履歴を全過去投稿から再生成しない |
| Public / Private / Local Only | 公開route ID・接続metadataは公開の到達情報。本文/参照はaccount暗号化。privateはepoch内。owner台帳はlocal |
| Gossip Hint | account宛の暗号化capsuleを追加予定。version/domain/上限のcontractを先行する |
| Blob | 既存source、暗号DM/添付を再利用。private hashの取得先もscopeで制限 |
| SQLite projection | 既存通知/DMに対象別dedupe索引、due outbox索引、移行cursorを追加予定 |
| 必須contract / scenario | NW-1〜10、2端末のpublic/private/DM、CN認証mixed state、移行中断、10倍回数比較 |

詳細な入口とsensitive sinkは [通信作業inventory](../architecture/network-work-inventory.md) を参照。

## 受付状態機械の実装境界

`transport::work_admission::NetworkWorkOwner`は§2/3の受付と停止指示を実装するI/Oなしの部品。
scope登録時にowner固有・再登録固有のtokenを発行し、account/private世代の変更時に失効させる。
意味上の認証・capability検証をtoken発行の前に行う責務はapp-api/runtimeに残る。
request keyはscope token、protocol、完全な対象のdigest、mode、persistence、byte limit、deadline、lane。
異なるdeadlineを合流させず、合流で試行時間を延長しない。同じkeyでもmetadataが違えば拒否する。
LocalOnlyはこのnetwork受付へ登録せず、既存local readに返す。

初期実装は64 scope・256要求・64waiters/要求・4MiB metadata payload・8実行を制御する。
固定サイズkey/索引/waiterの管理領域は件数上限で別に有界にし、payload予算を総RSSと表現しない。
表示の最終waiter離脱、失効、期限切れは取消指示を出し、I/O終了のackまで実行枠とpayload予算を保持する。
通常取得は待機者が消えても結果を所有する。queued需要の消失はI/Oを起動せず除去する。
executorは`start_next`の結果だけを実行し、取消を停止/awaitしてから`complete`する。
`Publish`は世代と期限の判定だけであり、scope/内容/保存先の既存guardを省略する許可ではない。
表示/通常取得へのproduction接続は下記adapterで行う。docs/gossipのprotocol別予算、SDK内部の取得、peer選択の統合は未完了。

## gossip adapterで再利用する公開API

D9のI/O所有は`iroh_gossip::proto::topic::State`と公開`topic::Message`を使う。
選択した接続への書込み、各topicのtimer、受信読み込みと取消をownerが管理し、native Gossip actorと二重に動かさない。
既存wireはtopic IDを含むstream headerの後に、u32 big-endian length付きpostcard topic Messageを流す。
上位`proto::State`のtopic付きMessageをそのままwireへ書かない。
実証ではnative peerと双方向配送し、未完結headerの読込みもfuture取消でQUIC closeできた。
複数topic/peer・timer・台帳・worker予算のproduction統合は未完了で、この実証だけでD9全体を完了にしない。

## 表示・通常取得の本番adapter

`IrohDocsNode`は`NetworkWorkRuntime`を所有し、明示的なremote表示/通常取得を同じ`NetworkWorkOwner`で受付する。
このadapterの初期値は64scope・8準備/実行枠・1flight64waiters。表示はconsumerごと、通常取得はservice/retry台帳とflight keyごとにscopeを持つ。
serviceはretry台帳の作成時に割り当てる非再利用のプロセス内IDで識別し、Arcのメモリアドレスを識別子にしない。
通常取得の合流は従来の境界を維持し、LocalOnly/表示/一時/保存/異なるbyte limitを新しく合流させない。scope認証の代替ではない。
待機中は有限queueへfutureを保持し、実行枠を得たものだけを1つのdriverが起動する。通常取得と表示の合計が8を超えない。
既存walk Semaphoreは表示の準備で併用するが、通常取得taskの待機には使わない。

最初の受付から待機・取得・完了判定まで同じ30秒deadlineを使い、合流で期限を延長しない。
開始済みの通常取得は待機者が消えても結果とcooldownを記録する。実行枠を待つqueueの最後の需要が消えれば、後からI/Oを起動しない。
表示はcaller futureの破棄で止める。node終了は受付を閉じ、queue/実行を取消し、結果を返さない。
完了callbackは受付lock外で実行し、retry結果を記録してからflightを退役する。queue futureのDropもlock外で行う。
取得bytesの既存scope/token/内容検証と保存先のguardはapp-api等のcallerが維持する。
R3-Cではapp-apiの本文・返信先・session・投稿添付を同じaccount閉鎖/参加世代の保存境界へ接続する。remote本文と投稿添付は取得中にblobを保存せず、取得後に現行の参加世代・取り下げ・添付所属・成人向けgateを再確認してからblob/projectionへ保存する。private退出と同じepochへの再参加は別世代とし、古いfutureを取消す。通常取得のcaller取消では上記のownerが成否を保持し、sessionの表示専用futureは最後のobserverが離れたら止める。添付の表示需要に伴うUI bytes/object URLの回収はR1-C、remote cache全体の所有と回収はR5-Aに固定する。

失敗cooldownは従来の3秒・service別keyを維持し、1,024件・key256byteに制限する。
期限索引で回収し、満杯なら期限の近い記録から捨てる。一時cacheであり、未送信outboxや試行回数の永続記録ではない。
queueに保持する診断ラベルはsubject128byte/error4,096byte、hash/flight keyは固定hash等の短い識別子だけ。
本文・添付をmetadataとしてqueueへコピーしない。通常取得のworkerには既存のper-service成否記録callbackを持たせる。

公開bucketの明示的なQUIC key/exact読取りは、1要求ごとにDocs workとして同じ64scope・8実行枠と
30秒deadlineへ登録する。clientのCN検索結果では候補最大4peer、8対象並列、batch全体30秒に制限し、
期限/取消後のQUIC要求を継続しない。署名・scope・取り下げはapp-apiの既存gateで検証する。
native docsの自動downloader、gossip、peer台帳、意味上のscope世代/需要理由の全caller接続は未完了。
このadapterを通る取得の合計上限を、SDK内部を含む全通信の上限達成と読み替えない。

## blob protocolのpeer観測と候補選択

2026-09-24のR2-A判断では、learnedの到達候補をaccount DBに30日・論理使用量64MiBまで保存し、期限/容量を超えた古い候補を回収する。保存中は索引cursorから有限窓で再訪する。明示ticketと設定済みseedはこの回収の対象外とし、再発見が必要な期限切れ候補はDHT/CNまたは再ticketに委ねる。候補台帳はdocs/blob/gossipの入口を区別し、account切替時は別DBを使う。現在のhot健康履歴はprotocolごとに分け、blobは実転送、docsは`SyncFinished`の結果、gossipは実neighbor成立だけを成功として記録する。旧記述の「docs-syncとblob-serviceがblob観測を共有する」は失効する。

`IrohDocsNode`の`BlobPeerHealth`はblob ALPN専用とし、docsとgossipは別の有界な観測履歴を持つ。docs/blobの候補入口も別にし、他protocolの成功だけで候補へ追加しない。gossipの明示候補はaccount DBから最大4件を巡回し、`MemoryLookup`への手動登録は有限窓に留める。SDK内部の既存peer保持と旧`start_sync`の撤去は後続のR5-Hで確認するため、R2-Aだけで総通信経路の有界化達成とはしない。

実行/待機のnode予算とは別に、観測とP2P頻度subjectを各1,024件にする。実行中のattemptは観測をpinして期限まで保持し、遅い旧世代の結果で新世代のstatus/backoffを上書きしない。頻度窓を途中でevictして制限を迂回させず、満杯は延期する。3秒のhash別cooldownは前節のまま。

fetch側は各sourceのcursorから最大4 IDを読み、直近に成功した2 ID/30秒以内の新peer4 IDを候補へ加える。入口台帳に存在する場合だけ最大12件のaddressをmaterializeし、最大4peerを選ぶ。明示的に取り込んだ直近ticketは、既存の成功peer4件がいても4枠へ含める。1peer内の接続候補は既存のdirect→remote_info→relay順を変えない。source台帳の永続意思と現在の選択窓は別であり、この窓から外れても保存済み投稿を削除しない。

remote hashの型付き欠損はpeerの接続失敗とせず、欠損counterへ入れる。pin済みiroh-blobsは実際の欠損にも`ERR_INTERNAL(3)`を返すため、このコードをNotFoundと断定せず別の拒否/内部結果counterへ入れる。どちらも接続不能のbackoffを増やさず、別addressで同じendpointを再試行しない。ローカルstoreの失敗もpeerへ転嫁しない。署名・capability・audienceの検証とprivate hashの取得先制限をこのscoreで代用しない。

# desktop の blob と remote 内容の保持

現行の受入条件と予算は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R1-C・R3-B/C・R5-A に従う。本書は保存先と取得経路の対応を示す。旧 #1207 時点の試行回数や無期限 cache の記録は現行仕様ではない。

blobの取得候補（[#1594](https://github.com/kukuri-app/kukuri/issues/1594)）は、既存のpeer台帳の有限cursor窓とhash別の取得元を合わせて最大4件。取得元とhash別の失敗抑止は一つの台帳に全体256組まで保持し、hint/検証済み取得成功から600秒で失効する。SDKの明確な欠損、またはSDKの欠損/拒否後に独自cache配信が明示した欠損では、そのhashの優先を解除し、残りのTTL中は一般候補経由でも再選択しない。同じpeerが提供する別hashやpeer全体のtrustは下げない。SDKの`ERR_INTERNAL`等の曖昧な拒否を、それだけで欠損とは扱わない。

接続/転送の失敗・timeout・曖昧な拒否は、そのhashとpeerの組だけを3秒抑止する。独自cache配信のtimeoutや通信エラーも欠損にはしない。抑止中の同じtopic/見出しのhint再登録では抑止を取り消さず、期限後に再選択し、検証済み成功でTTLを更新する。通常取得の3秒cooldown、表示需要の最大4試行・5/30/120秒、1取得の総期限30秒と需要ownerは維持する。選択・更新時に期限切れの組を回収し、常駐probeや新規provider探索は行わない。

| 所有者 | 内容と取得 | 保持・回収 |
| --- | --- | --- |
| Iroh SDK `<db>.iroh-store`（新しい store、#1221 R5-I） | 新形式の docs の内容、本人が書いた本文・添付（保護所有先と両方）、pin された資産 | backup には含めない。SQLite を失ったときに docs から本人の投稿を戻す既存の復元に使う |
| Iroh SDK `<db>.iroh-data`（旧領域） | 更新前の本人データ・pin・旧同期の他人の内容 | R5-G の移行が本人データを保護所有先へ、R5-I の移行が本人の docs entry・pin を新しい store へ、最近の他人の内容を cache へ移し、移し終えたら名前を変えて 1 回 128 件以内で消す（ADR 0048 §7.1）。R5-A の容量達成のために一括削除しない |
| account SQLite の remote cache | 検証済み公開bucket record、remote blob、投稿projection・索引と表示用ラベル根拠。大きなremote blobは同じ台帳に紐づくファイル | 非保護分を合計3GiB（#1395 AC-4 で1GiBから変更）、非利用7日、1処理128件以内で回収。取得中は1MiB単位の予約を計数し、容量不足では取得を延期する。bookmarkが参照する共有blobは参照が残る間保護する。ファイルの削除は台帳transaction確定後に行う |
| desktop の表示用ファイル・URL | viewport内の投稿・DM添付をファイルへ処理単位で転送し、Tauri asset URLで表示。投稿の画像はカードに並べた先頭4枚と、画像viewerで表示中の1枚だけを取得する（#1690） | viewport外へ出た取得中の要求は止める。取得済みのURLは、最後に表示の対象だった順に48件・64MiBまで残し、戻したときに取り直さない。上限を超えた分と、画面離脱・gate切替・account切替ではURLを解放し、専用の一時ファイルを削除する（#1419）。起動時にも残存ファイルを削除する。手元にある添付は他の取得の待ちに並ばない。remote取得の同時数はnodeの受付が上限を持つ。URLは最大256件 |

desktop の通常remote blob取得は一時bytesを返し、それだけでは保存しない。本文・添付・sessionのconsumerがaccount/参加世代・対象hash・取得gateを確認してから `put_remote_blob` でcacheへ書く。表示用添付は別のファイル経路で取得し、同じgateを再確認してから成人向け以外のremote blobをファイルのままcacheへ保存する。3GiBを超える添付はcacheへ入れず表示用ファイルから再生し、許容添付サイズをcache予算で狭めない。local状態の確認はremote I/Oを起こさない。（R5-I で「SDKに同じhashの保護blobがあれば二重保存しない」判定は撤去した。）cacheにあるblobはhash指定の実QUICで別peerへ再提供できる。公開bucketのrecordは署名とscopeの検証後だけcacheへ確定し、SDK namespaceのimportや定常syncを始めず対象キーで再読込・再提供する。公開topic bucketでは、保持したrecordをkeyの一覧の要求にも合わせて提供する（#1395、ADR 0054 §4）。

成人向け表示設定がOFFの対象は、Rust側でbytes取得前に止める。ONの間に表示した成人向け添付は、表示用ファイルと同じ経路で `scope_key = 'adult'` の印を付けてcacheへ置き、OFFへ戻したら印の付いた非保護のblobを部分索引で選んで1回128件以内で消す（#1419、ADR 0046 §4）。remote投稿projectionや表示用ラベルの根拠が回収され、現在の対象から再検証できない添付は `None` として非表示にする。ラベル回収通知が届いた画面は表示済みURLも破棄する。既存の保護投稿に結び付くラベルと旧保存領域の移行は通常remote cacheとは別に扱う。

旧領域の保護データの移行（R5-G）: desktop runtime の背景 task（満杯のページが続く間は100ms、追いついたら60秒ごと。shutdownで停止）と backup 作成前の drain が、kind ごとの索引を1回128件以内の cursor で歩く。本人投稿（`envelopes` の rowid 順で署名者が本人の行）、bookmark、custom reaction bookmark、DM 履歴の添付、未送信 outbox（作成時刻順。frame を送信者として開いて暗号化添付の hash を得る）、live/game（反映時刻順。state が指す envelope の署名者が本人のときだけ）、avatar、private 参加状態（現 epoch の metadata・policy・自分の参加 record・自分宛 grant）、自作 Dome の pin tag。旧 SDK の blob は BLAKE3、record は content hash で照合してから、保護参照（`own:`・`bookmark:`・`dm_outbox:` など）を付けた cache 行へ写す（1MiB 超は file）。共有 hash は1行だけ持ち、参照が1つでも残る間は保護される。保護行は3GiBの計数と回収の対象外。bookmark・custom reaction bookmark の解除、DM の ACK と手元の削除で参照を外す。private の record は capability を持つ間、旧領域が無くても key 指定の読み出しで読める。旧領域を削除できるのは、R5-H の writer 切替を永続化した後に全 kind の `caught_up_at` がその時刻より後になったとき（ADR 0048 §7）。

公開blobの発見（#1632、[ADR 0063](../adr/0063-public-blob-dht-discovery.md)）: 検証済みの公開記録が参照するblobを `public_blob_refs` に記録ごとに置き、公開参照とこの台帳の返せる保持（保護か7日以内の利用）の両方があるhashだけを `public_blob_announcements` に512件まで載せてMainlineへ告知する（本人のblob、次に利用の新しいもの）。告知の確認は利用時刻を変えず、告知のための保護参照を足さない。3GiB・7日・128件の回収と保護の条件は変えない。既知の候補で取れない公開blobは、取得の中でDHTから保持端末を探し、hash別の取得元（上の256組）へ覚える。Webは同じ意味の公開参照をIndexedDBの `public_refs` に置き、既知の候補で取れない公開blobの保持端末を、検索を提供するCommunity Nodeへ頼む（ADR 0063 §8）。取得元は到達情報ごと覚える。Webは告知をしない。

`blob_objects` の永続状態表は読取り先が無かったため撤去した。添付の表示状態は現在のBlobServiceのlocal状態から求める。欠損していても投稿と操作を続け、表示中の本文・返信先・sessionの再試行は #1221 R3-B の最大4試行・5/30/120秒・需要消失時停止に従う。ネイティブ添付表示ではbase64 IPCとWebView側の全bytes展開を使わず、動画の取得は表示需要がある間だけ行う。128MiBにはアプリ管理の添付bytes（ファイル表示では0）・取得中作業領域を含め、WebView decoder/OS cacheは含めない。旧SDK保存領域の保護移行と物理退役はR5-G/Iで確認する。

本人の書込みの直接の保護（R5-I）: 本人の blob は書いたときに保護参照（既定は `own_blob:<hash>`、送信待ちは `dm_outbox:`、DM 履歴は `dm_message:`、pin は `dome_pin:`）を付けて保護所有先へ置き、本人の docs record は `own_docs` の参照で置く。保護移行の背景 task は旧領域の退役で止まり、以後は常駐しない。

# desktop の blob と remote 内容の保持

現行の受入条件と予算は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R1-C・R3-B/C・R5-A に従う。本書は保存先と取得経路の対応を示す。旧 #1207 時点の試行回数や無期限 cache の記録は現行仕様ではない。

| 所有者 | 内容と取得 | 保持・回収 |
| --- | --- | --- |
| Iroh SDK `blobs.db`（旧領域） | 本人が書いた本文・添付、pin された資産、切替前の旧内容 | R5-G の移行が本人データを保護所有先へ写す（下記）。backup には含めない。閉じた旧領域の退役は R5-I。旧領域を R5-A の容量達成のために一括削除しない |
| account SQLite の remote cache | 検証済み公開bucket record、remote blob、投稿projection・索引と表示用ラベル根拠。大きなremote blobは同じ台帳に紐づくファイル | 非保護分を合計1GiB、非利用7日、1処理128件以内で回収。取得中は1MiB単位の予約を計数し、容量不足では取得を延期する。bookmarkが参照する共有blobは参照が残る間保護する。ファイルの削除は台帳transaction確定後に行う |
| desktop の表示用ファイル・URL | viewport内の投稿・DM添付をファイルへ処理単位で転送し、Tauri asset URLで表示 | viewport外・画面離脱・gate切替・account切替で取得とURLを解放し、専用の一時ファイルを削除。起動時にも残存ファイルを削除する。取得中は最大8件・各16MiBを128MiBのアプリ管理メモリ予算へ計上。URLは最大256件 |

desktop の通常remote blob取得は一時bytesを返し、それだけでは保存しない。本文・添付・sessionのconsumerがaccount/参加世代・対象hash・取得gateを確認してから `put_remote_blob` でcacheへ書く。表示用添付は別のファイル経路で取得し、同じgateを再確認してから成人向け以外のremote blobをファイルのままcacheへ保存する。1GiBを超える添付はcacheへ入れず表示用ファイルから再生し、許容添付サイズをcache予算で狭めない。local状態の確認はremote I/Oを起こさない。SDKに同じhashの保護blobがあれば二重保存しない。cacheにあるblobはhash指定の実QUICで別peerへ再提供できる。公開bucketのrecordは署名とscopeの検証後だけcacheへ確定し、SDK namespaceのimportや定常syncを始めず対象キーで再読込・再提供する。

成人向け表示設定がOFFの対象は、Rust側でbytes取得前に止める。ONで取得した成人向け添付は一時bytesのまま扱う。remote投稿projectionや表示用ラベルの根拠が回収され、現在の対象から再検証できない添付は `None` として非表示にする。ラベル回収通知が届いた画面は表示済みURLも破棄する。既存の保護投稿に結び付くラベルと旧保存領域の移行は通常remote cacheとは別に扱う。

旧領域の保護データの移行（R5-G）: desktop runtime の背景 task（満杯のページが続く間は100ms、追いついたら60秒ごと。shutdownで停止）と backup 作成前の drain が、kind ごとの索引を1回128件以内の cursor で歩く。本人投稿（`envelopes` の rowid 順で署名者が本人の行）、bookmark、custom reaction bookmark、DM 履歴の添付、未送信 outbox（作成時刻順。frame を送信者として開いて暗号化添付の hash を得る）、live/game（反映時刻順。state が指す envelope の署名者が本人のときだけ）、avatar、private 参加状態（現 epoch の metadata・policy・自分の参加 record・自分宛 grant）、自作 Dome の pin tag。旧 SDK の blob は BLAKE3、record は content hash で照合してから、保護参照（`own:`・`bookmark:`・`dm_outbox:` など）を付けた cache 行へ写す（1MiB 超は file）。共有 hash は1行だけ持ち、参照が1つでも残る間は保護される。保護行は1GiBの計数と回収の対象外。bookmark・custom reaction bookmark の解除、DM の ACK と手元の削除で参照を外す。private の record は capability を持つ間、旧領域が無くても key 指定の読み出しで読める。旧領域を削除できるのは、R5-H の writer 切替を永続化した後に全 kind の `caught_up_at` がその時刻より後になったとき（ADR 0048 §7）。

`blob_objects` の永続状態表は読取り先が無かったため撤去した。添付の表示状態は現在のBlobServiceのlocal状態から求める。欠損していても投稿と操作を続け、表示中の本文・返信先・sessionの再試行は #1221 R3-B の最大4試行・5/30/120秒・需要消失時停止に従う。ネイティブ添付表示ではbase64 IPCとWebView側の全bytes展開を使わず、動画の取得は表示需要がある間だけ行う。128MiBにはアプリ管理の添付bytes・URL・取得中作業領域を含め、WebView decoder/OS cacheは含めない。旧SDK保存領域の保護移行と物理退役はR5-G/Iで確認する。

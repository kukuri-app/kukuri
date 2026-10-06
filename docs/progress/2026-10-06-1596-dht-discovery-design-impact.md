# 少数候補の再利用とDHT発見を組み合わせる場合の設計への影響

2026-10-06の影響調査。基準commitは`b025c6da07b893dc8eeb239dcdf0b09826bfd43a`。
対象は「手元のcacheと少数の既知端末を先に利用し、必要なblobのhashで未知の保持端末を探す」案。
generalを数千万ユーザーが購読しても、各端末が全参加者・全投稿を読まずに表示と取得を続けられることを基準とする。

この資料は実装から確認した事実と変更案を記録する。実装・fork・設定変更・新しい通信の追加は行っていない。
previewへ組み込んで利用状況を確認する進め方を前提とし、公開Mainlineでの未計測を導入停止の条件にはしない。
[#1596のPoC記録](2026-10-06-1596-public-blob-discovery-poc.md)の試験結果は再利用する。

2026-10-06のユーザー判断: Webから一般のnative端末への探索依頼・候補返却・代理取得の案は撤回する。
一般の参加端末は自分の利用のために取得し、自分が保持しているデータを他の参加端末へ送信する。
Web向け探索を設ける場合はCNが担当し、CNは探索結果と接続先情報だけを返す。CNにも代理取得は追加しない。
自分のcacheの送信と、他人の要求による新しい取得は、利用者に求める協力が異なるためである。

この調査から作成した導入設計案は[Issue #1632](https://github.com/kukuri-app/kukuri/issues/1632)で追跡する。
取得・告知・CN検索・Web接続の設計、6つの受入条件とPR対応、実装前の判断事項はIssueへ集約する。
本資料は基準commitに対する影響調査として残し、導入済みの記録には読み替えない。

## 調査の完了範囲

1. 取得の入口、候補選択、発見、接続、検証、保存、停止の変更点をpath/callerで示す。
2. cache追加・削除と保持告知、公開の判定、保存・backupへの影響を示す。
3. Web、既存の端末発見、CN、タイムラインへの影響を示す。
4. 端末ごとの処理量を増やさない構造、必要な検証、未決事項を示す。

コードと関係ADRの読取り、および3担当による並行調査を実施した。新しい実行試験は行っていない。

## 維持できる既存処理

| 対象 | 確認した実装 | DHT追加後の扱い |
| --- | --- | --- |
| 手元優先 | `blob-service/src/lib.rs:425,436,546,568`がSDK/remote cacheを先に読む | 手元にあれば探索も通信もしない |
| 少数候補 | `transport/src/peers.rs:403,525,552`。一般候補はsource各4件のcursor読取り、最終4件。hash別の取得元は全体256組・期限10分 | 最近成功した相手とhash別候補を再利用し、DHT結果も同じ表へ入れる。別候補選択器を作らない |
| 失敗の更新 | `peers.rs:476,488`。hash別の欠損抑止、一時失敗3秒、成功更新 | DHTから得た相手にも同じ結果記録を使う |
| 取得本体 | 通常取得`remote_fetch.rs:437,466`、表示`:50,64`、file`remote_fetch/display_file.rs:3,18`が同じ`fetch_bytes_from_remote(:682)`へ進む | DHT探索を共通の取得処理へ接続する。入口ごとの別実装は不要 |
| 転送・検証 | `remote_fetch.rs:316,682,775`のiroh-blobs取得と既存cache配信への切替。`remote_blob.rs:44,210`がcacheを返してhashを確認 | データの配信方式とhash検証を維持。DHTの候補は保持・閲覧許可の証明にしない |
| 取得の受付 | `work_admission.rs:18,27`、`network_work.rs:104,139`。端末のnodeごとに8実行、256要求、metadata 4MiB。受付から30秒 | 探索も同じ枠と元の期限へ計上し、枠数・期限を増やさない |
| 表示終了・account切替 | `remote_fetch.rs:55,74`、`network_work.rs:217,269`、`stack.rs:497,530,664`、`host/accounts.rs:159,200` | 表示終了はその要求だけを止める。account切替・node作り直しでは関連要求を全部止めてから通信を閉じる |
| 既存の端末発見 | CN topic rendezvous、manual ticket、gossipでの学習、EndpointIdの住所解決 | hashによる保持端末発見とは役目が違うため維持する |

pathは上表のcrateに対応する`crates/`以下。保存前のaccount/参加状態/成人表示等の確認はapp-api側の既存処理を維持する。

## 取得処理とDHTの組立てで必要な変更

### 発見の許可と、候補・時間の配分

共通の取得処理はhashを受け取り、公開/private/DMの由来を知らない。DMも同じ`BlobService::fetch_blob`を使う
（`app-api/src/service/attachment_support.rs:151,162`）。公開blobの発見を追加するなら、公開であることを確認した
呼出しから発見を許可する情報を渡すか、保存した公開参照をhashで確認する必要がある。

#1596の`probe`はtest専用で、「既知候補が失敗→残時間で発見→候補登録→次の既存再試行で転送」である。
通常のアプリの取得本体にはhash検索が入っていない。実装時の選択肢は次の2つ。

| 案 | 既存設計への変更・表示への影響 |
| --- | --- |
| 発見した候補を次回試行へ渡す | 既存の最大4試行・5/30/120秒の再試行へ接続する。発見しても次回まで待つため、最初の再試行では5秒待つ。残り試行がない場合の扱いを決める |
| 同じ要求の未使用枠で発見候補を試す | 同じ30秒内で取得できる可能性がある。最大4端末のうち何件・何秒を既知候補へ使うかを決める。既知4件を試した後に新たな4件を足さない |

どちらも取得本体を複製しない。探索要求を同じhashでまとめる場合も、1つの表示の取消で他の表示まで止めないことが必要。
転送の保存モード・byte制限・accountの区別は現在どおり維持する。

### DHTを共有するための追加APIと設定

`IrohDocsNode`は現在DHTを直接保持せず、`build_endpoint_builder`が住所解決部品をendpointへ追加する
（`iroh-node/src/node.rs:197,445`、`transport/src/iroh/relay.rs:58,77`）。
`iroh-mainline-address-lookup 0.6.0`の公開APIはDHTを内部で新規作成し、既存DHTの注入・取出しを提供しない。

同じDHTを共有する最小案は、`iroh-address-lookups`リポジトリ内のこの部品へDHTを渡すAPIを追加し、
nodeがDHTとindexの通信を管理すること。kukuriに住所解決を再実装する案より、既存の署名record処理を維持できる。
このAPI追加案では、取消・処理量を直す2リポジトリに加え、3つ目の小さなAPI修正が必要になる。

- `n0-mainline`: 結果の蓄積数、進行中の探索、送信・応答待ち、各呼出しの取消を修正する。
- `iroh-content-discovery`: index要求の取消・上限、候補streamの保持数、登録blobごとの常駐処理と全登録読取りを修正する。
- `iroh-address-lookups`: 既存のDHTを住所解決へ渡す方法を追加する。

同じ`n0-mainline`は既存のEndpointId住所解決にも使われるので、fork差替えの検証はblob発見だけでは足りない。
iroh本体・iroh-blobs・既存forkの公開型/sourceは維持する。

現在は`static_peer`でDHT無効、`seeded_dht`で有効。さらにrelayとCN bootstrapがある場合にもDHTを無効化する
（`desktop-runtime/src/runtime/mod.rs:698`、`stack.rs:320`）。発見結果がEndpointIdだけでも、住所解決手段がなければ
接続できない。hash発見と住所解決を既存設定へ連動させるか、別の設定で制御するかを決める必要がある。
PoCが試験中に住所解決を追加したことを、その設定対応の実装済み証拠にはしない。

## cache保持と自動告知で必要な変更

### 公開の由来を保存する索引

現在のblob保存はhashごと1行で、通常の`scope = blob`と成人向けの`adult`はあるが、公開/private/DMの区別はない
（`store/src/sqlite/remote_cache.rs:426,503,852`、`web-runtime/src/content_cache/tx.rs:36,485`）。
`own_blob:<hash>`も公開の証明ではない。DM添付は復号したbytesを`dm_message:`で保存する
（`app-api/src/service/attachment_support.rs:170,179`）。現行のcache配信はhashを受けて読むだけである。
現在もhashを指定した相手へ平文添付を返せるが、全blobをDHT告知すると、そのhashから保持端末を検索できるようになる。
この事実から、一律に全cacheを公開告知する方法は、公開blobだけを扱う追加機能には使えない。
IP秘匿の新しい条件を追加するのではなく、公開投稿とその他の保存物を実装上区別する変更が必要ということ。

公開の材料は検証済み投稿にある。公開channelと本文/添付hashをprojection更新時にhashの逆引き索引へ残す案が適する
（`app-api/src/service/post_integrity.rs:113`、`object_persistence_support.rs:647`、
`store/src/sqlite/projections.rs:200`、`web-runtime/src/account/mod.rs:145`）。
現状は本文hashの逆引きとJSON添付hashの索引がないため、公開参照の別保存とprojectionの派生索引を一つにまとめる。
blob保存呼出しだけに印を付ける方法では、既に手元にある同じhashが後から公開投稿に結び付いた場合を扱えない。

告知用の印を既存の保護参照へ混ぜると、公開cache全体が容量計数・期限・成人OFFでの回収から外れる
（`remote_cache.rs:737,756,765`）。告知の資格と、データを消さずに保護する参照は分ける。

### 追加・削除の通知と告知更新

`ContentCacheStore`には一般的なblob追加・削除通知がない（`store/src/cache.rs:72,77`）。
SQLiteの保存・`delete_cache_item`とcommit、Webの`place`・`drop_item`・`Tx::commit`へ変更を集め、
保存が確定したものだけを告知処理へ渡す。容量回収、成人表示変更、保護解除による削除も同じ変更記録を使う。

告知は少数の常駐処理が、次回の更新時刻と差分位置を索引で選んで順番に実行する。全cache読取りや1blob1常駐処理は使わない。
更新時には利用時刻を変更しない`has_remote_content`等で保持を確認する。`remote_content_len`は利用時刻を更新するため、
定期告知に使うと未閲覧blobの7日回収を延ばす。7日の失効では物理削除前に読み出せなくなるので、
削除イベントだけを更新停止の根拠にしない（`remote_cache.rs:545,676`）。
保持がなくなったblobの更新を止め、既にDHTへ出た告知は期限で失効する。即時に全DHT記録を消せるとは扱わない。

### 保存・backup

公開参照と更新時刻/差分位置を永続化する案では、SQLite migration・索引・schema fixtureが変わる。
Webにも同じ公開参照を持たせる場合はIndexedDBの更新が必要。現在のversion 1のcallbackは全storeの新規作成なので、
既存storeを残す更新処理を加える（`web-runtime/src/content_cache.rs:48,228`、`idb.rs:165`）。
backupはSQLite全体を含むため、新しい表も入る。復元後は古い「告知済み」の状態を実ネットワークの状態として信頼せず、
再提供可能な保存物だけを有限件ずつ選ぶ。バックアップファイル形式の変更が必須とは現時点で判断していない。

## Web・補助index・CNへの影響

WebはIndexedDBに保持し、既知の相手からWebRTCまたはrelay経由で取得・再提供できる。cache保持とDHT発見は別の役目。
Webは直接UDPを使えないためMainline DHTを実行できない（ADR0056:85、`desktop-runtime/src/stack.rs:787`）。
`RemoteBlobProtocol::serve`はcacheに無いと欠損を返すだけで、探索依頼・代理取得・候補返却を持たない
（`iroh-node/src/remote_blob.rs:44,54`）。既存docs-readの要求も対象keyの読取りだけ。

Webから一般のnative端末への探索依頼・候補返却・代理取得は、ユーザー判断により採用しない。
既存のcache配信は保持分を返し、保持していない場合は現在どおり欠損を返す。

Web向け探索を設ける場合は、次の役割分担とする。探索要求と結果の通信は現在未実装で、追加が必要。

| 担当 | 処理 |
| --- | --- |
| Web | 必要な公開blobのhashについてCNへ探索を依頼し、得た接続先から既存の通信で取得する |
| CNの探索機能 | DHTと補助indexで保持端末を探し、EndpointId・relay等の到達情報を少数返す。Webに渡すためのblob取得・保存・転送は行わない |
| 保持する参加端末 | 自分が保持する内容を既存の配信方法で返す。他端末の要求を理由に、未保持の内容を別端末から取得しない |

CNへの新しい探索要求では、同時数・結果数・期限・取消を管理し、同じhashの結果を短期間再利用する。
既存`remote-blob/1`と、その欠損応答は維持できる。未対応の旧端末は現在の取得を続ける。
WebRTCの初回接続・交渉には既存のrelay到達情報を使うため、DHTのEndpointIdだけを返す設計ではWebに不十分な場合がある。
Webから使える接続先情報を返す方法と、使えない候補の扱いは未決事項として残す。
この探索機能によってCNがblobの取得元や保存先になる設計にはしない。

CN rendezvousは同じtopicの参加端末を知る補助機能、DHTのEndpointId住所解決は接続先を知る機能、
追加indexはMainline上のUDP送信元アドレスを署名付きEndpointIdへ変換する機能で、別々の役目。
`cn-iroh-relay`は既存のrelay処理を維持し、blob保有者表や探索依頼を追加しない。

固定discovery版のindex結果cacheは4,096件、有効期限つきで再利用可能だが、これはsocket→EndpointIdのcacheであり
hash→保持端末の表とは違う。既定の署名付きindex一覧は最大2サーバー、index実装の既定保持は200万行である。
数千万の登録・検索がそのまま同じ2台へ集まる設計にはしない。接続先の分散・変更方法と運用設定を決める必要があり、
Mainlineを使うだけで補助indexも自動的に分散するとは扱わない。補助indexはblobを保存・配信するサービスではない。

## generalの人数増加に対して、改善する部分と残る部分

保持端末の探索では、全topic参加者一覧を各clientへ持たせず、必要なhashで少数相手を見つけられる。
既存候補の再利用、同じhashの探索結果の短期再利用、同時数/保持数/期限の制限と両立する。
告知側も差分と期限索引で動かせば、1端末のcache件数を全件読んだり、登録件数だけ常駐処理を増やしたりしない。

ただし、現在のタイムラインはページ索引から見出し・hashを先に知る。blobのDHT検索は、未知の投稿のhashや
タイムラインの索引を発見する機能ではない（`app-api/src/service/remote_read_support.rs:233,760`、ADR0054）。
このページ取得・投稿通知の方法は維持するが、DHTによって自動的に処理量が減るとは扱わない。

投稿通知のapp処理は30秒32件と、溢れた場合の有限ページ読取りに制限されている。一方、transportでの受信ごとの
JSON解析・gossip検証/転送・一部の短命な通知は到着率に比例することがADR0052に明記されている
（`docs/adr/0052-scale-independent-timeline-sync.md:183`、`transport/src/iroh/topics.rs:686`）。
数千万購読で端末の体験を維持するという全体目標には、この受信方式の扱いも別に決める必要がある。
本調査ではその修正へ着手せず、DHTが解決する保持場所の探索と、解決しない通知/ページ取得を区別した。

## 実装前に決める事項と検証

| 決める事項 | 影響 |
| --- | --- |
| 4候補・30秒を既知取得と発見にどう配分するか | 同じ要求で取得するか次回まで待つか、最後の再試行後の結果が変わる |
| static_peer/seeded_dht/CN利用時の有効化 | 既存設定とCLIの意味、外部通信の開始条件が変わる |
| 公開投稿の本文・添付以外にavatar/session/pin等を告知するか | 公開の根拠と新しい索引の対象が変わる。既存IP秘匿の新条件を設ける論点ではない |
| 既存の保存物を告知対象へ移す方法 | 初回の全cache走査は避け、公開参照から再開可能な小分け処理を設計する |
| 取り下げ・成人OFF・公開projection回収後の告知 | 保持は継続する場合もあるため、保持と告知更新を止める条件を区別する |
| CNの探索要求・接続先情報と、補助indexの接続先 | 一般nativeへの探索依頼と代理取得は除外済み。CNの探索負荷・Webからの到達・配布順を決める |

必要な検証は、既存候補/失効（`transport/src/peers/tests.rs`）、共通受付/取消/期限（`iroh-node/src/network_work/tests.rs`）、
SDK/cache-only往復（`remote_fetch/tests/provider_discovery.rs`）、DHT住所解決と再起動（`desktop-runtime/src/tests/seeded_dht.rs`）、
保存/回収/rollback（`store/src/sqlite/remote_cache/tests.rs`）、Webのreload/chunk/容量（`web-runtime/src/browser_tests.rs`）、
account切替（`desktop-runtime/src/tests/host_generation.rs`）を拡張する。
Webを含む案では、Web→CNの探索依頼と候補返却の実ブラウザ往復、返された相手へのrelay/WebRTC接続を追加確認する。
CNがblobを取得・保存せず、一般nativeが未保持blobを他端末から取り寄せないことも確認する。
既存の`web-runtime/src/browser_tests.rs:250,255`と`apps/desktop/tests/web-e2e/main-flow.mjs:421`の経路判定を再利用できるが、
新しい探索要求が実装済みとは扱わない。
forkには、取消後の送信/保持ゼロ、処理待ち上限、同じ探索を利用する他の呼出しが残る場合の継続、期限と容量の確認を追加する。

利用者へ見える待ち・設定の意味・告知対象・通信要求の変更を決めてから実装する。
ネットワーク全体での効果はpreviewで観測する項目として、取得成功率・待ち時間・探索/告知量・実転送経路を記録する。

# CN応急復旧後の取り込み遅延調査

調査日: 2026-09-20。原因調査後、同日のユーザー指定に基づき「ピア状態管理と投稿取得scheduler」を軸に対策案を改訂。
本番設定・実装は変更していない。
先行する障害調査・再起動操作は [索引停止の調査](2026-09-20-cn-index-availability-investigation.md) に記録。

## 結論

初回巡回の約60.6分の主因は、本文・メディアblob取得における**直列の接続／転送タイムアウト待ち**。
接続後の成功転送は速く、データの転送量より、成功する取得先に到達する前の待ちが支配的だった。
最初の9/18の約40時間の停止原因がこれと同一であることまでは証明していない。

## 対象と集計方法

- 対象container: `community-node-cn-indexer-1`。
- 本番revision: `4b7519460e8baf3c29d7ec0eadc6eb2c727a1065`。
- 初回巡回: 12:15:23.051922 ～ 13:15:56.332860 JST。`last_pass_duration_ms=3633280`。
- `docker logs --since 2026-09-20T03:15:21Z --until 2026-09-20T04:15:57Z` を読み、ANSI escapeを除去。
- `kukuri_iroh_node::remote_fetch` の1187行を、メッセージ・時刻・hash・peerで対応付けた。
- timeoutログの終了時刻から`timeout_ms`を引いた区間の和集合を集計。この区間群に重複はなかった。
- 成功転送時間は同じhash/peerの直前の接続成功から転送成功まで。接続確立前の時間を含まない。
- 生ログは端末の一時領域で集計し、この文書には本文・hash・peer ID・IPを含めない。peerはID昇順にP1～P4へ匿名化。

## 実測

| 項目 | 回数 | 時間 |
| --- | ---: | ---: |
| 初回全件巡回 | 1 | 3633.280秒（60.55分） |
| 接続timeout | 200 | 5秒 × 200 = 1000秒 |
| ephemeral転送timeout | 167 | 15秒 × 167 = 2505秒 |
| 上記timeout区間の和集合 | 367 | 3505秒（58.42分、巡回時間の96.47%） |
| 成功したephemeral転送の接続後時間 | 68 | 合計0.971秒、中央値7.44ms、最大42.87ms |

timeoutはすべて`subject="bounded scan blob"`。このラベルは本文blobとメディアの両方で使われ、
メディア専用ではない。docs entryのremote取得開始は2回で、terminal記録まで合計約0.686秒。

bounded取得の開始は89回、転送成功68回、候補を使い切った失敗7回、terminalログなし14回。
最後の14回は外側で中断された取得と整合し、実測の`media_fetch_timeout=14`とも一致する。
terminalのない取得を成功または0秒として集計していない。
成功／候補枯渇まで追跡できた75回のbounded取得は、開始から終了まで中央値50.06秒、最大100.08秒。

| 匿名peer | 接続timeout | 転送timeout | ephemeral取得成功 |
| --- | ---: | ---: | ---: |
| P1 | 115 | 47 | 14 |
| P2 | 0 | 120 | 2 |
| P3 | 83 | 0 | 15 |
| P4 | 2 | 0 | 37 |

P2はQUIC接続が成功しても転送が15秒待ちになったケースが多い。
P4は成功68回中37回を担うが、順序上は後方に置かれていた。
P1～P3にも成功があり、単純に特定peerを削除すればよいという結果ではない。
別途、転送失敗62回で`io: stream reset by peer: error 3`を確認した。
接続先の応答遅延・blob未保持・相手実装／接続状態の問題のどれに起因するかは、相手側の証跡がなく未確定。

実例では、一つのblobでP1の3候補を各15秒、P2の2候補を各15秒待ち、
約77.19秒後にP3へ接続。そこから約34msで取得が完了した。
IP候補／relay URL付き候補のログは候補指定を示すものであり、実際にrelay経由で転送した証拠とは扱わない。

## 実装上の増幅要因

以下の主要取得処理・ingest処理は、本番revisionと調査時HEAD `b302f942` で差分がないことを確認した。

1. **peerと候補の二重の直列探索**
   - `crates/iroh-node/src/remote_fetch.rs:199` で全peer、`:207` / `:217` でpeer→候補を順次探索。
   - 接続5秒、転送15秒は候補ごとの上限。取得全体の上限ではない。
   - 接続成功後の転送timeoutでも、同じpeerの次候補から再試行する。
2. **成功実績が次の取得順に反映されない**
   - `crates/transport/src/peers.rs:98` はlearned→seed→importedの各BTreeMap順。
   - `connect_candidates`（同`:183`）はdirect→remote_info→relay付き等の候補を生成。
   - fetch成功時に高速な供給元へ優先順位を変更する処理はない。
   - retry stateはhash単位の3秒cooldown（同`:26`, `:40`）。peerの遅さを他hashの取得へ引き継がず、成功後はcooldownを消す。
3. **本文取得に全体期限がなく、検査結果の再利用より先に取得する**
   - `crates/cn-indexer/src/ingest/source.rs:170` の本文取得はbyte上限付きだが、全体時間上限なし。
   - `crates/cn-indexer/src/ingest.rs:569` の本文解決後に`:594`のscan/reuseへ進む。
   - 初回巡回で41回の判定再利用があっても、本文取得待ちは省略できない。
   - メディア取得には外側30秒のtimeoutがある（`media_fetcher.rs:78`）。本文とは待ち時間の境界が異なる。
   - raw blobを永続保存しない現行境界により、成功した本文でも次回巡回でremote取得が必要になり得る。
4. **投稿・scopeが直列で、遅い取得が後続を止める**
   - `ingest.rs:457` の投稿loopと `worker.rs:327` のscope loopはいずれも順次await。
   - general_ja=約31秒、dev=約10秒、test=約25秒に対し、general（64レコード）は約59分27秒。
   - full_pass完了前にはイベント処理loopへ入らない。起動中に届いた変更通知の処理も後ろに回る。
5. **API再開判定と進捗観測が全巡回完了に依存**
   - `worker.rs:334` で全scope成功後にlast_sync_at、`:363`でscope終了後にlast_ingest_atを更新。
   - DBで1件ずつ索引が増えていても、statusのscanned/indexedや同期時刻はscope完了まで止まって見える。
   - readinessはlast_sync_atとlast_ingest_atの両方が900秒以内であることを要求する。
   - 初回巡回が約60分のため、実際に取り込めていた投稿もAPI再開まで待たされた。

## 改訂対策: ピア状態管理と投稿取得scheduler（#1212）

2026-09-20のユーザー指定を受け、対策を以下の2機能として構成する。本節が現行方針であり、
後掲の初期案は経緯として残す。実装はIssue #1212で進め、本番適用は別途production rolloutに従う。

### 目的・責務境界

復旧時に成功する取得先へ早く到達し、取得不能な投稿が他の投稿の取り込みを止めない状態を作る。
CNの取り込み経路を最初の利用者とし、既存の共通peer台帳を拡張する。新しいcrateの追加は前提にしない。

| 機能 | 所有する判断・状態 | 連携先 |
| --- | --- | --- |
| ピア状態管理 | 接続状態、取得実績、受信リクエスト頻度・rate limit、取得先候補とpeer単位の利用枠 | transport/HTTP/relay adapterが観測を渡し、remote fetchが取得先と利用枠を問い合わせる |
| 投稿取得scheduler | 投稿の取得状態、queue、優先順位、再試行、並列実行、復旧checkpoint | 取得処理を起動し、ピア状態管理を利用。既存ingest pipelineへ検証済み結果を渡す |

署名・scope・撤回・送信防止・moderation判定・索引更新は既存pipelineが引き続き担当する。
schedulerの「取得成功」は「公開許可」ではない。HTTP/relayの受信要求は投稿queueを経由せず、
それぞれの入口からピア状態管理の受付判定を使う。

### 1. ピア状態管理

#### 管理する状態と更新点

| 状態 | 保持する情報 | 更新・維持方法 |
| --- | --- | --- |
| 接続状態 | endpoint ID、protocol、アドレス候補、接続世代、接続試行中/接続中/切断/状態不明、観測時刻、実際に観測した通信経路 | endpointの接続・切断通知と定期照合で更新。古い世代の切断通知で新しい接続を上書きしない。期限切れは不明へ戻す |
| 取得成功実績 | peer/protocolごとの直近成功・失敗、接続/転送時間、連続失敗、実行中件数、再試行可能時刻。短期のhash別供給実績 | remote fetchの接続結果と検証済み転送結果から更新。成功・timeout・not-found・拒否・cancelを区別し、時間経過で実績を減衰させる |
| 受信要求頻度 | 主体・protocol/操作別の受信/許可/拒否件数、時間窓またはtoken、burst、次回受付可能時刻。転送量制限はbyte単位の別枠 | 入り口で原子的に受付判定し、実行前にquotaを消費。拒否も観測する。idle状態を期限・件数上限で回収 |

QUIC接続成功だけではblob取得可能としない。接続状態と取得実績は独立に持つ。
blob未保持をpeer全体の通信障害と決めつけず、hash別の短期情報として扱う。
取得成功率は接続先選択のための運用指標であり、social trustやコンテンツの安全性評価へ混ぜない。

#### 取得先選択と利用枠

- 現在の接続状態と最近の取得実績から候補を選ぶ。失敗の多いpeerは一時的に後回しにし、
  期限後に少数の再試行を許して回復を検出する。未知peerにも有限の試行機会を残す。
- 同じpeerの候補を何度も待つ前に、他の有望なpeerを試す。必要なら少数の候補を並行実行するが、
  投稿schedulerと別々に無制限の並列度を持たず、共有する全体・peer別利用枠を必ず消費する。
- `Direct P2P → Relay Supported P2P → Relay Fallback` の優先度を維持する。
  relay URL付き候補の存在と、実データがrelayを通った観測を区別する。
- 受信rate limitと、自分からpeerへ取得する際の並列数・送信予算は別枠にする。
  同じ管理機能で扱っても、受信過多によって相手からの取得成功実績を書き換えない。

#### rate limitの集約と状態の寿命

現行の `cn-user-api/src/rate_limit.rs` はIP単位のHTTP要求制限、
`cn-iroh-relay/src/lib.rs` はupstreamの`ClientRateLimit`によるbyte制限を使用している。
主体・単位が異なるため、単一のpeer IDやカウンターへ統合しない。

- 共通の受付・観測interfaceで管理を寄せ、キーはP2Pの検証済みendpoint ID、HTTPの送信元IP、
  必要な認証後主体などを型で区別する。IPからP2P identityを推測しない。
- HTTPの未認証入口の制限とtrusted proxy設定を維持する。P2Pではprotocol要求の受付前に制限する。
- relayは純粋なiroh relayを維持する。upstream limiterをadapter経由の実行主体として扱い、
  同じquotaを二重計上する独自limiterは重ねない。観測hookがない部分は未観測とし、
  必要なhookの追加・対応可否を最初のcontract作業で確定する。
- 一つのruntime内で台帳と利用枠を共有する。別processのmemoryを共有状態と見なさない。
  rate policyにはprocess単位／node単位の適用範囲を明記し、node全体のquotaを要求する経路は原子的な共有backendを使う。
- 接続・実行中利用枠は再起動で状態不明／空へ戻す。取得実績は時刻・構成世代付きの小さなcheckpointで復元し、
  古い成功を現在の接続成功とは扱わない。受信quotaの復元・失効は各policyの時間窓に従い、
  再起動による制限回避を許さない設計にする。保存項目・保持期間はデータ分類へ反映する。

### 2. 投稿取得scheduler

#### 投稿取得状態の更新・維持

jobの単位は `scope + object_id + source revision` とする。取得すべき本文・添付・manifest参照を
そのjobの依存対象として扱い、投稿の公開可否は既存pipelineの結果で決める。
policy/scan構成世代の変更も再評価理由として保持する。

| 状態 | 意味・主な遷移 |
| --- | --- |
| queued | 起動時reconcile、変更通知、定期照合から登録。利用枠確保後にfetchingへ |
| fetching | lease/試行ID・期限・利用peerを持って取得中。完了時はfetched、過渡失敗はretry_waitへ |
| fetched | hash・サイズ等を検証済みの一時データを保持。既存pipelineへ渡す。これだけでは索引成功にしない |
| processing | 参照再確認・moderation・索引更新中。結果に応じてcompleted、retry_wait、suppressedへ |
| retry_wait | 失敗理由、試行回数、次回実行時刻を保持。期限や新たな到達情報に応じて再queue |
| completed | 当該source/policy世代の処理完了。参照・policy変更で新世代をqueue |
| suppressed / cancelled | 非allow等による公開抑止、撤回・scope解除・旧世代の失効。原因に対応する変更だけで再評価 |

- queue・試行回数・次回実行・source世代・完了checkpointを永続化する。CNでは既存Postgresを候補とし、
  接続先実績と同様、本文・media raw bytesは保存しない。
- 再起動時に期限切れleaseを回収し、fetching/fetched/processingは参照を再確認して再queueする。
  memoryから失われたbytesを取得済みと誤認しない。Postgres truthとprojection間の途中失敗は既存の冪等reconcileで回収する。
- 起動巡回・変更通知・定期巡回は同じqueueへ投入し、同じjobを重複実行しない。
  新世代が届いたら旧結果の反映を世代/lease照合で拒否する。新旧世代の同一投稿の索引書込みは直列化する。
- 遅い投稿をretry_waitへ移して他の投稿を進める。timeoutを非allowの確定判定やallowに変換せず、
  既存の過渡障害時のentry保持・確定した撤回等の削除規則を守る。
- 完了済み・変更なしのjobを起動ごとに無条件再取得しない。再利用には現在のsource参照・scope・撤回・送信防止・
  policy世代との照合を必要とする。索引済みtextだけを根拠に公開を再許可しない。

#### 並列実行と資源上限

- 投稿間を上限付きworker poolで並列化する。scope/投稿者間に公平性を持たせ、復旧queueと新規通知のどちらも飢餓状態にしない。
- 投稿全体、本文、添付の取得に期限を設ける。期限内に取得できない依存対象は状態と理由を残して再試行する。
- 投稿数だけでなく、全体/peer別の取得数、bytes in flight、検査providerの同時実行数・送信枠を制限する。
  queue満杯時は未投入対象のcursor/再照合要求を残し、通知を黙って破棄しない。
- 同一hashの同時取得は、許可scope・用途・byte上限が互換な要求に限って共有する。
  cancelした一利用者が他jobの取得を止めないよう利用者数と寿命を管理し、最後の利用者終了で一時bytesを解放する。
- 実行枠は成功・失敗・timeout・cancel・panic回収で確実に返す。network await中に管理台帳のlockを保持しない。

### 連携と復旧時の流れ

1. ピア状態管理が接続観測・有効な取得実績を復元し、schedulerがcheckpointと現在のsupported setを照合する。
2. 起動時の未完了jobと新規通知を同じqueueへ登録する。全件取り込み終了まで通知処理を待たせない。
3. schedulerが投稿の枠を確保し、remote fetchがピア状態管理から候補とpeer利用枠を得る。
4. 接続・転送結果をピア状態管理へ返し、投稿jobの状態と進捗を更新する。遅いjobは期限付き再試行へ移す。
5. 既存pipelineの検証・索引反映を終えたjobだけをcompletedにし、次のjobを進める。

readiness向けにはworker生存、投稿の実進捗、scope照合の完了、最古の未処理jobを別々に公開する。
投稿完了で更新する時刻と全巡回完了時刻を区別し、heartbeatだけで鮮度を更新しない。
API再開条件の変更はこのstateに基づくADR/contractで定義し、単なる900秒閾値の緩和で代替しない。

### 実装順序・受入条件

| ID | 作業・主な対象path | 受入条件と検証 | 依存 |
| --- | --- | --- | --- |
| T1 | interface・主体/寿命・状態遷移・rate limit入口inventoryを固定。`docs/adr/`、データ分類、既存`transport/peers.rs`・HTTP/relay入口を照合 | P2P接続/取得、HTTP認証前後、relay byte制限の主体・判定点・状態ownerに未分類がない。期限・queue上限・quota単位とreadiness条件をcontract化 | なし |
| T2 | ピア状態管理とadapter。`crates/transport/`、`crates/iroh-node/`、`crates/blob-service/`、`crates/docs-sync/`、`crates/cn-user-api/`、`crates/cn-iroh-relay/` | 古い切断通知の隔離、検証済み取得実績、失敗減衰・回復probe、同時受付の原子性、quotaの拒否前副作用0、再起動/期限/回収を検証。既存rate設定の意味を維持 | T1 |
| T3 | 投稿取得state・永続queue・schedulerを導入し既存worker/ingestへ接続。`crates/cn-indexer/`、`crates/cn-core/`のschema/migration | 遅い1投稿に他投稿が阻害されない、並列数/メモリ上限、重複通知、旧世代完了、cancel、再起動、truth/projection途中失敗を検証。withdrawal等のguardとscan再利用も回帰確認 | T1、T2 |
| T4 | status/readiness・運用手順と復旧scenario。`crates/cn-cli/`、`crates/cn-indexer/`、`harness/scenarios/`、関連runbook | 先頭peer無応答/後方peer即時供給を再現し、健全投稿の初回索引遅延と復旧所要時間が固定予算内。進捗停止を検出し、2巡以上・再起動後も継続。安全性/通信経路の境界を維持 | T2、T3 |

主要な完了条件は、指定された5責務（接続状態・取得実績・受信要求頻度・投稿取得状態・並列化）に
それぞれ一つのownerと更新経路があり、失敗peer/投稿の待ちを健全投稿から分離できること。
初期並列度や期限の具体値は、T1で4peer・複数scope・80レコードの再現fixtureと資源予算を基に固定する。
本番の到達可能性に依存するため、この案の時点で「何分で全件復旧」とは約束しない。

実装時は `REFACTORING.md` のpath別validationに従い、`cargo xtask rust-test`、
`cargo xtask cn-check` / `cargo xtask cn-test`、変更するconnectivity/復旧scenarioをローカルで実行する。
認証・取得・公開境界を含む実装のリスク分類と必要な独立監査はissue lifecycleに従って記録する。
現時点の未決事項はrelayの観測hook、共有quotaのbackend、保存期限、実行予算の具体値であり、T1の成果物へまとめる。

### #1212 実装結果

- `kukuri-transport::PeerAddrBook` は接続世代・接続状態、取得成功/失敗、連続失敗、平滑化した
  転送時間、peer単位のbackoffを保持する。接続状態は5分、成功優先は10分で失効し、古い状態を
  永続的な事実として扱わない。取得候補は接続状態・成功実績・backoffから順位付けする。
- request frequency ledgerはHTTP IP / P2P endpoint / relay clientを型で分離する。remote fetchは
  endpoint単位の要求枠を使用する。HTTPはtower-governor、relay ingress byteはupstream limiterを
  enforcement adapterとして継続し、共通の`RequestRatePolicy`へ変換する。
- remote fetchはpeer/candidate個別の5秒/15秒timeoutに加え、blob取得全体を30秒で終了する。
  budget超過は`Ok(None)`の一時的な取得不能として既存のfail-closed処理へ渡る。
- `PostFetchScheduler` はsource revisionごとのleaseとqueued/fetching/processing/retry_wait/completed/
  suppressed/cancelledを保持し、stale leaseの完了を拒否する。状態はraw bytesを含まず、再起動後は
  authoritative replicaの全件照合から再構築する。
- scope内の投稿は`COMMUNITY_NODE_INDEXER_MAX_CONCURRENT_POSTS`（既定4）を上限に連続的に補充する
  worker setで処理する。遅いjobの完了を待たず、空いた枠へ次の投稿を入れる。
- scheduler leaseは既存`ReferenceGuard`の一部として、scan/reuseとindex mutation前の再確認に入る。
  source revisionが更新された旧jobは一時失敗として既存entryを保持し、新jobが再評価する。
- `/v1/status`へ`post_scheduler`集計を追加した。既存readiness JSON readerは`serde(default)`で
  旧indexerとの互換を維持する。

実装時に具体化した値は、投稿並列4、blob取得全体30秒、P2P endpointへの取得要求16回/秒、
失敗backoff 2～60秒、接続状態TTL 5分、成功優先TTL 10分。いずれも環境依存の本文やpeer addressを
保存・公開せず、取得不能をallowへ変換しない。

## 初期の優先順位案（履歴・上の改訂案で置換）

1. **取得先探索の無駄な待ちを減らす**: 成功した供給元の実績を利用し、遅いpeerへの短期backoffを別hashにも活かす。
   小さな上限付き並行探索を検討し、同じpeerの候補を何度も待ってから次peerへ進む構造を見直す。
   Direct P2P → Relay Supported P2P → Relay Fallbackの優先度、hash検証・byte上限を維持する。
   全peerを一律短timeoutで切る方法だけでは、今回成功していた供給元まで到達できず欠落を増やし得る。
2. **1件の遅延で復旧全体を止めない**: 本文／投稿の処理予算と小さな並列度、遅い対象の再試行queue、
   投稿単位の進捗・checkpointを検討する。時間切れをallowへ変換しない。withdrawal、scope解除、参照変更、
   transmission preventionの再確認と既存entryの扱いを維持する。
3. **復旧判定・観測を改善**: worker生存、実取り込み進捗、各scopeの巡回完了を分けて観測する。
   全巡回が長いだけで全APIを閉じる問題は別途契約を整理する。進捗がないのにheartbeatだけで鮮度を偽装しない。
4. **再巡回コストを下げる**: 変更のない対象の再確認方法、判定再利用前の本文取得、manifest変更の全scope fallbackを検討する。
   raw blobの恒久cacheや古いprojectionだけでの無条件再公開は、この調査から直接採用しない。

最初に固定したい再現条件は「先頭peerは接続成功後に転送無応答、後方peerは即時供給」と、
「一つの遅い投稿と複数の健全な投稿が混在」。健全な投稿の索引までの時間、全巡回時間、取得試行数を測り、
既存の検証・公開境界を変えずに短縮できることを確認する。

## 制約と現在との関係

- 原因の数値は保存した本番ログの集計とコード照合による。#1212の実装検証ではlocal contractと
  connectivity scenarioを使用し、新しい本番投稿・再起動・rolloutは行っていない。
- 13:21:49 JSTのstatusでは初回巡回成功後のイベント処理中で、全scope fallback理由は`manifests/media`。
  次の全巡回完走や継続可用性は、この調査の結果で保証しない。
- HEADの#1209はイベント処理前のseed更新を追加するが、上記の直列取得・本文期限・再取得順序を変更しない。
  最新revisionへの更新だけでこの約60分を解消する根拠はない。
- 推奨対策の効果時間は未測定。成功転送が速いことは大きな短縮余地を示すが、「全復旧を1秒にできる」という意味ではない。

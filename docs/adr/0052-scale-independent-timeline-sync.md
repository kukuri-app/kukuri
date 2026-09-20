# ADR 0052: タイムラインの反映と復旧を総件数に依存させない

## Status

Accepted

## Context

利用者が 10 名程度の段階で、CPU・メモリ・ネットワークを使い切る現象が複数人から報告された（Issue #1221）。調査の結果、待ちなしの空転は無く、
処理量が「件数」と「失敗の数」に比例して増える経路が複数あり、互いを起動し合っていた。blob の再取得は #1207、全件走査の頻度は #1225 で下げたが、
総件数に比例する構造は残った（Issue #1239）。

`AGENTS.md` の「設計原則: 件数に依存しない処理」は、次を前提にする。

- 不特定多数が参加する P2P SNS で、取りこぼしゼロの全件取得は不可能であり、目標にしない。
- 「N 件までは足りる」で判断しない。総件数に比例する経路は、頻度を下げても欠陥として扱う。
- 利用者の操作と表示の経路に、件数に比例する待ちを置かない。

現行実装（`117af820`）で確認した事実は次のとおり。

1. docs（iroh-docs）の query は、`docs-sync` が既定の並び順（`SortBy::AuthorKey`）で組んでいたため、key を 1 つ指定した読み出しも含めて namespace 全体の
   table scan になっていた。1 つの replica の entry 数を 1,000 → 10,000 にすると、key 指定の読み出しが 2.4 ms → 24 ms になる（計測 test
   `measure_exact_query_cost`）。doc event ごとの個別反映も、利用者の操作ごとの読み出しも、この上に乗っていた。
2. 反映の保険として、replica の 5 つの prefix の全 entry を読み込む全件走査が、購読タスクの起動時、recovery tick、購読の再起動、hint、利用者の操作ごと、
   空ページの表示で走る。profile は author replica の全投稿をロードしてソートする。view の生成中にも、行ごとに `withdrawals/` の全件を読む。
3. 投稿の保存時に、時系列で並ぶ索引 entry（`indexes/timeline/…`・`indexes/thread/<root>/…`）を docs へ書いているが、読む経路が無い。
4. docs の event は容量 256 の broadcast で配られ、受信側は取りこぼし（`Lagged`）を黙って捨てる。全件走査は、この取りこぼしを全件比較で埋める前提だった。
5. iroh-docs の同期は replica 全体の集合の突き合わせで、range の fingerprint を範囲内の全 entry から計算する。同期の開始ごと、相手ごとに、
   双方で replica の総 entry 数に比例する。新規の参加者は topic の全履歴の entry を同期し、client の保存量にも上限が無い。

## Decision

### 1. 同期の契約: best effort

- 同期と復旧は best effort とする。ある topic の全投稿を、すべての client が持つことを目標にしない。
- 表示と操作は、欠けている状態でも成立する。欠けている範囲は、利用者がそこを見ようとしたときに取りに行き、取れなければその旨を示して操作を続けられる。
- 「event の取りこぼしを全件比較で埋める」仕組みを持たない。replica の全件走査は、通常経路・復旧経路・利用者の操作の経路に置かない。

### 2. 読み取りの正本は projection、反映は 3 つだけ

表示と操作は projection（SQLite）だけを読む。docs から projection への反映は、次の 3 つに限る。

| 反映 | 契機 | 読む量 |
| --- | --- | --- |
| 個別反映 | docs の event（`InsertLocal` / `InsertRemote`）と gossip hint。key 単位。`objects/`・`reactions/`・`withdrawals/`・`sessions/*` の `/state` を扱う | 対象の key とその関連 key だけ |
| 窓の追いつき | 購読タスクの起動時、docs の同期の完了（`SyncFinished`）、event の取りこぼし（`Lagged`）の検出、空ページの表示。定期の polling はしない | 時系列の索引の新しい側から固定件数（窓）。projection に無い object だけを key 指定で反映する |
| 遡りの取得 | タイムライン・thread・profile で、projection のページが足りないとき | 表示中の最古の行より古い側を、索引から 1 ページぶん |

- 窓の大きさと 1 ページの件数は、replica の総件数に依存しない定数とする（初期値: 窓 200、ページは呼び出し側の `limit`）。
- 窓より古い範囲の取りこぼしは、遡りの取得で埋まる。埋まらない範囲が残ることを許容する。
- 取り下げは、取り下げの event・hint と、`withdrawals/` の新しい側の窓で反映する。表示側は projection の取り下げ表だけで判定し、view の生成中に docs を読まない。
  取り下げ済みの投稿の本文と添付は表示しない（窓より古い取り下げを持たない投稿を遡って反映するときは、その object の `withdrawals/<object id>/state` を key 指定で確認する）。
- 購読していない topic の投稿でありうる repost 元、profile の投稿、profile の投稿の返信先は、取り下げの event が届かない。表示したときに、背景で `withdrawals/<object id>/state` を key 指定で確認する。
  確認は確認先（replica と object id の組）ごとに間隔（初期値 60 秒）を空け、同時実行と台帳の件数に上限を置く。view の生成は確認を待たない。取り下げから表示が伏せられるまで、最大でこの間隔ぶん遅れうる。
  台帳の key に replica を含めるのは、topic を偽った repost の snapshot が、正しい topic での確認を見送らせないようにするため。
- 個別反映で投稿（`objects/<object id>/state`）を反映するときは、同じ object の `withdrawals/<object id>/state` を先に key 指定で確認する。
  取り下げの event が対象の envelope より先に届いて反映できなかった場合も、投稿の反映の時点で取り下げが反映される。
- 取り下げとして読めない record と、署名・著者が対象と合わない record は、取り下げとして扱わない（warn を出して無視し、投稿の反映を続ける）。
  public topic の replica は誰でも書けるので、読めない record を 1 件置くだけで、特定の投稿を隠したり topic 全体の操作を止めたりできないようにする。
  対象の envelope がまだ手元に無い取り下げは、対象が届いたときに反映し直す。docs と projection の読み書きの失敗は、無視せずエラーとして返す。
- 逆向きも同じく防ぐ（Issue #1250）。取り下げとして検証に通らない record を 1 件置くだけで、著者の正しい取り下げを無効にできないようにする。
  同じ key には docs author ごとの record があり、key 指定の読み出しは docs author の昇順で返すので、先頭の 1 件だけを見てはならない。
  `withdrawals/<object id>/state` を key 指定で読む入口（投稿の個別反映、取り下げの event・hint、背景の確認、遡りの取得）は、上限つきの読み出しで
  最大 8 record を調べ、その object を対象とし、検証に通る最初の取り下げを反映する。対象の envelope は、候補があるときだけ 1 回読む。
  上限を超える数の不正な record を先に積まれた取り下げは、この読み出しでは反映できない（best effort）。「上限に達したら伏せる」とはしない
  （不正な record を積むだけで他人の投稿を隠せてしまう）。これは、著者が docs author を示した投稿について ADR 0053（Issue #1258）が解消する: 読む側は先に「docs author と key の組」で 1 件読み、無ければこの上限つきの読み出しへ落ちる。
- 投稿の反映は、`objects/<object id>/state` の値を使わない（Issue #1248）。projection の行・通知・repost の snapshot・bookmark は、同じ object の
  署名つき envelope（`objects/<object id>/envelope`）から作る。envelope は `verify()` に通り、`envelope.id` が object id と一致し、投稿（post・comment・repost）で
  なければならない。public topic の replica は誰でも書けるので、署名の無い `state` の申告値（著者・本文・添付・topic・channel）を信用しない。
  `state` と `envelope` のどちらの event でも個別反映を試す（`state` が先に届いた投稿は、`envelope` の event で反映される）。
- 読んだ replica が受け入れる投稿だけを反映する。public topic の replica（`topic::<topic id>`）は、envelope の `topic_id` がその topic で、`channel_id` が無く、
  公開範囲が public の投稿。private channel の replica（`channel::<channel id>[::epoch::<epoch id>]`）は、envelope の `channel_id` がその channel で、`topic_id` が
  その channel を購読している topic の投稿（replica id は topic を含まないので、topic は購読の文脈から渡す。channel id は owner が決める文字列で、別の topic の
  channel と重なりうる）。それ以外の replica（author・device、区切りを含む channel id / epoch id）からは投稿を反映しない。署名が正しくても、別の replica に
  置かれた投稿は反映しない（public replica に置いた投稿が private channel の id を申告しても、その channel には出ない）。
- 同じ key には docs author ごとの entry がありうる。envelope の key は上限つき（8 件）で読み、検証に通る最初の 1 件を使う。先頭に不正な entry を 1 件置くだけで
  正しい投稿を隠せないようにする。上限を超える数の不正な entry を 1 つの key へ積まれた投稿は反映できない（best effort の範囲）。
- 検証に通らない record と読めない record は、その object だけを飛ばす（warn）。同じ replica の他の投稿の反映と、タイムライン・thread の取得を失敗させない。
- projection の投稿の行は、`projection_version` 3 から検証済みの投稿だけで作る。それより前の行は migration で消し、手元の docs から反映し直す。
- reaction の反映も、`reactions/<target>/<reaction id>/state` の値を使わない（Issue #1252）。行は同じ reaction の署名つき envelope
  （`reactions/<target>/<reaction id>/envelope`）から作る。envelope は `verify()` と `parse_reaction` に通り、対象と reaction id が key と一致し、
  reaction id が「読んだ replica・対象・署名者・reaction の key」から決まる値（`deterministic_reaction_id`）と一致し、topic / channel が読んだ replica の
  受け入れる範囲（投稿と同じ規則）と一致しなければならない。`state` と `envelope` のどちらの event でも個別反映を試す。
  同じ key の record は上限つき（8 件）で読み、検証に通るもののうち署名時刻が最も新しい 1 件を使う。反映済みの行より古い envelope では行を戻さない
  （古い署名つき envelope を置き直して、取り消した reaction を復活させられないようにする）。
- live session と game room の反映は、署名された manifest で確かめる（Issue #1252）。書く側は、manifest 全体を content にした envelope（kind は
  `live-session` / `game-session`）に署名して、state と同じ replica の `envelopes/<envelope id>` へ state より先に置き、`state.last_envelope_id` がそれを指す
  （Dome の Instance / Preset と同じ形）。読む側は `state` の key と `envelopes/<envelope id>` の key を、それぞれ上限つき（8 件）で読む。
  - live session と ScoreGame: 署名者が manifest の `owner_pubkey` と一致し、id の末尾（最後の `-` より後）が owner の pubkey の先頭（8 桁以上の 16 進）と
    一致し、state の id・topic・channel・owner・status と manifest blob の内容が署名された manifest と一致し、topic / channel が読んだ replica の受け入れる範囲と
    一致しなければならない。docs は同じ key を別の鍵でも書けるので、id と owner を結び付けて、別の鍵で署名した state による上書きを防ぐ。
    新しく作る id の末尾は 16 桁（64 bit）とする。それより前の id は 8 桁（32 bit）で、結び付けの強さはその桁数ぶんに留まる。
  - metaverse room: 訪問者も chat で room の manifest を書く設計なので、owner の署名は要求しない。topic / channel と Spatial Context が読んだ replica と一致し、
    id が Spatial Context と owner から決まる値（`dome-<hash>` の 24 桁）と一致することを確かめる。owner であることは、これまでどおり一覧の時点で
    署名つきの Dome Instance で確かめる（ADR 0036）。title などの表示内容は、その topic に書ける者が変えられる（Dome の authority の対象外）。
  - 署名された manifest を確かめる前に、未検証の state が指す manifest blob を取りに行かない。例外は、署名つきの envelope を持たない `dome-` の id の state
    （修正前の client が書いた Dome）で、manifest blob から上の metaverse room の規則で確かめる。
  - 利用者の操作（終了・参加・更新・Dome の移動と削除）が読む state と manifest も、同じ検証を通す。
  - 互換: 修正前の client が書いた live session と ScoreGame は署名つきの envelope を持たないので、修正後の client には表示されず、操作（終了・参加・更新）も
    できない（owner 自身の client でも同じ）。未検証の state へ owner が署名を付け直す経路は作らない（第三者が置いた内容へ署名させる入口になるため）。
    修正前の client には、その session がそれまでの状態のまま見え続ける。state doc の形は変えていないので、修正前の client は修正後の record を読める。
- reaction・live session・game room でも、検証に通らない record と読めない record は、その object だけを飛ばす（warn）。全件走査・event・hint・利用者の操作を失敗させない。
- reaction の行と、live session・game room の行は、`projection_version` 2 から検証済みの record だけで作る。それより前の行は migration で消し、手元の docs から反映し直す。

### 3. docs の読み出しの規則

- docs の query は、必ず key の索引（`SortBy::KeyAuthor`）で読む。`Exact` と `Prefix` の結果は key の昇順になる。
- 件数が上限なく増える prefix（`objects/`・`reactions/`・`withdrawals/`・`indexes/*`・`profile/posts/`・`profile/reposts/`）を、全件読みしない。
  読むときは、key だけを返す上限つきの読み出し（`DocsSync::query_replica_keys`: prefix、昇順 / 降順、`limit`）を使う。1 回の query が返す entry 数に上限を置く。
- `query_replica_keys` の trait の既定実装はエラーを返す。全件読みへ黙って落ちる実装を作らない。委譲 wrapper（`ReloadableDocsSync`）は転送を宣言する。
- iroh-docs の query には「この key より古い側」という範囲指定が無い。時系列の索引（時刻を 20 桁で 0 埋め）は、10 進の桁の prefix がそのまま時間の範囲になるので、
  cursor の時刻の桁を下から順に 1 つずつ減らした prefix を新しい側からたどって、古い側の 1 ページを読む（`query_time_index_desc`）。
  1 回の呼び出しの query 数は「時刻の各桁の数字の和 + 1」以下の定数で、replica の総 entry 数に依存しない。
- iroh-docs の key は任意の byte 列で、public topic の replica は誰もが書ける。UTF-8 でない key の entry は kukuri の key ではないので、読み出しはその entry だけを飛ばし、
  全体を失敗させない（#1253）。飛ばした分を読み足さず（追加の query を発行しない）、記録は読み出し 1 回につき 1 回だけ出す。飛ばす entry の本体は取得しない。
- 同じ秒の中の遡りは 512 件までを見る。1 秒に同じ索引へそれを超える投稿があると、超えた分は遡りで取りこぼしうる（best effort の範囲）。

### 4. 利用者の操作

- repost・reaction・reply・bookmark・取り下げ・community index の解決は、対象の行だけを引く。projection に無ければ `withdrawals/<object id>/state` と
  `objects/<object id>/state` を key 指定で反映し、無ければ操作を失敗として返す。replica を走査しない。
- 読み出しの policy: 利用者の操作の key 指定の読み出しは `LocalOnly` とする（entry の本体が手元に無い対象は、remote 取得で操作を待たせずに失敗として返す）。
  repost 元の解決だけは、購読していない topic の投稿を対象にできるよう `LocalThenRemote` とする（取得は #1207 の単一走査・クールダウン・同時実行の上限に従う）。
  event・hint の個別反映と背景の確認は `LocalThenRemote` とする。
- private channel の scope の操作は、対象の行が projection に残っていても、参加状態の確認（`ensure_private_channel_access`）を先に通す。
  退出した channel の投稿を、手元に残った行から操作できないようにする。確認は手元の状態の照会だけで、docs は読まない。
- 自分の既存の repost の検索など、条件で探す処理は projection の索引で行う。必要な列と索引は projection の schema に足す。

### 5. 上限

- 1 回の API 呼び出しが読む projection のページ数、1 回の反映が読む docs の entry 数、背景の取得の同時実行数、台帳の件数に上限を置く。
- 非表示の著者の行を読み飛ばす処理は、読むページ数に上限を置き、上限に達したら集まった分だけを返す。

### 6. key 設計と移行

- 時系列の索引は、既存の `indexes/timeline/<created_at 20 桁>-<object id>/<object id>` と `indexes/thread/<root>/<sort key>/<object id>` を正とする。
  既存の client が書いた entry をそのまま読めるので、この ADR の範囲では docs の key の移行は無い。
- projection の schema の追加（列・索引）は migration で行い、既存の行は反映し直さずに使えるようにする（足した列が空の行は、その行を次に反映したときに埋まる）。
- `created_at` は投稿者の申告値であり、未来や過去の値を持つ entry がありうる。窓は「索引の新しい側」から読むので、極端に未来の時刻の entry が窓を占有しうる。
  窓を読むときは、現在時刻 + 許容幅（初期値 10 分）より未来の entry を読み飛ばし、読み飛ばす件数にも上限を置く。

### 7. 残る総件数依存と、replica の時間分割

Context の 5 は、app-api の読み方を直しても残る。iroh-docs を fork せずに解消するには、1 つの replica の大きさに上限を置くしかない。

- 方向: public topic・private channel の epoch・author の replica を、時間の bucket で分ける。client は新しい側の bucket だけを常時同期し、古い bucket は遡ったときに開く。
  1 つの replica の大きさは「投稿の頻度 × bucket の長さ」で抑えられ、累積の履歴に比例しなくなる。保存量にも上限を置ける。
- 本 ADR の窓の追いつきと遡りの取得は、replica を分けた後も bucket ごとにそのまま使う。
- これは protocol の変更で、旧 client・CN indexer との相互運用、移行、bucket の長さ、bucket をまたぐ参照（thread・reaction・取り下げ）の扱いを決める必要がある。
  詳細は後続の ADR で定め、実装は Issue #1243（#1239 の子 Issue）が所有する。同期の対象と同時に開く replica の上限（作業集合）は Issue #1224 の設計と合わせる。

## Consequences

- 全件走査（`hydrate_subscription_state`・`hydrate_topic_state`・`hydrate_scope_projection`・`hydrate_author_state`）と、#1225 の `ReplicaScanCache` は削除する。
  `MissingBodyLedger`（欠損した本文の行単位の取り直し）は残す。
- 窓より古い範囲は、遡るまで projection に入らない。検索や集計のように「全件を前提にする」機能は、client 単体では成立しない前提で設計する
  （community index は CN が担う。`docs/architecture/p2p-first-community-node-responsibility-boundary.md`）。
- docs 同期より先に届いた hint の対象は、doc event の到着か次の窓の追いつきまで表示されない。
- 完了条件は、replica の件数を 1,000 / 10,000 / 100,000 にしても、定期処理・利用者の操作・表示の各操作が読む docs の entry 数と projection の行数が増えないことを、
  回数で assert する test で示す。所要時間の閾値は使わない。

## References

- Issue #1221（統括）、#1239（本 ADR の実装）、#1248・#1252（反映の検証）、#1243（replica の時間分割）、#1224（接続と取得の統合設計）、#1225（全件走査の頻度の抑制）、#1207（blob の再取得）
- `AGENTS.md` の「設計原則: 件数に依存しない処理」
- `docs/architecture/replica-read-inventory.md`
- iroh-docs 0.101.0: `src/store/util.rs`（`IndexKind::from`）、`src/store/fs/query.rs`、`src/store/fs/bounds.rs`、`src/store/fs.rs`（`get_fingerprint`）

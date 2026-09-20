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

### 3. docs の読み出しの規則

- docs の query は、必ず key の索引（`SortBy::KeyAuthor`）で読む。`Exact` と `Prefix` の結果は key の昇順になる。
- 件数が上限なく増える prefix（`objects/`・`reactions/`・`withdrawals/`・`indexes/*`・`profile/posts/`・`profile/reposts/`）を、全件読みしない。
  読むときは、key だけを返す上限つきの読み出し（`DocsSync::query_replica_keys`: prefix、昇順 / 降順、`limit`）を使う。1 回の query が返す entry 数に上限を置く。
- `query_replica_keys` の trait の既定実装はエラーを返す。全件読みへ黙って落ちる実装を作らない。委譲 wrapper（`ReloadableDocsSync`）は転送を宣言する。
- iroh-docs の query には「この key より古い側」という範囲指定が無い。時系列の索引（時刻を 20 桁で 0 埋め）は、10 進の桁の prefix がそのまま時間の範囲になるので、
  cursor の時刻の桁を下から順に 1 つずつ減らした prefix を新しい側からたどって、古い側の 1 ページを読む（`query_time_index_desc`）。
  1 回の呼び出しの query 数は「時刻の各桁の数字の和 + 1」以下の定数で、replica の総 entry 数に依存しない。
- 同じ秒の中の遡りは 512 件までを見る。1 秒に同じ索引へそれを超える投稿があると、超えた分は遡りで取りこぼしうる（best effort の範囲）。

### 4. 利用者の操作

- repost・reaction・reply・bookmark・community index の解決は、対象の行だけを引く。projection に無ければ `objects/<object id>/state` を key 指定で 1 回反映し、
  無ければ操作を失敗として返す。replica を走査しない。
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

- Issue #1221（統括）、#1239（本 ADR の実装）、#1243（replica の時間分割）、#1224（接続と取得の統合設計）、#1225（全件走査の頻度の抑制）、#1207（blob の再取得）
- `AGENTS.md` の「設計原則: 件数に依存しない処理」
- `docs/architecture/replica-read-inventory.md`
- iroh-docs 0.101.0: `src/store/util.rs`（`IndexKind::from`）、`src/store/fs/query.rs`、`src/store/fs/bounds.rs`、`src/store/fs.rs`（`get_fingerprint`）

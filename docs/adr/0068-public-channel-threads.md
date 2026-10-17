# ADR 0068: 公開 channel（公開スレッド）

## Status

Accepted（2026-10-17、#1840）。#1840 の AC-1 で識別・署名・置き場所・投稿の経路を、AC-2 で topic ごとの一覧（§6）を、AC-3 で画面（§7）を、AC-4 で配信中の Live（§8）を実装した。

## Context

kukuri の channel は、招待や相互フォローで参加する private channel だけだった。`ChannelRef` は topic の公開タイムライン（ID なし）と private channel（ID あり）の 2 種類で、多くの処理が「channel の ID があれば private」と決めていた。

#1840 では、topic の中に誰でも読み書きできるタイトル付きの channel を作り、掲示板のスレッドとして使う。判断の経緯と案の比較は [#1840 のコメント](https://github.com/kukuri-app/kukuri/issues/1840#issuecomment-6230392310) にある。

## Decision

### 1. 識別子と metadata

- 公開 channel の ID は `pc-<作成時刻のミリ秒>-<作成者の公開鍵の先頭 16 桁>` とする（Live の session ID と同じ 64 bit の結び付け）。
  - 作成時刻から metadata を置いた bucket が決まるので、ID だけで metadata を 1 件読める。
  - private channel の ID（`channel-<ミリ秒>-<8 文字>`）とは形が重ならない。private channel の参加（招待・grant・share の取込み）は、この形の ID を拒否する。
  - 公開 channel かどうかは ID の形で見分ける（`ChannelId::is_public_channel`）。「ID があるか」では決めない。
- metadata は作成者が署名した envelope（種類 `public-channel`）で、中身は ID・所属 topic・タイトル（1〜200 文字、前後に空白なし）・作成時の最初の投稿の ID（無くてもよい）。
  - 署名の時刻は ID の作成時刻（秒）にする。読む側は署名、ID と署名者・時刻の結び付き、上限を確かめる。
  - どの欄も後から変えない。タイトルの変更と channel の削除は無い。
- 作成の順は、ID を決める → 最初の投稿を channel の投稿として保存する → その ID を入れた metadata を署名して保存する。投稿は channel の ID だけを参照するので、互いの hash を待つ循環は起きない。
- ID から読むときは、metadata と、作成時の最初の投稿を作成日と翌日の bucket から 1 件ずつ読む。作成から日を跨いだ channel でも、ID から最初の投稿の返信ツリーを開ける。

### 2. 置き場所

公開 channel の内容は、すべて所属する公開 topic の時間 bucket（[ADR 0054](0054-time-bucketed-docs-replicas.md) §1）に置く。専用の replica は作らない。

| record | key | bucket |
| --- | --- | --- |
| metadata | `channels/list/<作成時刻の秒 20 桁>-<channel ID>/<channel ID>` | ID の作成時刻 |
| 投稿・返信 | `objects/<id>/{state,envelope}`（既存と同じ） | 署名した作成時刻 |
| channel の時刻順の索引 | `channels/<channel ID>/timeline/<並びの key>/<object id>` | 同上 |
| 最後の投稿の索引 | `channels/activity/<投稿の作成時刻 20 桁>-<channel ID>.<投稿 ID>/<channel ID>.<投稿 ID>` | 同上 |
| 返信の索引 | `indexes/thread/<root>/…`（既存と同じ） | 同上 |
| 添付の manifest・リアクション・取り下げ・リンクプレビュー | 既存と同じ | 既存と同じ |

- metadata の key と最後の投稿の索引は、タイムラインの索引と同じ形（時刻 20 桁の 0 埋め）にする。作成の新しい順と最後の投稿の新しい順に、既存の時刻の索引の読み出しで有界に読める。ID からは、作成時刻から key を作って 1 件読む。
  - AC-1 で使った `channels/list/<channel ID>` は公開していない版なので、移行は無い。
  - 最後の投稿の索引の id は channel ID と投稿 ID の組なので、投稿の本体を読まずに channel ごとにまとめられる。
- 投稿の envelope は既存の項目で表す（`channel_id` に公開 channel の ID、`visibility` に Public）。envelope の形は変えない。
- 公開 channel の投稿は、topic のタイムラインの索引（`indexes/timeline/`）にもプロフィールの索引にも載せない。topic のタイムラインとプロフィールの投稿一覧には出ない（2026-10-17 ユーザー判断）。
- Community Node は topic の bucket の「channel_id あり」の投稿と `channels/` の key を読まないので、公開 channel の投稿は CN の検索に出ない（同日、受け入れ済み）。

### 3. 受信側の確認

- 公開 topic の replica は、topic の公開タイムラインの投稿と、公開 channel の ID の形で公開範囲が Public の投稿・リアクション・session を受け入れる。それ以外の channel の申告（private の形の ID、公開範囲が Public でないもの）は従来どおり拒否する。
- private channel の replica は、公開 channel の ID の形の申告を受け入れない。
- 旧版の端末は従来の確認で公開 channel の投稿を拒否するので、旧版では公開 channel も投稿も見えない（同日、受け入れ済み）。

### 4. hint・購読・読み先

- channel の中の投稿・リアクション・取り下げの hint は、channel ごとの hint の topic（`hint/public-channel/<channel ID>`）に送る。rendezvous には公開の鍵を使う。この topic は topic の一覧・診断・CN の公開 topic の索引に出さない（`wire::is_non_public_topic`）。
- 作成者は、作成を topic の hint の `SessionChanged`（種類 `public-channel`、`session_id` に channel ID）で知らせる。topic の lease を持つ端末は metadata を 1 件読んで一覧の行に置く（§6）。旧版は未知の種類として読まない。
- 公開 channel の列は `Topic(topic)` と `PublicChannel(topic, channel)` の 2 つの lease を取る（[ADR 0055](0055-demand-owned-network-work.md) §1.1）。`PublicChannel` の task は channel の hint だけを購読し、topic の gossip の停止に従う。topic の lease だけの端末は channel の投稿を受け取らない。
- channel のページと対象の取得は、channel の hint の近隣を先に、足りない分を topic の近隣で埋めて有界に読む（最大 4）。他の参加者が検証して保持した内容の提供（ADR 0054 §4、#1395）は topic の bucket に働くので、提供側の変更は要らない。保持する側は、書き手と同じ関数で §2 の索引を導く。

### 5. API

- `ChannelRef` と `TimelineScope` に公開 channel の種類（`public_channel`）を足す。既存の `public` と、private channel を指す `private_channel`・`channel` の意味は変えない。
- 公開 channel への返信は親の channel に置く。リアクション・取り下げ・bookmark は対象の channel と同じ種類を求める。
- 公開 channel の投稿は repost できない（既存の channel の投稿と同じ。同日のユーザー判断）。
- ID からの取得（`get_public_channel`）は、一覧と同じく非表示の作者の channel を返さない（#1840 AC-3）。
- ゲーム・metaverse（Dome）は公開 channel を受け付けない。Live は §8。

### 6. 一覧（#1840 AC-2）

- `list_public_channels(topic, order, cursor, limit)` は、topic の公開 channel を新しい順に返す。order は「最後の投稿」（既定）と「作成」の 2 つ（2026-10-17 のユーザー判断 2）。戻り値は行・続きの位置・取得できなかった数（`unavailable_count`）。
- 表示は手元の行だけを読む（[ADR 0052](0052-scale-independent-timeline-sync.md) §2）。
  - 行は SQLite の `public_channels`（索引は (topic, 作成時刻, ID) と (topic, 最後の投稿の時刻, ID)）と、Web の IndexedDB の版 3 の store（同日の判断 7）に置く。
  - 非表示の作者（mute と、どちらの向きでも有効な block）のスレッドは除く。読み飛ばすのは 4 ページまで（ADR 0052 §5）。
- 行の最後の投稿の時刻は、公開 channel の投稿を projection に反映するたびに新しい方へ進める（自分の投稿も同じ）。非表示の作者の投稿は数えない。
- 照合（ページの範囲の照合と同じ扱い）:
  - 先頭のページは背景で照合を始めて 300 ms だけ待ち、続きのページは照合を待つ（タイムラインと同じ。#1624）。同じ範囲は 30 秒の間隔の内では繰り返さない。
  - 作成の新しい順は、cursor の日（無ければ現在）から古い側へ 4 日分の bucket の metadata の索引を、topic の近隣（最大 4 台）から、読み先と bucket ごとに `limit` 件まで読む。
  - 最後の投稿の新しい順は、現在の日（続きは cursor の日）と前日の bucket の、最後の投稿の索引（読み先と bucket ごとに `limit` の 5 倍）と metadata の索引を読む。
  - channel ごとに最も新しい記録だけを残す。手元の行が無いもの、または行より新しいものだけを、記録を読んだ読み先から確かめてから反映する（1 回 20 件まで、全体で 30 秒）。確かめるのは、metadata の署名と、最後の投稿の記録なら指している投稿の署名・channel・時刻。
  - 読めない・確かめられない記録の数を `unavailable_count` で返す。投稿の多いスレッドが最後の投稿の索引の新しい側を占めると、ほかのスレッドの新しい投稿を拾いきれないことがある（同日の判断 2 で受け入れた）。
  - 続きは手元の行の続きから読み、その位置の日から照合する（タイムラインと同じ）。この端末が知らない、照合の範囲より前のスレッドは、手元の行が尽きると一覧に出ない。リンクと通知からは開ける（2026-10-17 のユーザー判断 13。照合した範囲の前から続ける位置を返す案は、前のページの行を次のページに繰り返すので採らない）。
- 一覧は channel の投稿を読まず、channel ごとの hint も購読しない。一覧の列が取るのは topic の lease だけ。
- 保持: 他人が作った行は remote cache の種類 `public_channel` として容量を数え、7 日使わなければ回収する。一覧のために手元から読んだ行（返さなかった非表示の作者の行を含む）と、照合や ID から引いた行は、使った時刻を更新する（1 時間ごと）。自分が作った行は数えず、回収しない。期限を過ぎてまだ回収されていない行は無いものとして扱い、次の照合が確かめて置き直す（投稿の行と同じ）。

- 行の作成者の名前は、ページの作成者の手元の profile から付ける。手元に無い profile は、タイムラインの投稿の作成者と同じ背景の取得（重複を除き、上限と間隔つき）へ渡し、読めたら次の取得で付く（#1840 AC-3）。

### 7. 画面（#1840 AC-3）

- 一覧は列の種類 `publicThreads`（scope は topic で、channel は無し）。route は `#/timeline?topic=<topic>&context=publicThreads`。Control Center の topic の「公開」の次の「公開スレッド」の行と、列の追加から開く（2026-10-17 のユーザー判断 9）。
  - 並び順は列ごとの `publicThreadOrder`（既定は `last_post`、ほかに `created`）で、列と一緒に保存する。列の上部の icon の menu で切り替え、切り替えると読み込んだ行を捨てて先頭から読む。
  - 行はタイトル・作成者・並び順の時刻。表示中の列だけが 3 秒ごとに先頭のページ（20 件）を読み直し、読み込んだ続きの行の前に重ねる。続きは下端で読む。行は 200 件までで、超えたら先頭の行を外し、「最新に戻る」で先頭から読み直す（タイムラインと同じ）。
  - 作成は列の下部の「スレッドを作る」の Dialog（タイトルと最初の投稿。判断 3）。作った後は詳細の列を開き、その topic の一覧の列を先頭から読み直す。
- 詳細は timeline の列（scope の channel が公開スレッドの ID）で、見出しは「公開スレッド」、scope の表示は「<タイトル> · <topic>」。route は `#/timeline?topic=<topic>&channel=<ID>`。
  - 新しく開くときは、route の反映が、その topic の一覧の列（無ければ作る）を親にする。直接リンクや本文のリンクから開いても、閉じると一覧の列へ戻る。
  - 列の上部の「リンクをコピー」はこの route を写す（判断 5）。タイトルを知らなければ、列が表示中の間に ID から読み、読めなければ「タイトルを取得できません」と出す。Control Center の列の一覧でも「公開スレッド」と呼ぶ。
- 本文の中のリンクはタイトルの chip にする（判断 10）。知らないスレッドの chip は、画面内にある間に ID から読み、読めた値を chip が持つ（覚えたスレッドから外れても表示を保ち、読むのは chip ごとに 1 回まで）。本文の下には最初の 1 件だけ、タイトルと作成時の最初の投稿の card を出す。card は画面内にある間だけ読み、最初の投稿は返信ツリーの取得で 1 件読む。ID からの読み込みは画面で共用し、同じスレッドを並行して読まず、読めたものは使い回す（読めなかったものは次に表示したときに読む）。
- 非表示の作者（mute と、どちらの向きの block）のスレッドは、一覧の行・本文の chip と card・詳細の見出しに出さない。mute・block した直後は、読み込んだ行も次の読み直しを待たずに出さない。
- 公開スレッドの投稿の公開範囲の icon は公開と同じ `book-open` で、tooltip は「公開スレッド: <タイトル>」（判断 11）。外部のリンクの preview は、topic の公開の投稿と同じく出す。
- 読んだスレッドは、列の見出し・本文の chip・公開範囲の tooltip のために 256 件まで覚える（内容が変わったものを新しい側へ回し、超えたら古いものから外す）。端末には保存しない。
- 呼び名は「公開スレッド」「公開スレッド一覧」、英語は "Public thread"・"Public threads"、簡体字中国語は「公开帖子」「公开帖子列表」（判断 1・12）。既存の返信ツリーの「スレッド」（列の種類 `thread`）は変えない。

### 8. 配信中の Live（#1840 AC-4）

- 公開 channel で Live を始められる（`create_live_session_in_channel` の `public_channel`）。manifest と開始 post の channel は公開 channel を指し、置き場所は topic の公開の Live と同じ topic の bucket。
  - Live の変化（作成・終了）と視聴の presence（参加・退出・10 秒ごとの継続）は topic の hint に送り、参加の lease は topic だけを取る。一覧の列は topic の lease だけで配信中の印を変える。topic の hint で届いた presence は、受け取る側が Live の行の channel（公開 channel）に結び付けて視聴者に数える（private channel の Live の presence は、従来どおり channel の hint で届いたものだけを数える）。
  - 視聴の資格は topic の公開の Live と同じ（資格の判定の Spatial Context は topic）。private channel の参加を求めない。
  - 公開の Live なので、どこからでも紹介できる（`LiveRefV1::allowed_in`）。紹介の Dialog の出し先は、始めたスレッドを先頭（既定）にし、ほかは公開の Live と同じ（2026-10-18 のユーザー判断 17。#1839 の判断 1）。
- 一覧の行は配信中の印（`live_now`）を持つ。行ごとに、配信中の行だけの部分索引（`idx_live_session_cache_live_now`）から 8 件まで読み、非表示の主催者の Live は数えない。終わった Live の数に比例しない。Web は Live を扱わない（[ADR 0060](0060-web-client-entry-capability-and-verification.md)）ので印は付けない。
- 画面:
  - 詳細の列の上部（本文と一緒に流れない）に、そのスレッドで始めた配信中の Live を、新しく始まった順に 3 件まで、タイトルと配信者の 1 行ずつで出す（判断 6・14）。押すと、詳細の列を親にしたスレッドの「ライブ」の列（列の種類 `stream`、scope は公開スレッド）を開いてその Live を示す。より多いときは「すべての配信」から列を開く。
  - 上部の「ライブ」（「リンクをコピー」の隣）もこの列を開き、列の下部の「ライブを開始」でスレッドに Live を始める（判断 15）。topic の「ライブ」の列には、スレッドの Live を出さない（判断 16）。
  - 上部は、表示中の詳細の列だけが `list_live_sessions`（公開スレッドの scope）を読む。読むのは列が active になったときと、同期の時刻が変わったとき（「ライブ」の列と同じ）。非表示の主催者（mute・block、CN の評価で非表示を勧めるもの）の Live は出さない。
  - 上部の行・「ライブ」・一覧の印は、既存の Live の画面と同じく開発者モードでだけ出す（README の Experimental）。
  - 上部と印を出すだけでは、映像・音声を受信も送信もしない。視聴は「ライブ」の列や投稿の中の Live の「参加」からで、列を閉じても配信は止めない（#1839）。

## 互換

| 対象 | 変更 | 旧版・既存データへの影響 |
| --- | --- | --- |
| 投稿の envelope | 形は変えない。「channel_id あり + Public」の組を新たに使う | 旧版は受け付けず、表示しない |
| channel の metadata | 新しい種類の署名つき envelope | 旧版は読まない |
| docs の key | topic の bucket に `channels/list/…`・`channels/activity/…`・`channels/<ID>/timeline/…` を足す | 旧版と CN は読まない |
| hint | channel ごとの hint の topic を新設する。作成の知らせは既存の `SessionChanged` に種類を足す | 旧版は購読しない。未知の種類は読まない |
| API・CLI | `ChannelRef`・`TimelineScope` に種類を足し、作成・取得・一覧の command を足す。行に作成者の名前（無いときは欄を出さない）と配信中の印（`live_now`）を足す。Live の作成で `public_channel` を受け付ける | 既存の値と出力の意味は変えない |
| Live の manifest・参照 | 形は変えない。channel に公開 channel の ID を使う。公開 channel の Live への参照はどこでも許す | 旧版は公開 channel の Live を受け付けない。旧版は、別の場所の紹介 post の参照を限定 channel の Live と見て捨てる（Live の表示の無い投稿に見える。#1839 の判断 3 と同じ） |
| workspace の保存・route | 列の種類 `publicThreads` と並び順の欄、route の `context=publicThreads` を足す。詳細の列は既存の timeline の列（scope の channel が公開スレッドの ID） | 旧版は一覧の列を読み飛ばし、`context=publicThreads` を topic のタイムラインへ正規化する。詳細の列とそのリンクは、旧版では参加していない channel として投稿が出ない（判断 8 の範囲） |
| SQLite | 一覧の表 `public_channels` と migration を足し、remote cache に種類 `public_channel` を足す。Live の表に配信中の行の部分索引を足す（AC-4） | 既存の表と行は変えない |
| IndexedDB | 版を 2 から 3 に上げ、一覧の store を足す | 更新の間は「データの移行中です」で待つ。更新した database を旧版で開くことは扱わない（2026-10-08 の決まり） |
| backup・account 同期 | 変えない（公開 channel には参加の状態が無い。一覧の行は account の SQLite に入る） | — |

## Feature Data Classification

- Feature 名: 公開 channel（公開スレッド）
- Durable / Transient: Durable（metadata と投稿）。hint は Transient
- Canonical Source: 作成者が署名した metadata の envelope と、投稿者が署名した投稿の envelope
- Replicated?: はい（公開 topic の bucket を有界に読み、検証した record を保持して提供する）
- Rebuildable From: 公開 topic の bucket の署名つき record
- Public Replica / Private Replica / Local Only: Public Replica（所属 topic の時間 bucket）
- Gossip Hint 必要有無: あり（channel ごとの hint の topic）
- Blob 必要有無: 投稿の本文と添付（既存の投稿と同じ）
- SQLite projection 必要有無: 投稿は既存の projection（channel の値は公開 channel の ID）。一覧の行は `public_channels`（§6）
- 必須 contract: 公開 channel の ID の形と結び付き、metadata の署名、topic の bucket での受け入れと拒否（`crates/core/src/tests/public_channels.rs`、`hydration_integrity_contract.rs`）
- 必須 scenario: 招待の無い端末が ID から読み、投稿・返信・リアクションし、作成者に同じ channel の文脈で届く（`crates/app-api/src/tests/public_channels.rs`）。別の端末のスレッドを 2 つの並び順で一覧から取得し、新しい投稿で上へ移り、読めない記録を数え、無関係なデータで読む量が増えない（`crates/app-api/src/tests/public_channel_list.rs`）。2 人が同じスレッドで始めた Live がスレッドの Live の一覧と一覧の行の印に出て、片方を終えても残り、topic の資格で視聴できる（`crates/app-api/src/tests/public_channel_live.rs`）

## References

- [#1840](https://github.com/kukuri-app/kukuri/issues/1840)
- [ADR 0018](0018-channel-first-sidebar-and-unified-epoch-lifecycle.md)（private channel）
- [ADR 0054](0054-time-bucketed-docs-replicas.md)、[ADR 0055](0055-demand-owned-network-work.md)

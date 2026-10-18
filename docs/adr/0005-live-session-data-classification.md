# ADR 0005: Live Session Data Classification

## Status
Accepted

## Feature Data Classification
- Feature 名: live session
- Durable / Transient: Durable
- Canonical Source: `iroh-docs` for current state pointer, `iroh-blobs` for manifest payload
- Replicated?: Yes
- Rebuildable From: `docs + blobs`
- Public Replica / Private Replica / Local Only: Public replica for session state pointer, local projection for viewer presence cache and joined-by-me state
- Gossip Hint 必要有無: Yes, `LiveSignal` for start/end and `LivePresence` for viewer heartbeat only
- Blob 必要有無: Yes
- SQLite projection 必要有無: Yes
- 必須 contract:
  - `late_joiner_backfills_live_session_manifest`
  - `restart_restores_live_session_manifest`
  - `live_presence_expires_without_heartbeat`
  - `ended_live_session_rejects_new_viewers`
  - `start_post_and_live_resolve_from_another_device_and_restore`（#1839）
  - `third_party_introduces_the_same_live_in_another_topic`（#1839）
  - `limited_live_stays_in_its_channel`（#1839）
  - `legacy_live_without_a_start_post_is_read_as_is`（#1839）
  - `resolving_a_live_does_not_grow_with_unrelated_records`（#1839）
- 必須 scenario:
  - live session panel 導入後に `create live -> join -> viewer count visible -> end -> restart -> restored ended state` を追加する

## Decision
- `docs` の topic replica に `live/<session_id>/state` を置き、current manifest blob ref と状態の index metadata だけを保存する。
- live manifest 本体は JSON blob として `iroh-blobs` に保存し、更新のたびに新しい blob hash を払い出す。
- state と manifest blob は署名を持たない。owner は manifest 全体を content にした `live-session` の envelope に署名して同じ replica の
  `envelopes/<envelope id>` へ置き、`state.last_envelope_id` がそれを指す。読む側は、署名者・id と owner の結び付け・読んだ replica との整合を確かめた
  session だけを反映し、操作にも使う（Issue #1252。規則の正本は ADR 0052 §2）。
- manifest は owner の署名対象となる必須の `revision` を持つ。初版は 1、更新ごとに 1 増やす。同じ key の候補と永続 projection は revision で新旧を判断し、
  `state.updated_at` や record の到着順を使わない。#1260 時点で本番 record は無いため、revision の無い旧形式は移行せず拒否する。
- `LiveSignal` は `SessionStarted` と `SessionEnded` の通知だけに使い、viewer 数の正本にはしない。
- viewer presence は `LivePresence` heartbeat と local SQLite projection で扱い、restart 後に自動再 join はしない。

### 追補: 映像・音声の descriptor（#1034 AC-2a、2026-10-13）
- manifest に任意の `media_endpoint_id`（映像・音声を送る配信者の端末の iroh endpoint id）を足す。desktop runtime（GUI・CLI）の作成では
  作った端末の id を入れ、Web の作成と旧版の manifest は持たない（media の無い session）。owner の署名と revision は既存の manifest の規則のまま。
- 無いときは JSON に出さないので、media の無い session の manifest は従来と同じ bytes になる。旧版は未知の項目を無視して読み、映像・音声を
  扱わない。視聴者は検証済みの manifest の id へだけ購読する（ADR 0067 §3）。配信ごとの署名鍵の公開鍵は下の追補。

### 追補: 映像・音声の署名鍵（#1034 AC-2b、2026-10-13）
- manifest に任意の `media_signing_key`（配信者が映像・音声の frame に付けた署名を確かめる x-only 公開鍵の hex）を足す。`media_endpoint_id` と
  同じく desktop runtime の作成で入れ、owner のアカウントの鍵と session id から導く（ADR 0067 §8）。owner の署名と revision は既存の manifest の
  規則のまま。無いときは JSON に出さない。
- 鍵の無い manifest は、`media_endpoint_id` があっても media の無い session として扱う（視聴者は購読せず、配信者は配信を始めるときに理由を
  示して断る）。

### 追補: 開始 post・紹介 post と作成場所からの権限（#1839 AC-1、2026-10-17）
- Live の識別子は既存の `session_id` とし、Live の正本はこの session のまま変えない。別の registry は作らない。
- manifest に任意の `start_post_id`（開始 post の envelope id）を足す。owner の署名と revision は既存の manifest の規則のまま。無いときは JSON に出さない。
  開始 post の無い旧版の manifest は、開始 post の無い Live として読み、移行も破棄もしない。
- post の署名対象 content に任意の `live`（`session_id`、Live を作成した `topic_id`、限定なら `channel_id`）を足す。無い post は従来と同じ bytes で、
  旧版は知らない項目として読み飛ばし、通常の投稿として表示する。開始 post と紹介 post は同じ項目を使う。
- 作成は、`session_id` を決め、`live` を持つ開始 post を Live の作成場所に置き、その id を `start_post_id` に入れた revision 1 の manifest を置く順に行う。
  post は Live の id しか参照しないので、互いの hash を待つ循環は無い。開始 post は id が `start_post_id` と一致し、著者が owner の post に限る。
  それ以外の `live` を持つ post は紹介 post で、紹介者は Live の操作の権限を得ない。Live への返信・リアクションは開始 post に付ける。
- Live の公開範囲は作成場所（manifest の topic・channel）で決まり、紹介 post の場所・紹介者・閲覧中の topic では変わらない。限定 channel の Live は、
  その channel の post からだけ参照できる。作成時に拒否し、受信した post の参照も同じ規則（`LiveRefV1::allowed_in`）に合わなければ捨てる。
  非公開投稿を repost できないのと同じく、公開の場所に限定 channel の id と Live の id を載せない。
- 別の topic の Live は、表示中の post の参照ごとに既存の表示要求（`set_session_display`、Live の topic と scope を指定）で、その 1 件だけを読む。
  表示用の行は `session_id` が主キーなので、無関係な投稿・Live の件数に比例して読まない。限定 channel の Live の行は、その channel に参加している間だけ返す。
- 表示用の行は SQLite の `object_index_cache.live_json` と `live_session_cache.start_post_id` に置く（migration `20261017000000`）。Web の行は serde の既定値で読む。
- 画面（#1839 AC-2）: 作成 Dialog で開始 post の本文を受け取り、投稿の中の Live は画面に入ったときに表示要求で読む。参照の `channel_id` と読めた Live の
  `channel_id` が一致しない参照は展開しない。紹介 post は出し先を選ぶ Dialog で作り、限定 channel の Live はその channel に固定する。UI の記録は
  `docs/ui-reviews/2026-10-17-1839-live-object.md`。

### 追補: 映像に投稿とリアクションを重ねる表示（#1841、2026-10-18）
判断と D1〜D3 の結論は [#1841 のコメント](https://github.com/kukuri-app/kukuri/issues/1841#issuecomment-6243763891)。重ねる表示は視聴する端末の画面だけで作り、映像の bytes・署名・時刻、
post・リアクションの正本、Live の寿命・権限・視聴の共有（#1839）を変えない。新しい正本・会話の写し・Live 専用の反応の台帳は作らない。
- 入力の結び付け: 重ねる Live（参照の topic・channel・session）、会話を読む場所（ライブの列の scope）、Live の開始 post（manifest の `start_post_id`）の 3 つを
  別々に持つ。Live は参照から表示要求で 1 件読み、会話の場所の投稿は通常の列と共用する scope ごとの cache から読む。Live の視聴の資格と会話の場所の閲覧の資格は
  別々に満たす（private channel の会話は、参加しているときだけ読む）。
- Live への反応は開始 post だけに属する。紹介 post やスレッドの別の投稿へのリアクションを、Live への反応として数えたり浮かべたりしない。会話の場所の投稿のうち、
  開始 post の返信ツリーに入るもの（返信先か root が開始 post）は「ライブへの返信」として示す。
- 数: 開始 post を返信ツリーの root だけで読み、その `reaction_summary` を使う。読み直しは同期の時刻の変化で行う（timer は使わない）。集計には通常の表示と同じく
  非表示の作者のリアクションも入る。
- 新着: 手元へ反映したリアクションの新しい版のうち、付いてから 30 秒の内のものを runtime event `reaction_received` で受け（[ADR 0017](0017-reaction-data-classification.md) §4.4）、
  対象が開始 post で、開いた時点以降に付いたものだけを浮かべる。初回の表示・再接続・照合で届いた古い版は浮かべず、外した版は数の読み直しで減るだけにする。
  履歴は読まない。
- 投稿の差分: 開いた時点の投稿は動かさずに出し、後から届いた、開いた時点より新しい投稿だけを新着として示す。取り下げた投稿・本文を取得できない投稿・成人向けや
  信頼値の評価で隠す投稿は重ねない（mute・block の作者の投稿は、既存どおり一覧から除かれる）。
- 上限（描画の資源の管理で、元の post とリアクションは消さない）: 重ねる投稿は最新 5 件。新着の判定に覚える投稿は 25 件。浮かべるのは 1 個 3 秒、同時に 12 個、
  待ちは 24 個（溢れたら古いものを捨てる）、重複の判定に覚えるのは 256 件。リアクションの runtime event は 1 秒に 36 件まで（浮かべる同時の数と待ちの数の和）。
  event の channel は全種類で共有し、溢れると画面へ読み直し（画像の解放・関係の読み直し）を促すので、照合で多くのリアクションをまとめて反映しても溢れさせない。
  溢れた分は送り直さない。表示を閉じる・Live を切り替えると、待ち・timer・購読を捨てる。
- Feature Data Classification: Transient / Local Only。runtime event と演出の待ちは保存しない。表示の設定は端末に 1 組だけ保存する（他の利用者へ送らない）。
  docs・wire・SQLite・IndexedDB・backup の形式は変えない。

## Consequences
- 映像・音声の搬送は [ADR 0067](0067-media-transport-moq.md) に従う。manifest の descriptor は上の追補による（分類は ADR 0067 の Feature Data Classification）。
- #1262: state/envelopeの到着による個別反映、表示中だけのmanifest取得、未取得候補と検証済みsessionの区別はADR 0052「Session の個別反映と表示要求」に従う。timerによる反映retryはしない。
- #1813: gossipのhintで読めなかったsessionの上限つきの読み直しは、ADR 0052「Session の個別反映と表示要求」の例外（ADR 0054「読み取り候補」）。
- late joiner と restart 後の復元は `docs state + manifest blob` だけで成立しなければならない。
- 終了済み session は durable state で `Ended` として復元され、新規 join を拒否しなければならない。
- 過去に署名された revision を置き直しても、終了済み session の表示と操作は過去へ戻らない。
- viewer count は shared durable state ではなく local projection の TTL 管理で表現される。

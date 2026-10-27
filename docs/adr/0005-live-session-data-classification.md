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
  信頼値の評価で隠す投稿は重ねない（mute・block の作者の投稿は、既存どおり一覧から除かれる）。本文も添付も無い投稿（本文を空にした開始 post・単純な repost）も
  重ねず、最新 5 件はそれ以外の投稿から数える（2026-10-18 のユーザー判断）。
- 上限（描画の資源の管理で、元の post とリアクションは消さない）: 重ねる投稿は最新 5 件。新着の判定に覚える投稿は 25 件。浮かべるのは 1 個 3 秒、同時に 12 個、
  待ちは 24 個（溢れたら古いものを捨てる）、重複の判定に覚えるのは 256 件。リアクションの runtime event は 1 秒に 36 件まで（浮かべる同時の数と待ちの数の和）。
  event の channel は全種類で共有し、溢れると画面へ読み直し（画像の解放・関係の読み直し）を促すので、照合で多くのリアクションをまとめて反映しても溢れさせない。
  溢れた分は送り直さない。表示を閉じる・Live を切り替えると、待ち・timer・購読を捨てる。
- Feature Data Classification: Transient / Local Only。runtime event と演出の待ちは保存しない。表示の設定は端末に 1 組だけ保存する（他の利用者へ送らない）。
  docs・wire・SQLite・IndexedDB・backup の形式は変えない。runtime event は CLI の `events.watch` にも流れる（種類が 1 つ増えるだけで、既存の種類の形は変えない）。
- ライブの列への組み込み（AC-3）: 重ねる表示はライブの列（幅 2）の左側に出し、列の entity の Live を対象にする。会話は列の scope。Live の場所が列の scope と
  違うとき（別の場所の紹介 post から開いたとき）だけ、列に Live の場所を持たせ、workspace の保存と URL（`liveTopic`・`liveChannel`）に載せる。旧版はこの項目を
  読み飛ばす（Local Only）。入口は、公開スレッドの詳細の上部の行、ライブの列の一覧のカードの「大きく表示」（列を置き換えて Live を切り替える）、投稿の中の Live の
  「大きく表示」（投稿の場所を会話にした列を開く）。左側の「ライブに返信」は、返信の列をライブの列の子として右に開き、ライブの列を残す。別の場所の Live は、
  右側の一覧で取得待ちとして探さない（どちらも 2026-10-18 のユーザー判断）。
- 需要: 会話の場所は列の購読の需要（既存）で受け、Live の場所が違えば、その場所の需要を列に結び付けて取る（observer は列の id に `#live` を付けたもの。上限で
  取れなくても列は閉じず、その場所のリアクションの新着が届かないだけ）。左側の会話の場所は、表示中の間だけ既存の 3 秒の読み直しに足す（参加していない private
  channel は足さない）。重ねる投稿と開始 post は、成人向けの判定・信頼値の評価・画像の取得の対象に通常の投稿と同じく入れる。列を閉じる・Live を切り替えると、
  これらと演出の待ち・timer を外す。列の下部の参加は、Live の場所の一覧で Live を探す。

### 追補: 投稿添付の画像とGIFを重ねる表示（#1950 AC-1、2026-10-27）

2026-10-27のユーザー判断で、通常表示は複数添付の先頭の画像1枚だけとする。本文の有無にかかわらず描画し、本文・作者・返信の文脈は既存のまま保つ。画像の透明部分をカード背景で塞がず、GIFは元のbytesとMIMEを使って再生する。reduced motionでは表示寸法以内のcanvasへ1フレームだけ描き、元画像の読み込みを解放する。

- Feature 名: Liveの投稿添付overlay
- Durable / Transient: Transient（表示状態のみ）
- Canonical Source / Rebuildable From: 既存のpost headerと添付blob。表示のための写しや再エンコードを作らない
- Replicated?: 表示状態は複製しない。投稿・添付は従来どおり
- Public Replica / Private Replica / Local Only: 表示はLocal Only。会話の既存の閲覧条件を適用する
- Gossip Hint / Blob / SQLite projection: 新規追加なし。既存の投稿media需要・URLとcacheを共用する
- 必須contract: 先頭画像のみ、本文あり/なし、表示gateと需要回収、映像・音声の共有を維持する対象component test
- 必須scenario: 透過PNGと透明色付きanimated GIFの実描画、reduced motion、広幅/狭幅・dark/light・ja/en/zh-CN

投稿レイヤーoff・画面外・背景・終了で画像DOMと表示需要を外す。他の表示面が同じhashを使用していれば、共通ownerがその需要を保持する。画像の準備や失敗はLive映像・音声を待たせない。Pushの一時network eventはAC-2で追加する。

### 追補: 公開スレッドの添付Push（#1950 AC-2、2026-10-27）

2026-10-27のユーザー承認と[#1950](https://github.com/kukuri-app/kukuri/issues/1950)のScope revision `2026-10-27-r3`に従う。通常の画像表示は先頭1枚だけとし、Pushは添付ごとに選ぶ。画像は画面下側へ寄せ、既存のリアクション表示と重ならないように配置する。表示中の同じ投稿・添付への押下は送信者を問わず加算し、右下へ `x${回数}` と表示する。表示が消えた後はx1から始める。

- Feature 名: 公開スレッドの添付Push
- Durable / Transient: Transient。要求は10秒、演出は最後の有効な押下から5秒
- Canonical Source: 署名付きの短命要求。元投稿と添付は既存docs/blobsの正本を参照する
- Replicated?: 既存gossipによる一時配送。永続複製・outbox・ACK・再接続時の再送は追加しない
- Rebuildable From: 復元しない。初回表示や履歴取得で過去のPushを再構成しない
- Public Replica / Private Replica / Local Only: 公開スレッドの既存hint scopeで配送。重複台帳と演出はLocal Only
- Gossip Hint 必要有無: 既存の署名付きevent carrierを共用。下記のwire互換を保つ
- Blob / SQLite projection: 新規の正本なし。Pushが元投稿・添付・日時・reaction数を変更せず、画像bytesを送らない
- 必須contract: 3端末の独立押下、同一eventだけの重複除去、投稿/添付/scope結合、非表示と終了、4096件・期限回収、24署名主体×10回/秒×60秒の保持窓
- 必須scenario: `live-overlay-push`の実アプリ3端末、対象添付のbrowser実操作、GIFの先頭再生と元表示の維持、12のviewport/theme/locale条件

#### wireと互換

新しいgossip enum variantは増やさず、既存の`MetaverseRoomEvent { topic_id, room_id, event }`を署名付き短命eventのcarrierとして使う。`room_id`はこのkindでは公開スレッドIDを表す。`event.kind = "live-overlay-push-v1"`で振り分け、旧版のMetaverse handlerは未知のkindを`Ok(None)`で無視する。未知のvariantによるtransportエラーや、投稿の同期hintとしての再解釈を起こさない。

署名対象contentは`version=1`、押下ごとに生成する32桁hexのnonce、`target`（topic、公開スレッドID、元投稿ID、元の添付配列のindexとhash）、元投稿の作成時刻の手がかり、`sent_at`（ms）。event IDはenvelope IDとし、署名者を送信者とする。topic/公開スレッドとcarrierの一致、署名、version/nonce、10秒の寿命と未来2秒以内を検証する。受理・重複IDの保持・解決の期限は、いずれも`sent_at+10秒`に揃える。古い投稿もキー指定で解決し、署名検証済みprojectionのscope・添付index/hash・image MIME・role、撤回・mute/block・成人向けgateを照合する。合流中も最初のindex/hash結合を変えず、後続要求の不一致を破棄する。信頼値とadvisoryは通常mediaの共通表示判断を使う。要求をblob取得の任意hash指定経路にしない。

#### 所有・上限・回収

- dedupeはaccount runtimeの全体4096件。期限順の索引で回収し、生きたIDを容量都合で追い出さない。満杯は受付不能として通知する。送信側は公開前にIDを予約し、ローカル反映とネットワークから戻る同一要求を二重実行しない。
- 送信中と表示用post解決は各256件まで。受信通知のchannelも256件で、既存runtime/host channelの取りこぼしも明示的な混雑通知へ変換する。bodyの取得待ちでPushが止まらないよう、既存の公開スレッドgossip購読から独立したstreamを読む。子taskは既存leaseと一緒に止め、押下や画像ごとのnetwork購読を作らない。
- 採用CNがある場合は送信者も既存のaccount共通信頼評価で確認してから合流する。未評価・照会失敗は既存のfail-openに従う。未完了照会は全stageで256件までとし、scopeを切り替えても完了まで数える。応答後にも表示状態・scope・要求の期限を確認し、不要になった結果は捨てる。採用CNがなければこの照会は行わない。
- idle時の台帳回収はweak参照を持つaccount単位のtaskが担当する。送信・解決のpermitは完了/取消で返し、解決は要求の残り寿命内だけで行う。初回履歴取得や再接続で台帳を再生しない。
- 描画は各Live表示面で最大6枠。待ちqueueなし、期限timerと描画通知は表示面に各1つ。連打をevent単位で加算して描画だけをまとめる。同じhashのURL・ファイル・blob取得は既存media ownerへ需要を渡す。GIFはcanonical URLを保持したまま表示用fragmentを分け、再登場の先頭再生と表示中の継続を両立する。新しいobject URLを発行しない。
- 表示中の元投稿はobserverごとの`liveStagePosts`に置き、通常のadvisory・trust・media需要へ接続する。同じ投稿を複数Liveで使っても、一方の終了で他方の需要を消さない。隠す・背景・終了・scope切替で演出、timer、listener、追加の投稿需要を外し、遅れて返る解決結果は捨てる。共通cacheは既存の予算と回収規則を保つ。

GUI/Web/CLIが共用するruntime eventへ`live_overlay_push_received`を追加する。既存eventの形は変更しない。保存データ、backup、投稿・Liveのwire、SDK接続の優先順位と映像・音声のbytes/署名/寿命は変えない。

## Consequences
- 映像・音声の搬送は [ADR 0067](0067-media-transport-moq.md) に従う。manifest の descriptor は上の追補による（分類は ADR 0067 の Feature Data Classification）。
- #1262: state/envelopeの到着による個別反映、表示中だけのmanifest取得、未取得候補と検証済みsessionの区別はADR 0052「Session の個別反映と表示要求」に従う。timerによる反映retryはしない。
- #1813: gossipのhintで読めなかったsessionの上限つきの読み直しは、ADR 0052「Session の個別反映と表示要求」の例外（ADR 0054「読み取り候補」）。
- late joiner と restart 後の復元は `docs state + manifest blob` だけで成立しなければならない。
- #1909（2026-10-22）: 公開topicを保持する端末は、現在と前日のbucketのsessionのkeyのうち反映していない版を読み、反映の経路と同じく
  新しい版のstate・署名済みenvelope・manifest blobを保持して返す（ADR 0055 §1.1）。開始・終了・revisionの新旧は既存の規則に従う。
- 終了済み session は durable state で `Ended` として復元され、新規 join を拒否しなければならない。
- 過去に署名された revision を置き直しても、終了済み session の表示と操作は過去へ戻らない。
- viewer count は shared durable state ではなく local projection の TTL 管理で表現される。

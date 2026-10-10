# ADR 0062: QR・専用リンクの移行の招待と、両端末の確認、必須 bundle と任意の投稿の履歴の転送

## Status

Accepted（Issue #1211 W7 AC-1、鍵・設定の転送と保存は AC-2 で §5、任意の投稿の履歴は AC-3 で §6、自動同期への接続は AC-4 で §7 に固定した。画面と既存の鍵の export・backup との対象差の説明は AC-5 で §8 に固定した。Web 版の移行先のカメラでの QR の読み取りは #1628 で §1 に足した。同じ protocol と置き場を使う本人の端末どうしの和集合の同期は #1650 で §9 に固定した）

## Context

利用者が QR コードか専用リンクで 2 台の端末をつなぎ、アカウントと鍵・設定を軽く移したい（#1213 D-13）。
移行は秘密を送るので、送る前に「どの 2 台が本人の端末どうしとしてつながったか」を固定する必要がある。

基準（統合 branch `348cf3d62`）の事実:

- 端末は iroh の endpoint を 1 つ持ち、`Router` に ALPN ごとの protocol を登録している（`crates/iroh-node/src/node.rs`）。接続先の endpoint id は QUIC の認証で確かめられる。
- desktop は `kukuri://` のリンク起動を受ける（`tauri.conf.json` の deep-link）。
- Android は同じ custom scheme を mobile deep-link 設定へ登録し、既存 plugin の cold/warm intent から Ready 後の入力欄へ渡す（#1197 AC-3b）。App Links は採用せず、受信だけで接続・移行・同意を実行しない。チャンネル招待の既存 `kukuri://access-preview?token=...` は同じ登録から既存の確認 Dialog へ入り、明示 Join 前の参加を行わない。
- Web の公開 URL・origin は未定（ADR 0060 §2）。

## Decision

### 1. 画面の流れ（2026-10-02 ユーザー決定）

- 移行元（アカウントを持つ端末）が QR とリンクを表示し、移行先がリンクを開く・貼り付ける。
- 両端末に同じ 6 桁の確認コードを出し、両方で「一致する」を押したときだけ接続が確立する。
- desktop の QR は表示だけで、カメラの読み取りは作らない。desktop はリンクの貼り付けと OS のリンク起動で受ける。リンク起動は入力欄へ入れるだけで、接続は利用者の操作で始める。
- Web 版の移行先は、移行元の QR をカメラで読める（#1628、2026-10-06 ユーザー決定）。QR の中身は §2 の `kukuri://` のリンクで、端末の OS のカメラで読んでも Web 版では開けないため、アプリの中で読む。desktop は上のとおり読み取りを出さない。
  - Android も同じ読み取りを使う（#1193 D1 / #1197 AC-5、2026-10-09 ユーザー決定）。Manifest の CAMERA だけを追加し、明示した読み取りの `getUserMedia`（audio:false）を既存 WebView の OS 権限要求へ接続する。拒否・取消はカメラを使えない結果として貼り付けを残し、独自の撮影・保存・送信経路を作らない。
  - 「QR コードを読み込む」でカメラ（背面を優先）を起動し、映像を 200 ms ごとに読む。移行用のリンク（`kukuri://transfer#`）の QR を認識したら、カメラを止めてリンクを入力欄へ入れるだけで、接続は利用者の操作で始める（リンク起動と同じ）。他の QR は読み飛ばす。
  - 映像の frame はメモリの中の canvas で読むだけで、保存・送信しない。読み取り・「読み込みをやめる」・画面を閉じるのどれでもカメラを止める。
  - カメラを使えない（許可の拒否・カメラ無し・API 無し）ときは理由を示し、貼り付けで受ける。
  - decoder は jsQR（Apache-2.0）で、読み取りを開いたときだけ読み込む。
- 入口は「アカウント追加」ダイアログに「別の端末へ移す」「別の端末から移す」を置く。

### 2. 招待の wire

- リンク: `kukuri://transfer#v1.<base64url(JSON)>`。秘密は fragment にだけ置き、HTTP の query・Referer へ出さない。QR の中身はリンクと同じ。Web の公開 URL が決まったら（W8）、QR とリンクを Web の URL＋同じ fragment に替える。
- JSON（未知の欄は拒否）: 移行元の endpoint id（hex）、relay URL（1 件まで、256 byte まで）、直接の addr（4 件まで）、32 byte の秘密（hex）、期限（ミリ秒）。
- リンクは 1024 byte まで。有効期間は 5 分。アカウントの秘密鍵・チャンネルの秘密・SDP・ICE の情報を載せない。
- 秘密を含む型は `Debug` に中身を出さない（`AccountTransferInvite`、IPC の `AccountTransferLink`・`OpenAccountTransferRequest`）。

### 3. 接続と確認（ALPN `/kukuri/account-transfer/1`）

1. 移行先は招待の endpoint id の端末へ接続する（接続先の束縛）。経路は通常の endpoint と同じで、native どうしは招待の直接 addr（Direct P2P）、relay だけで届く端末は relay を使う。ICE の完了を待たない。
2. 移行先は証明 `blake3::keyed_hash(秘密, "kukuri account transfer hello v1\0" ‖ 移行元の id ‖ 移行先の endpoint id)` を送る。
3. 移行元は、期限内・未使用・証明が正しいときだけ招待を消費して受ける（1 回限り）。期限の正本は移行元が持つ。誤った証明は拒否し、待っている招待は消費しない。
4. 両端末が `blake3::derive_key("kukuri.app 2026-10-02 account transfer code v1", 秘密 ‖ 移行元の id ‖ 移行先の endpoint id)` から 6 桁の確認コードを出す。
5. 両端末が自分の承認（一致する・一致しない）を送り、相手の承認を受ける。両方が「一致する」のときだけ「確認済み」になる。確認の待ちは受けてから 120 秒まで。
6. 確認済みが §5 の転送の唯一の入口になる。期限切れ・改竄・再生・接続先違い・未認証（証明なし・拒否・承認なし）では確認済みにならず、何も送らない。

- 移行は端末ごとに 1 つ。新しい移行・取消・iroh stack の作り直し・停止で前の移行は終わる（stack の作り直しの途中の招待は保持しない）。
- 同時に応じる接続は 2 本まで。超えた接続は待たせずに閉じる。

### 4. 状態

`AccountTransferStatus`（`crates/core/src/account_transfer.rs`）: `idle`・`waiting`（移行元）・`connecting`（移行先）・`confirming`（確認コードと自分の承認の有無）・`transferring`（両端末の承認の後。送った・保存した item の数）・`history`（必須の移行の後、履歴を送っている・受けている。投稿と、移行元に無かった本文・添付の数。§6）・`completed`（移行先は受けたアカウントの ID を持つ。履歴を選んだときはその結果）・`failed`（`expired`・`invalid`・`unreachable`・`rejected`・`cancelled`・`interrupted`・`storage`）。画面は開いている間だけ 500 ms ごとに状態を読み、閉じたら移行を取り消す。AC-1 の `confirmed` は、確認の後に同じ接続で転送を続けるので `transferring` に置き換えた。

### 5. 必須 bundle の転送と保存（AC-2、2026-10-03 ユーザー決定を含む）

- 両端末の承認の後、同じ接続の新しい stream で移行元が frame（長さ 4 byte の後に JSON）を送る: `key`（アカウントの秘密鍵、1 回）→ `items`（0 回以上）→ `end`（送った item の総数）。
  - item は移行元の account の replica（ADR 0061）の docs の key と、その封（§3 の封。AAD はアカウントの公開鍵と docs の key）。種類は W5 の allowlist のうち profile・表示例外・channel の参加・世代の鍵・担当（tombstone を含む）で、変更の窓・投稿・履歴は送らない。
  - フォロー・ブロック（#1211 AC-6、2026-10-04 ユーザー判断）は、replica の `graph/` の木の代わりに、移行元の store から相手の順に読んで送る（AC-6 より前の edge は replica に無いため）。自分のフォロー、自分のブロック、自分への follow（フォロワー）の順に、それぞれ 64 件ずつ送る。`channel/` の item より先に送る。
    - フォロワーは、#1650 から全員を送る（移行先のフォロワーの一覧を移行元とそろえる。2026-10-07 ユーザー判断）。それまでは、移行先が相互フォローを判定して DM を送受信するため、自分がフォローしている相手からの follow だけを、そのフォローと同じ page で送っていた。ミュートは端末ごとのまま送らない（ADR 0022）。
  - 移行元は、送信待ちの行を先に replica へ書き、replica を W5 の周回と同じ prefix の木（1 照会 64 key）で辿る。送り直しは、行の無い旧 profile（W5 AC-3 より前の確定）も行にしてから書く（ADR 0061 §8、#1646）。読む量はアカウントの item の数で、投稿・履歴の量に依らない。
  - 上限: 1 frame は 1 MiB、`items` は 64 件まで。総量では打ち切らない（ユーザー決定）。frame の待ちは 30 秒、確定の ACK の待ちは 60 秒。
- 移行先は frame の大きさ・件数・順序・総数と、各 item の封（受けた鍵から導出した payload の鍵で開く）と種類を確かめる。外れたら確定せず、保存を消して `invalid` にする。
- 保存（staging）: 受けたアカウントの DB の隣の `<db>.account-transfer.json`（manifest）と `<db>.account-transfer-<n>.json`（chunk）。chunk は受けた封のまま置く（秘密は封の中）。置き場はアカウントごとに 1 つで、新しい受信が前の置き場を消す。
- 確定: `end` を受けたら manifest を完了にし、新規のアカウントは鍵の保存と登録簿への追加（host のアカウントの操作と排他。同じ公開鍵が登録済みなら追加しない）を行ってから ACK を返す。移行元は ACK を受けたときだけ `completed`、移行先は確定の後に `completed`。保存できない（容量不足など）ときは `storage` にして接続を閉じ、移行元にも `storage` を示す。ACK が届かなくても移行先の保存は戻さない（やり直しは同じアカウントへの item 単位の merge になる）。
- 反映: 置き場は、そのアカウントの runtime の起動時（使っているアカウントへの移行なら確定の直後）に背景で chunk ごとに W5 の item 単位の merge（`(updated_at, op_id)`）で反映し、反映した chunk から消す。手元の新しい版は古い bundle で戻らない。別のアカウントの runtime は読まない（置き場はアカウントの DB の隣で、封はアカウントの鍵でしか開けない）。
  - 受信を失敗・取消・停止で止めたら置き場を消す（取消・停止は移行の task ごと止めるので、確定も破棄もせずに落とされた保存が消す）。再起動などで残った受信途中（manifest が完了でない）の置き場は反映せず、次の受信かそのアカウントの起動で消す。反映の途中で止まったら、manifest の位置から続ける（消えた chunk は反映済み）。
- 切替（ユーザー決定）: 移行先は完了の画面で「このアカウントを使う」を押したら、受け取ったアカウントへ切り替える（使っているアカウントなら閉じるだけ。§8）。
- 確認済みの直後に片方だけが切断と表示しうる AC-1 の挙動は、同じ接続で転送を続け、移行元が ACK を受けてから閉じる形にして解消した。
- 実装: core の `AccountTransferFrame`・`AccountTransferItem`、iroh-node の `AccountBundleSource`・`AccountBundleSink`、app-api の `account_transfer_page`・`merge_account_transfer_items`、desktop-runtime の `accounts/transfer.rs`。

### 6. 任意の投稿の履歴（AC-3、2026-10-04 ユーザー決定を含む）

- 範囲（ユーザー決定）: 移行先がリンクを貼る画面で選ぶ。移さない（既定）・直近 30 日・直近 1 年・すべて。期間は時間 bucket（UTC の日、ADR 0054 §1）の範囲で、最初の bucket は最初の試行の時刻から決め、続きでは同じ値を使う。「すべて」だけが、時間 bucket へ切り替える前の旧形式（`topic::`・`channel::`・自分の `author::` と、旧形式の移行で守った `own:<id>`、#1221 R5-I）も含む。
- 移すもの（ユーザー決定）: 移行元の保護所有先の自分の record（ADR 0058 §7 の保護参照 `own_docs`。「すべて」は `own:<id>` も）を、replica（bucket locator）・key・docs author・値のまま移す。その日の bucket の自分の記録（投稿・返信・repost・取り下げ・reaction・索引の行など）をすべて含む。account・device の replica と他人の author 領域は送らない。投稿の envelope（`objects/<id>/envelope`）が指す本文（`BlobText`）と添付の blob も送り、移行元に無いものは数える（取得不能）。値が 512 KiB を超える record・読めない record も送らずに同じ数に数える。他の端末で書いて移行元に無い投稿は移らない。
- wire（同じ接続）: 必須の ACK の後、移行先は履歴を選んでいなければ接続を閉じる。選んでいれば新しい stream で `history`（範囲の最初の bucket。すべては無し。続きの位置）を送る。移行元は page ごとに `records`（64 件・1 MiB まで）、本文・添付ごとの `blob`（512 KiB ずつ、0 から順に長さまで）、`page`（次の位置。範囲の終わりは無し。投稿の数・移行元に無かった数）を送り、移行先の保存の ACK（1 byte）を受けてから次の page を読む（送っている page は常に 1 つ）。frame の待ちは 30 秒、ACK の待ちは 60 秒。
- 移行元の page: 最初の page の前に、送信待ちの取り下げを書く（取り下げた投稿を取り下げの record とともに送る）。保護参照の索引（参照・replica・key・docs author の順）を続きの位置の次から読み、範囲より前の bucket と対象外の replica は seek で飛ばして行を読まない。1 page は 64 件・照会 8 回まで（届かなければ、その位置を続きにする）。1 page の照会の数・読む行・bytes と、同時の処理（1 つ）は、選択外の履歴の量に依らない。
- 移行先の置き場: 受けたアカウントの DB の隣の journal（`<db>.account-history.json`。範囲・続きの位置・保存した page の数・反映した page の数）と、page ごとの `<db>.account-history-<n>.json`、blob の部分の `<db>.account-history-<n>-<p>.bin`。record は空でない・NUL を含まない・範囲の replica であることを、blob は順番・長さ・hash を確かめる。page は `page` を受けたら保存して journal を進め、ACK を返す。確定していない page の部分は取消・失敗で消し、再起動で残ったものは次の受信で消す。
- 反映: そのアカウントの runtime の起動時（使っているアカウントなら page の保存の後）に、page ごとに自分の record（`put_owned_record`）と blob（保護参照 `own_blob:<hash>`）として保存し、反映した page から消す。既にある自分の record は上書きしない。範囲の終わりまで反映したら journal も消す。
- 中止・失敗・再開（ユーザー決定）: 履歴の途中の取消（どちらの端末からも）・切断・保存の失敗では、必須の移行は完了のまま、履歴は止まった理由（`stopped`）を結果に持つ。もう一度つないで同じ範囲を選ぶと、journal の続きの位置から受ける（受けた page は送り直さない）。別の範囲は最初から。続きの位置は送り元の端末の索引の位置なので、journal に送り元の端末（endpoint id）を持ち、別の端末からは最初から受ける（#1650）。自動の再接続・一時停止はしない。
- 切替（ユーザー決定）: 移行先は履歴を受ける間は完了にせず、履歴が終わったら（完了・中止・失敗）完了の画面を示す（切替は §8 の「このアカウントを使う」から）。切替は runtime と endpoint を作り直し、移行の接続が切れるので、履歴を受ける間は切り替えない。移行先は履歴を受ける間は Dialog を閉じさせない（「戻る」と閉じるボタンを出さず、Escape・外側のクリックでも閉じない）。履歴を終えるのは「履歴の受け取りをやめる」だけ（2026-10-04 ユーザー決定。閉じる操作で履歴が止まり、切り替わらなかった監査の指摘 B-1 による）。
- 表示: 移行先では、移した投稿は自分のプロフィールに出る（作者の bucket の索引を、手元の自分の record から読む）。topic の timeline は時間 bucket を相手の端末から読むので、移行先の自分の record だけでは出ない（相手の端末が居れば読める）。取り下げは、表示した投稿の背景の確認が投稿の日の bucket を読んで反映する（確認先が旧形式の replica だけだった既存の不具合を、2026-10-04 のユーザー判断でこの AC で直した）。
- 実装: core の `AccountTransferHistory`・`AccountHistoryRecord`・`AccountHistoryCursor` と frame の `history`・`records`・`blob`・`page`、store の `protected_records_after`、iroh-node の `AccountBundleSource::history_page`・`blob_part`・`AccountBundleSink::history`・`AccountHistoryStaging`、desktop-runtime の `accounts/history.rs`、app-api の `schedule_withdrawal_check`。

### 7. 自動同期への接続と、端末ごとに残すもの（AC-4）

- 移行先は、受け取ったアカウントへ切り替えると、その runtime の起動で W5 の account 同期の lease を取る（ADR 0061 §7）。本人の端末の候補は Community Node の rendezvous からだけ得る。そのため、移行先の端末がそのアカウントで node に同意して認証（bearer token の取得）した後に、移行元を見つけて差分の取得を始める（ADR 0061 §10）。再 QR は要らない。同意のダイアログは、node に同意していないアカウントへ切り替えたときの既存のもの。移行の接続先を同期の相手には使わない。
- 端末ごとに残し、移さないもの: endpoint の秘密鍵と端末 ID（担当の判定・変更の窓の ID）、node の設定・同意・token、アプリの同意と年齢の申告（端末の app data）、成人向けの表示の設定、OS の permission。必須 bundle は W5 の allowlist の item だけを運ぶ（§5）。account 由来の docs author（ADR 0053）は、両端末で同じになる。
- 担当（ADR 0018 §8）: 移行は担当の記録をそのまま運び、復元の印を作らない。移行先は、移行だけでは担当にならない（担当は「この端末で行う」の明示の操作か、担当の移譲で移る）。
- 実 iroh の 2 端末で確かめて見つけた W5 の 2 つの不備を、この AC で直した（2026-10-04 ユーザー判断）。
  - 相手の account の replica の読み出しは、手元の保持分で短絡しない（ADR 0061 §10）。本人の端末は同じ docs author なので、手元で書いた item も同じ key・author の保持分になり、相手の新しい版を隠していた。
  - 差分の取得の task と移行の反映は、account の状態を共有する handle（`AppService::account_handle`）で merge する。持ち主ではないので、落としても購読を止めない。それまでは別の lease の表を持つ handle で merge していたため、別の端末から届いた channel の世代の変化が購読とメモリへ反映されず、移行で受けた channel の購読は反映の task が終わると外れていた。
- 実装: docs-sync の `RemoteDocsSource::exact`、app-api の `AppService::account_handle`。試験は desktop-runtime の `account_transfer_sync.rs`（2 端末の実 runtime の切替、loopback の直接経路、rendezvous の在席を返す模擬の node）。

### 8. 画面の説明と、既存の鍵の export・backup との対象差（AC-5、2026-10-04 ユーザー判断）

- 移行の画面は、接続の前（移行元のリンクを出す画面と、移行先のリンクを貼る画面）に、移るもの・移らないもの・移行元のアカウントが残ることを示す。移行先の完了の画面にも、移っていないものを示す。必須の完了と履歴の結果の表示は §5・§6 のまま。
  - 移るもの: アカウント鍵、profile、フォローとブロック（AC-6）、「この作者を常に表示する」の指定、参加中の private channel とその鍵。選んだときは、移行元で書いた自分の投稿の履歴（本文と添付）。
  - 移らないもの: ミュート（ADR 0022）、DM と通知の履歴（ADR 0020・0023）、Community Node の設定と同意、アプリの同意と年齢の申告、成人向けの表示の設定（§7）。
- 切替は画面を読み込み直すので、移行先は完了してもすぐには切り替えず、完了の画面の「このアカウントを使う」で切り替える（2026-10-04 ユーザー判断。完了したらすぐ切り替える旧い形を改めた）。閉じたときは、アカウントの menu から切り替える。
- 移行元のアカウント（登録簿と鍵）は、移行の後も残す。移行は移行元の鍵・登録簿・DB に触れない。
- 設定の「アカウント」は、「アカウントを別の端末へ移す」ボタンで移行元の dialog を開く（#1629、2026-10-06。3 つの方法の差を示す旧い形を改めた。差は ADR 0047 §6）。
- 実装: `apps/desktop/src/components/settings/AccountTransferPanel.tsx` の `TransferScope` と完了の画面、`apps/desktop/src/shell/page/AccountMenu.tsx`、`AccountKeyPanel.tsx`。

### 9. 本人の端末どうしの和集合の同期（#1650、2026-10-07 ユーザー判断）

同じアカウントを使う 2 台の端末で、プロフィール・フォロー・フォロワー・投稿などを、両端末の和集合にそろえる。W5 の自動同期（ADR 0061）が運ばない旧データ（同期の行を作る前の edge）とフォロワー、投稿の記録も合わせる。

- 入口: アカウントメニューの、使用中のアカウントの行の右端（チェックの右）の「プロフィールの端末間同期」（`refresh-cw`）。同期できるのは起動中のアカウントだけなので、使用中でない行には出さない（menu のために他のアカウントの通信を起動しない）。押すと同じ名前の Dialog を開いて待ち受け、閉じると止める。
- 待ち受けと接続: 押した端末は 5 分間待ち受け、本人の端末の候補（account の hint topic の購読の相手と、その topic の rendezvous の応答の本人の端末。ADR 0061 §7）へ、ALPN `/kukuri/account-union/1` でつなぐ。待つ間は rendezvous の応答のたびに、まだつないでいない候補へつなぐ（相手ごとに 1 つ。周期処理を新設しない）。移行と同じ枠（端末ごとに 1 つ）を使い、新しい同期・移行は前のものを取り消す。
- 確かめ方: つないだ端末は、アカウント鍵から導出した鍵（ADR 0061 §1）で両端末の endpoint id と向きに束縛した証明を送る。受けた端末は、待ち受けていて証明が正しいときだけ受けて、自分の証明を返す。つないだ端末は相手の証明を確かめてから送る。どちらの端末も、相手が同じアカウント鍵を持つことを確かめる前に何も送らない。
  - 確認コードは出さない（両端末で利用者が押したことと、同じアカウント鍵を持つことで足りる）。
  - 待ち受けは最初に確かめた 1 台とだけ使う。両端末が互いへつないだときは、endpoint id の小さい端末からの接続を使う（受けた端末は、同じ相手へつないでいる最中で自分の id が小さいときは断る）。
- 送るもの: 両端末が同じ接続で同時に、§5 の必須 bundle から鍵を除いたもの（profile・表示例外・channel の参加と鍵・担当と依頼・参加者、store の自分のフォローとブロック・フォロワー全員）と、§6 の投稿の履歴の「すべて」（本文・添付の blob つき）を、移行と同じ frame で送る。stream は向きごとに 1 本で、bundle の総数の ACK の後に、受ける側が範囲と続きの位置を送り、page ごとに ACK する。
- 取り込み: 移行と同じ置き場と反映（§5・§6。使っているアカウントなので、確定・page の保存の直後に反映する）。item は W5 の item 単位の merge（ADR 0061 §4）で、片方にしか無いものは足し、同じ key は新しい版を採る。同じ相手へのフォロー・ブロックは新しい操作を採る（後で解除していれば解除のまま）。投稿の記録は、既にある自分の record を上書きしない。
- 結果: 受けた item・投稿の数と、相手に無かった本文・添付の数を示す。どちらかの向きが止まったら止まった理由を示し、相手に取り消されて送り終える前に止まったときも、受け終えていても中断として示す（独立監査 B-1）。閉じる側は、どの stream も最後の書込みの受取りを確かめてから閉じ（相手の ACK を読むか、`stopped` を待つ）、iroh の QUIC（noq）は閉じた接続でも受け取り済みの data を読ませるので、送り終えた向きが閉じる順で失敗になることはない（この前提は noq の実装に依る。版を上げるときは、正常な終わりで止まった理由が無いことを判定する試験で確かめる）。投稿の途中で止まったら、同じ相手ともう一度両端末で押すと、続きの位置から受ける（§6）。
- 量: 送る量は、アカウントの item・フォロー・フォロワー・自分の投稿の記録の数に比例する。利用者の明示の操作で両端末の全件を比べるため、毎回すべてを送る（続きの位置は止まったときだけ使う）。読み出しは移行と同じ page（64 件・1 MiB）で背景で進め、画面の操作を待たせない。
- 前提と対象外: 両端末が新しい版で、Community Node に同意して rendezvous で互いを見つけられること（自動同期と同じ）。3 台以上の同時の同期、旧版の端末との同期、同期の後に届いたフォロワーの自動の同期は扱わない。
- 実装: core の `AccountSyncKeys::pairing_proof`、iroh-node の `AccountTransfer::sync`・`sync_peers`（`account_transfer/sync.rs`）、desktop-runtime の `start_account_union_sync`・`account_union_sync_peers`、app-api の `account_transfer_edges_page` のフォロワーの段、store の `list_follow_edges_by_target_after`、画面の `AccountMenu`・`AccountTransferPanel`（`sync` の役）。

## 採らない方式

- 秘密鍵・チャンネルの秘密を QR に直接載せる: 画面を見た人・リンクを受け取った経路に秘密が渡る（#1213 の Non-goals）。
- 招待を中央のサーバーで仲介する: 秘密の移行を中央へ委ねない（#1211 INVAR-2）。接続は既存の P2P の経路を使う。
- 移行元だけの承認: 移行先が別の端末（差し替えられた QR）とつながっても気付けない。両端末の確認で、つながった 2 台が手元の 2 台であることを確かめる。
- リンクを browser の storage に置いて起動時の重複を防ぐ: 秘密を保存することになる。処理済みのリンクは process の中だけで覚える。

## Consequences

- 移行の秘密の転送（AC-2）は、確認済みの接続の上だけで行う。
- 受けたアカウントの鍵・設定は、そのアカウントの runtime が起動するまで置き場に残る（移行先は完了の画面から切り替えるので、通常はすぐ反映する）。
- Web の runtime の組み立て（W1 AC-5）以後、Web も同じ protocol で移行先・移行元になれる（`iroh-node` は wasm32 で build する共用 crate）。

## Data classification

ADR 0002 の template に従う。

- Feature 名: QR・専用リンクの移行の招待と確認、必須 bundle と任意の投稿の履歴の転送
- Durable / Transient: 招待と確認の状態は Transient（移行元・移行先の memory だけ。期限・取消・停止で終わる）。Web / Android の QR の読み取りの映像の frame も Transient（読み取りの間の memory の canvas だけ）。移行先の置き場（staging）は Durable で、反映・失敗・取消・次の受信で消す。履歴の置き場（journal と page）も Durable で、反映した page から消し、範囲の終わりまで反映したら journal も消す（アカウントごとに 1 つ）
- Canonical Source: 移行元の memory の招待（期限・使用済みの正本）。bundle の正本は移行元の account の replica の item。履歴の正本は移行元の保護所有先の自分の record と blob（移した後は移行先の保護所有先にも同じ値を持つ）
- Replicated?: しない。明示した 2 端末の間だけ（§9 の同期は、同じアカウント鍵を持つことを確かめた本人の 2 端末の間だけ）
- Rebuildable From: 再構築しない。やり直すときは新しい招待を出す（置き場は新しい受信で作り直す）
- Public Replica / Private Replica / Local Only: Local Only（置き場は受けたアカウントの DB の隣。中身は封のまま）
- Gossip Hint 必要有無: なし
- Blob 必要有無: 履歴を選んだときだけ、投稿の本文・添付の blob を送る（移行先は自分の blob として保護する）
- SQLite projection 必要有無: なし（反映は W5 の item 単位の merge で、既存の行へ入る）
- 必須 contract: 招待の形式・上限・期限、証明と確認コードの束縛、秘密の `Debug` の非出力、bundle と履歴の frame の上限と item の検証（core の試験）、正例と負例の接続、確定の境界の故障、履歴の中止・失敗・続きの位置（iroh-node の試験）、保存・確定・反映・やり直し・再起動、履歴の page の範囲と読む量・置き場の再開と回収・往復の反映と契約（desktop-runtime の試験）、保護参照の索引の読み出し（store の試験と Web の browser 試験）、移行の後の同意を経た自動同期への接続と、端末ごとに残すもの・担当の分離（desktop-runtime の 2 端末の試験）
- 必須 scenario: Web↔native の往復（W8）
- 新しい外部送信: なし（利用者が選んだ自分の端末との P2P の接続。relay は既存の relay だけ。§9 の同期が Community Node へ送るのは、既存の account の hint topic の rendezvous の不透明な鍵だけ）
- §9 の同期の待ち受けと接続の状態は Transient（memory だけ。期限・取消・停止で終わる）。受けたものの置き場・反映・消し方は移行と同じ

## References

- Issue #1213（D-13・D-14）、#1211（本 ADR）
- ADR 0047・0048・0057・0060・0061

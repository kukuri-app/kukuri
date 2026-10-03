# ADR 0062: QR・専用リンクの移行の招待と、両端末の確認、必須 bundle の転送

## Status

Accepted（Issue #1211 W7 AC-1、鍵・設定の転送と保存は AC-2 で §5 に固定した。履歴は AC-3、自動同期への接続は AC-4、画面と既存の鍵の export・backup との対象差の説明は AC-5）

## Context

利用者が QR コードか専用リンクで 2 台の端末をつなぎ、アカウントと鍵・設定を軽く移したい（#1213 D-13）。
移行は秘密を送るので、送る前に「どの 2 台が本人の端末どうしとしてつながったか」を固定する必要がある。

基準（統合 branch `348cf3d62`）の事実:

- 端末は iroh の endpoint を 1 つ持ち、`Router` に ALPN ごとの protocol を登録している（`crates/iroh-node/src/node.rs`）。接続先の endpoint id は QUIC の認証で確かめられる。
- desktop は `kukuri://` のリンク起動を受ける（`tauri.conf.json` の deep-link）。
- Web の公開 URL・origin は未定（ADR 0060 §2）。

## Decision

### 1. 画面の流れ（2026-10-02 ユーザー決定）

- 移行元（アカウントを持つ端末）が QR とリンクを表示し、移行先がリンクを開く・貼り付ける。
- 両端末に同じ 6 桁の確認コードを出し、両方で「一致する」を押したときだけ接続が確立する。
- desktop の QR は表示だけで、カメラの読み取りは作らない。desktop はリンクの貼り付けと OS のリンク起動で受ける。リンク起動は入力欄へ入れるだけで、接続は利用者の操作で始める。
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

`AccountTransferStatus`（`crates/core/src/account_transfer.rs`）: `idle`・`waiting`（移行元）・`connecting`（移行先）・`confirming`（確認コードと自分の承認の有無）・`transferring`（両端末の承認の後。送った・保存した item の数）・`completed`（移行先は受けたアカウントの ID を持つ）・`failed`（`expired`・`invalid`・`unreachable`・`rejected`・`cancelled`・`interrupted`・`storage`）。画面は開いている間だけ 500 ms ごとに状態を読み、閉じたら移行を取り消す。AC-1 の `confirmed` は、確認の後に同じ接続で転送を続けるので `transferring` に置き換えた。

### 5. 必須 bundle の転送と保存（AC-2、2026-10-03 ユーザー決定を含む）

- 両端末の承認の後、同じ接続の新しい stream で移行元が frame（長さ 4 byte の後に JSON）を送る: `key`（アカウントの秘密鍵、1 回）→ `items`（0 回以上）→ `end`（送った item の総数）。
  - item は移行元の account の replica（ADR 0061）の docs の key と、その封（§3 の封。AAD はアカウントの公開鍵と docs の key）。種類は W5 の allowlist のうち profile・表示例外・channel の参加・世代の鍵・担当（tombstone を含む）で、変更の窓・投稿・履歴は送らない。
  - 移行元は、送信待ちの行を先に replica へ書き、replica を W5 の周回と同じ prefix の木（1 照会 64 key）で辿る。読む量はアカウントの item の数で、投稿・履歴の量に依らない。
  - 上限: 1 frame は 1 MiB、`items` は 64 件まで。総量では打ち切らない（ユーザー決定）。frame の待ちは 30 秒、確定の ACK の待ちは 60 秒。
- 移行先は frame の大きさ・件数・順序・総数と、各 item の封（受けた鍵から導出した payload の鍵で開く）と種類を確かめる。外れたら確定せず、保存を消して `invalid` にする。
- 保存（staging）: 受けたアカウントの DB の隣の `<db>.account-transfer.json`（manifest）と `<db>.account-transfer-<n>.json`（chunk）。chunk は受けた封のまま置く（秘密は封の中）。置き場はアカウントごとに 1 つで、新しい受信が前の置き場を消す。
- 確定: `end` を受けたら manifest を完了にし、新規のアカウントは鍵の保存と登録簿への追加（host のアカウントの操作と排他。同じ公開鍵が登録済みなら追加しない）を行ってから ACK を返す。移行元は ACK を受けたときだけ `completed`、移行先は確定の後に `completed`。保存できない（容量不足など）ときは `storage` にして接続を閉じ、移行元にも `storage` を示す。ACK が届かなくても移行先の保存は戻さない（やり直しは同じアカウントへの item 単位の merge になる）。
- 反映: 置き場は、そのアカウントの runtime の起動時（使っているアカウントへの移行なら確定の直後）に背景で chunk ごとに W5 の item 単位の merge（`(updated_at, op_id)`）で反映し、反映した chunk から消す。手元の新しい版は古い bundle で戻らない。別のアカウントの runtime は読まない（置き場はアカウントの DB の隣で、封はアカウントの鍵でしか開けない）。
  - 受信を失敗・取消・停止で止めたら置き場を消す（取消・停止は移行の task ごと止めるので、確定も破棄もせずに落とされた保存が消す）。再起動などで残った受信途中（manifest が完了でない）の置き場は反映せず、次の受信かそのアカウントの起動で消す。反映の途中で止まったら、manifest の位置から続ける（消えた chunk は反映済み）。
- 切替（ユーザー決定）: 移行先は完了したら、受け取ったアカウントへ切り替える（使っているアカウントなら切り替えない）。
- 確認済みの直後に片方だけが切断と表示しうる AC-1 の挙動は、同じ接続で転送を続け、移行元が ACK を受けてから閉じる形にして解消した。
- 実装: core の `AccountTransferFrame`・`AccountTransferItem`、iroh-node の `AccountBundleSource`・`AccountBundleSink`、app-api の `account_transfer_page`・`merge_account_transfer_items`、desktop-runtime の `accounts/transfer.rs`。

## 採らない方式

- 秘密鍵・チャンネルの秘密を QR に直接載せる: 画面を見た人・リンクを受け取った経路に秘密が渡る（#1213 の Non-goals）。
- 招待を中央のサーバーで仲介する: 秘密の移行を中央へ委ねない（#1211 INVAR-2）。接続は既存の P2P の経路を使う。
- 移行元だけの承認: 移行先が別の端末（差し替えられた QR）とつながっても気付けない。両端末の確認で、つながった 2 台が手元の 2 台であることを確かめる。
- リンクを browser の storage に置いて起動時の重複を防ぐ: 秘密を保存することになる。処理済みのリンクは process の中だけで覚える。

## Consequences

- 移行の秘密の転送（AC-2）は、確認済みの接続の上だけで行う。
- 受けたアカウントの鍵・設定は、そのアカウントの runtime が起動するまで置き場に残る（移行先は完了で切り替えるので、通常はすぐ反映する）。
- Web の runtime の組み立て（W1 AC-5）以後、Web も同じ protocol で移行先・移行元になれる（`iroh-node` は wasm32 で build する共用 crate）。

## Data classification

ADR 0002 の template に従う。

- Feature 名: QR・専用リンクの移行の招待と確認、必須 bundle の転送
- Durable / Transient: 招待と確認の状態は Transient（移行元・移行先の memory だけ。期限・取消・停止で終わる）。移行先の置き場（staging）は Durable で、反映・失敗・取消・次の受信で消す
- Canonical Source: 移行元の memory の招待（期限・使用済みの正本）。bundle の正本は移行元の account の replica の item
- Replicated?: しない。明示した 2 端末の間だけ
- Rebuildable From: 再構築しない。やり直すときは新しい招待を出す（置き場は新しい受信で作り直す）
- Public Replica / Private Replica / Local Only: Local Only（置き場は受けたアカウントの DB の隣。中身は封のまま）
- Gossip Hint 必要有無: なし
- Blob 必要有無: なし
- SQLite projection 必要有無: なし（反映は W5 の item 単位の merge で、既存の行へ入る）
- 必須 contract: 招待の形式・上限・期限、証明と確認コードの束縛、秘密の `Debug` の非出力、bundle の frame の上限と item の検証（core の試験）、正例と負例の接続、確定の境界の故障（iroh-node の試験）、保存・確定・反映・やり直し・再起動（desktop-runtime の試験）
- 必須 scenario: Web↔native の往復（W8）
- 新しい外部送信: なし（利用者が選んだ自分の端末との P2P の接続。relay は既存の relay だけ）

## References

- Issue #1213（D-13・D-14）、#1211（本 ADR）
- ADR 0047・0048・0057・0060・0061

# ADR 0062: QR・専用リンクの移行の招待と、両端末の確認

## Status

Accepted（Issue #1211 W7 AC-1。鍵・設定の転送は AC-2、履歴は AC-3、自動同期への接続は AC-4、画面と既存の鍵の export・backup との対象差の説明は AC-5）

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
6. 確認済みが AC-2 の転送の唯一の入口になる。期限切れ・改竄・再生・接続先違い・未認証（証明なし・拒否・承認なし）では確認済みにならず、何も送らない。

- 移行は端末ごとに 1 つ。新しい移行・取消・iroh stack の作り直し・停止で前の移行は終わる（stack の作り直しの途中の招待は保持しない）。
- 同時に応じる接続は 2 本まで。超えた接続は待たせずに閉じる。

### 4. 状態

`AccountTransferStatus`（`crates/core/src/account_transfer.rs`）: `idle`・`waiting`（移行元）・`connecting`（移行先）・`confirming`（確認コードと自分の承認の有無）・`confirmed`・`failed`（`expired`・`invalid`・`unreachable`・`rejected`・`cancelled`・`interrupted`）。画面は開いている間だけ 500 ms ごとに状態を読み、閉じたら移行を取り消す。

## 採らない方式

- 秘密鍵・チャンネルの秘密を QR に直接載せる: 画面を見た人・リンクを受け取った経路に秘密が渡る（#1213 の Non-goals）。
- 招待を中央のサーバーで仲介する: 秘密の移行を中央へ委ねない（#1211 INVAR-2）。接続は既存の P2P の経路を使う。
- 移行元だけの承認: 移行先が別の端末（差し替えられた QR）とつながっても気付けない。両端末の確認で、つながった 2 台が手元の 2 台であることを確かめる。
- リンクを browser の storage に置いて起動時の重複を防ぐ: 秘密を保存することになる。処理済みのリンクは process の中だけで覚える。

## Consequences

- 移行の秘密の転送（AC-2）は、確認済みの接続の上だけで行う。
- Web の runtime の組み立て（W1 AC-5）以後、Web も同じ protocol で移行先・移行元になれる（`iroh-node` は wasm32 で build する共用 crate）。

## Data classification

ADR 0002 の template に従う。

- Feature 名: QR・専用リンクの移行の招待と確認
- Durable / Transient: Transient（招待と確認の状態は移行元・移行先の memory だけ。期限・取消・停止で終わる）
- Canonical Source: 移行元の memory の招待（期限・使用済みの正本）
- Replicated?: しない。明示した 2 端末の間だけ
- Rebuildable From: 再構築しない。やり直すときは新しい招待を出す
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: なし
- Blob 必要有無: なし
- SQLite projection 必要有無: なし
- 必須 contract: 招待の形式・上限・期限、証明と確認コードの束縛、秘密の `Debug` の非出力（core の試験）、正例と負例の接続（iroh-node の試験）
- 必須 scenario: Web↔native の往復（W8）
- 新しい外部送信: なし（利用者が選んだ自分の端末との P2P の接続。relay は既存の relay だけ）

## References

- Issue #1213（D-13・D-14）、#1211（本 ADR）
- ADR 0047・0048・0057・0060・0061

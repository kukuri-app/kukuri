# ADR 0034: Community Node の案件保持と legal hold

- Status: Accepted
- Date: 2026-08-25
- Issue: #763

**2026-10-09 改訂**（#1705）: §3 の起動時の処理を、件数に依存しない形へ改めた。当初は既存行を API listen 前に sealing し、実装は起動のたびに全暗号文を認証していたため、起動の処理量と待ちが暗号文と通報・申出の行数に比例していた。起動時は暗号文 1 行で鍵を照合し、改ざんは読むときに検出し、旧平文の確認は migration の 1 回だけにする。起動時の sealing と全件の認証は失効した。旧平文が残る DB は migration で止まり、#776 以後・#1705 より前の版で一度起動して sealing してから更新する。

## Context

Community Node は通報、権利侵害申出、moderation artifact、operator audit を Postgres に保持するが、従来は接続ログ30日・モデレーションログ180日という生成文書上の集約値しかなく、案件データの期限切れ非表示、物理削除、復元後整理、案件限定の legal hold が無かった。発信者情報開示へ備えるために新しい常時ログを集めるのではなく、実際に取得した情報だけを通常保持と法的保全に分けて扱う必要がある。

## Feature Data Classification

- Feature 名: Community Node 案件保持・legal hold
- Durable / Transient: Durable。期限到来後は通常読取から除外し、active hold が無ければ物理削除する
- Canonical Source: 当該 node の Postgres
- Replicated?: No
- Rebuildable From: No。Postgres backup は期限内データと active hold 対象を含む
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 無
- Blob 必要有無: 無。本人確認資料・証拠ファイルの upload は行わない
- SQLite projection 必要有無: 無
- 必須 contract: 期限境界、通常非表示、物理削除、hold 対象限定、解除後削除、暗号化、redacted export、復元後 cleanup
- 必須 scenario: 書込みなしの時刻経過と復元後の初回 cleanup

## Decision

### 1. 区分と既定保持期間

| 区分 | 既定 | 起算点 | 保存先・削除 |
|---|---:|---|---|
| 接続ログ | 30日 | 観測時刻 | 各ログ保存先。期限削除 |
| 通報本体 | 180日 | 受付時刻 | `cn_admin.reports`。期限削除 |
| 通報者連絡先 | 90日 | 受付時刻 | 暗号化した sensitive item。期限削除 |
| 未解決の権利侵害申出本体 | 730日 | 最終更新時刻 | `cn_legal.rights_requests`。状態更新時に再計算 |
| 解決済みの権利侵害申出本体 | 365日 | 終了時刻 | `actioned`。期限削除 |
| 却下等の権利侵害申出本体 | 180日 | 終了時刻 | `declined / out_of_scope / withdrawn`。期限削除 |
| 申出者連絡先 | 180日 | 受付時刻 | 暗号化した sensitive item。申出本体より先に削除可 |
| 本人・代理権確認情報 | 180日 | 受付または追加時刻 | 暗号化した sensitive item |
| 証拠参照 | 180日 | 受付または追加時刻 | 暗号化した sensitive item |
| 判断・通知履歴 | 365日 | 記録時刻 | `cn_legal.rights_request_events`。申出本体を外部キーで参照せず、本体とは別の期限で消す。本体の物理削除は履歴の残りを待たない（#1706） |
| operator audit | 365日 | 操作時刻 | `cn_admin.operator_actions`。機微本文を複製しない |
| signed moderation event | 180日 | 永続化時刻 | `cn_safety.signed_moderation_events` |
| risk signal | 180日 | 永続化時刻 | `cn_safety.risk_signals`。より早い `expires_at` を優先。著者の対応（`cn_safety.risk_signal_subject_authors`）は、その内容の risk signal の行が無くなった時点で同じ取引で消し、risk signal の行が無い内容には作らない（#1699） |
| legal hold の記録 | 解除まで | 開始時刻 | `cn_legal.legal_holds`。解除で行を消す。開始・解除・export の記録は operator audit に残る（#1706） |

保持日数は `RetentionConfig` で正の有限日数として変更できる。既存の `moderation_logs_days` は後方互換の集約表示として残すが、削除判定は event、signal、audit の個別値を正とする。

通報から risk signal までの区分は、期限を行に持たず、上表の起算点の列と、保持区分ごとの日数（`cn_admin.retention_days`）で判定する（#1704。接続ログと legal hold の記録は対象外）。権利侵害申出は、状態の区分（未解決・措置済み・却下等）と最終の状態遷移の時刻で判定する。日数は cn-user-api の起動時と `cn-cli retention sweep` が operator config から書くだけなので、日数を変えると既存の行にもすぐ効き（短縮すれば期限切れ、延長すれば未削除の行が再び読める）、行は書き直さない。以前の、行ごとの期限の列を起動時と毎時に全行で書き直す方式は、処理量が全行数に比例するので廃止した。

未解決案件は補正・調査・発信者照会が継続し得るため730日、措置済み案件は判断説明と送達履歴の確認のため365日、却下・範囲外・取下げはデータ最小化を優先して180日とする。未解決案件を無期限にはしない。継続保全が必要なら案件限定 hold を開始する。

### 2. 通常期限と legal hold を分離する

legal hold は `report` または `rights_request` の既存 ID と、列挙済みデータ区分を対象にする。開始根拠、開始時刻、終了条件、actor を持ち、全 DB または wildcard 対象を許さない。解除で hold の行を消し、解除の時刻・actor・区分は operator audit の解除の記録に残す（#1706）。

active hold は対象行の物理削除だけを止める。期限切れデータは hold 中でも公開状態照会、通常 API、通常 admin 一覧へ返さず、権限付き export のみが参照できる。解除時点で期限切れなら次回 cleanup で削除する。

hold の開始・解除・export は append-only operator audit に、対象 ID、区分、actor、時刻だけを記録する。申出本文、連絡先、本人確認、証拠を audit payload に複製しない。

### 3. 機微区分を暗号化して分離する

通報者連絡先、申出者の氏名・組織・住所・email・電話・代表される権利者、代理権根拠、証拠参照は、用途分離した `COMMUNITY_NODE_LEGAL_DATA_KEY` による XChaCha20-Poly1305 で暗号化する。AAD に owner kind、owner ID、data category を束縛する。private channel secret の鍵は再利用しない。

受付 capability が有効なのに鍵が無い・弱い場合、暗号文が改ざんされている場合、旧平文が残っている場合は fail-closed とし、平文 fallback を行わない。

- 起動時は API listen 前に暗号文を 1 行だけ復号して、設定した鍵を照合する。誤鍵なら起動しない。暗号文が無ければ照合しない。鍵が無く暗号文があれば起動しない。暗号文を書くのは照合を通った鍵だけなので、全行が同じ鍵で書かれている。
- 改ざんは各行を読むとき（運営者の管理画面・cn-cli・hold export）に AEAD の認証で検出し、その読取りをエラーにする。
- #776 より前に平文で保存された `reporter_contact` と権利申出 JSON の機微フィールドは、#776 以後の版の起動時に sealing 済みとする。旧平文の確認は migration（`202610090006_legacy_plaintext_guard.sql`）が 1 回だけ行い、残っていれば適用を止める。`cn_admin.reports.reporter_contact` 列は削除した。

### 4. 期限切れを読取と定期処理の両方で強制する

全通常読取は「起算点 > 基準時刻 − 保持期間（`cn_admin.retention_interval(区分)`）」を条件にし、期限切れを返さない。起動時は listen 前に cleanup を実行し、その後は固定間隔で cleanup する。cleanup は明示的な基準時刻を受ける純粋な契約境界を持ち、子行から親行の順で削除する。

backup は database 全体を含み得るため、backup object 自体の lifecycle と、復元 DB の論理保持を区別する。復元後は API 公開前の cleanup を必須とし、期限切れ非 hold 行を再表示しない。

### 5. export は allowlist に限定する

hold export は案件と対象区分から明示 DTO を組み立てる。秘密鍵、暗号鍵・nonce、JWT、認証 token、追跡 secret hash、private channel secret、private message、無関係な案件は出力しない。保有していない pubkey、IP address、port、接続時刻、投稿 ID を推測・補完しない。

## Consequences

- operator は保持設定と専用暗号鍵を配備し、hold の開始・終了を案件単位で管理する必要がある。
- 機微情報は通常 DB dump に暗号文として含まれる。復元には同じ鍵が必要だが、期限切れは復元後 cleanup で非表示・削除される。
- 期限切れ hold 対象は通常運用画面に見えず、法的保全 export だけで扱う。

## Out of Scope

- 発信者特定のための新しい通信監視・ログ項目
- 本人確認資料・証拠ファイルの upload と blob 保管
- 法的判断、意見聴取、外部提出の自動化

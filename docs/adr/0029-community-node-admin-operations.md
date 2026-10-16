# ADR 0029: Community Node admin operations

## Status

Accepted

案件データの保持、legal hold、限定 export は [ADR 0034](0034-community-node-case-retention-legal-hold.md) を正とする。hold の開始・解除・export は既存 admin 認証と CSRF 保護を通し、機微本文を含めない append-only audit を残す。

## Context

Community Node の admin Web surface は GCP IAP TCP forwarding と IAM を公開境界にし、
稼働状態、supported topic、通報、readiness、Cloud Logging 導線を read-only で提供している。
運用時の定型操作を browser から行うには、CLI と同じ runtime state を更新しつつ、誰が何を
変更したかを改変不能な形で残し、cross-site request と意図しない即時適用を防ぐ必要がある。

一方、provider credential、LLM endpoint、capability、image revision は deployment state であり、
runtime DB の運用 state ではない。これらを admin process へ預けたり browser から直接変更すると、
`operator-config.yaml`、Terraform、secret manager、readiness activation の責任境界を迂回する。

## Decision

### Browser から適用できる操作

admin listener では、runtime Postgres が canonical source である次の操作だけを扱う。

- admission mode の変更
- public supported topic の追加・除去
- 受信済み report の状態変更
- malware の report からの投稿・添付の blob hash の制限と、その制限の解除（#1062、ADR 0032 §6）
- 異議申し立て中のリスク判定の認容・棄却・検知情報調整・訂正版再発行

各操作は `preview -> confirm -> apply` の二段階にする。apply 時にも入力を再検証し、対象 state の
変更と append-only audit row の追加を同じ Postgres transaction で commit する。

異議申し立て審査は `COMMUNITY_NODE_SAFETY_OPERATOR_REVIEW` も明示的に有効な場合だけ変更できる。
異議申し立て通報は対象のリスク判定へ関連付け、同じ判定への複数通報を一つの審査対象として表示する。
認容は `Disputed` から `Cleared` へ移して関連通報を `actioned` にし、信頼評価への寄与を除外する。
棄却は `Disputed` から `None` へ戻して関連通報を `dismissed` にし、寄与を維持する。検知情報の
調整と訂正版再発行は署名済みモデレーション事象と利用者の正本状態を変更しない。

### Browser から適用しない操作

次は引き続き reviewed `operator-config.yaml` / Terraform / secret management / `cn-cli readiness`
workflow でのみ変更する。

- provider / LLM / Project Arachnid の endpoint と credential
- capability availability / authority scope
- image digest / deployment revision
- private channel secret
- invite code、allowlist、ban のように秘密値または subscriber identity を扱う操作

admin UI は上記の設定状態と責任境界を表示してよいが、値や credential を表示・保存・変更しない。

### Actor と認可

- listener 自体は public user API と分離し、GCP firewall と IAP TCP forwarding / IAM の内側だけで公開する。
- browser write は `COMMUNITY_NODE_ADMIN_ACTOR` が非空で設定されたときだけ有効にする。
- audit の actor はこの deployment-controlled 値を使い、form input や任意 HTTP header は信用しない。
- actor 未設定時は dashboard を read-only に保ち、write endpoint は fail-closed にする。

IAP TCP forwarding は HTTP identity header を注入しないため、初期運用では deployment actor を
記録する。将来 HTTPS IAP が authenticated principal を保証できるようになった場合は、検証済み
principal へ置き換えられる。任意 header だけを根拠に actor を切り替えてはならない。

### CSRF と確認

- process 起動時に十分な entropy を持つ CSRF token を生成し、admin state のみに保持する。
- state-changing endpoint は POST のみとし、hidden token の一致を必須にする。
- preview は変更前後、影響、actor を表示し、apply は明示的な confirm action からのみ到達させる。
- apply は preview を信用せず、現在値と入力を再取得・再検証する。
- 異議申し立て審査は preview 時の判定情報と関連通報状態を確認情報に含め、apply 時に一項目でも
  変化していれば古い確認として拒否する。

### Audit

`cn_admin.operator_actions` は append-only とし、次を保存する。

- action id / occurred_at / actor
- action / target kind / target id
- before / after JSON

DB trigger で UPDATE / DELETE を拒否する。秘密、credential、report details、reporter contact、
private channel capability は audit payload に含めない。

異議申し立て審査の操作記録には、判定状態、分類、深刻度、確信度、公開範囲、失効時刻、関連通報の
識別子と状態だけを変更前後として保存する。申し立て本文、申立者の識別情報、連絡先は保存しない。

malware の制限・解除の操作記録（`malware.restrict_post` / `malware.release_post`、対象は投稿。
`malware.restrict_blob` / `malware.release_blob`、対象は添付の blob hash）には、
通報の識別子と状態、判断の識別子・根拠・措置 capability・解除時刻・運営者が入力した解除理由だけを
保存する（#1062）。通報の本文と連絡先は保存しない。

### Feature Data Classification

- Feature 名: Community Node admin operations and audit
- Durable / Transient: operation result と audit は Durable、CSRF token と preview は Transient
- Canonical Source: runtime state と audit は Community Node Postgres、deployment state は `operator-config.yaml` / Terraform / secret manager
- Replicated?: しない
- Rebuildable From: runtime state は既存 operator workflow、audit は再構築不可
- Public Replica / Private Replica / Local Only: node operator private state（IAP 内部のみ）
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要
- 必須 contract: actor 未設定 fail-closed、CSRF 拒否、preview と apply の再検証、異議申し立ての発行元検証、mutation と audit の transaction 原子性、audit UPDATE / DELETE 拒否、secret・申し立て本文・連絡先の非記録
- 必須 scenario: Postgres integration で admission / supported topic / report status / 異議申し立て審査と audit の同時反映を確認。production は IAP tunnel 経由の read/preview と harmless no-op apply を smoke する

## Consequences

- 日常的な runtime 操作は admin UI で確認と監査を伴って実行できる。
- deployment / credential の変更は browser write の対象外で、既存の review と readiness gate を維持する。
- IAP TCP forwarding 中は actor が deployment 単位であり、個人単位の IAM principal ではない。この制約は UI と runbook に表示する。
- admin listener を public Caddy / DNS へ載せてはならない。

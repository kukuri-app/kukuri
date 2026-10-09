# Community Node Operator Docs Generator (`cn-operator`)

最終更新日: 2026-09-02

## 目的

community node 運営者が `operator-config.yaml` を単一の入力元として、有効化した機能に対応した
運営者向け文書群（利用規約 / プライバシーポリシー / 外部送信表示 / 電気通信届出補助資料 /
ネットワーク構成説明 / server manifest）を**決定論的に**生成できるようにする。

これは `cn-operator`（`crates/cn-operator`）の実装手順であり、`docs/architecture/p2p-first-community-node-responsibility-boundary.md`
の責任境界を共通前提とする。community node を中央 SNS 運営者にするためのものではなく、
P2P network の補助層を個人・小規模グループでも説明可能に運用できるようにするためのもの。
そもそも P2P 基盤上には network 全体を統治する中央権者が構造的に存在しないため、生成文書も
node 単位の説明責任に閉じる。

## cn-cli との責務境界

`cn-operator` は `operator-config.yaml` を真実源に、deploy 前の宣言、文書・manifest・Terraform 変数の生成、設定 drift と safety readiness の検証を行う。Postgres へ接続して稼働中 node の状態を変更する command は提供しない。

稼働中 node の migration、auth rollout、通報確認、入会制御、supported set、indexing request、relation 解析、法的な送信防止には `cn-cli` を使う。運用入口と起動順は [`dev.md`](dev.md#cn-cli-と-cn-operator-の役割) を参照する。

## 法的な送信防止

権利侵害等の判断は node ごとの authority scope に閉じる。決定には、basis category、対象 capability、判断者、判断時刻、任意の期限・関連 report ID を必ず記録する。

```bash
# 自 node の index/search/discovery/recommendation から対象投稿を除外
cn-cli transmission-prevention apply \
  --actor operator@example.net \
  --subject-kind post \
  --subject-id <object-id> \
  --basis copyright \
  --capabilities community-index,search,discovery,recommendation \
  --related-report-id <report-id>

# 現在有効な判断を確認
cn-cli transmission-prevention status --subject-kind post --subject-id <object-id>

# 解除。既存 index は自動復活せず、fresh ingest 後にだけ再評価される
cn-cli transmission-prevention release \
  --actor operator@example.net \
  --subject-kind post \
  --subject-id <object-id> \
  --reason "claim resolved"
```

適用は Postgres index truth の削除と ArcadeDB projection の除外を同じ運用操作として行う。再起動・再 ingest 時も active decision を本文・media fetch より先に確認する。公開 status は `GET /v1/transmission-preventions/{subject_kind}/{subject_id}` で確認でき、異議申立先として当該 node の `POST /v1/report` を案内する。

これは network-wide deletion ではない。別 node、Direct P2P、既に他 peer が保持する copy は対象外であり、利用者向け説明でも「ネットワーク全体から削除」と表現してはならない。`features.blob_cache=true` は対象 blob の eviction と再 cache 拒否 backend が必要であり、現配布物では未接続のため `cn-cli readiness` が fail-closed で停止する。backend が接続されるまでは blob cache を無効にする。

## relation distance opt-out policy

`COMMUNITY_NODE_INDEX_QUERY_ENABLED=true` または `COMMUNITY_NODE_TRUST_READ_ENABLED=true` の
node は、`COMMUNITY_NODE_RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY` を `(0, 1]` の範囲で
明示設定する。pairwise proximity がこの値未満、または未観測の場合を距離境界外とする。

- user本人がdistance opt-outを有効化した場合だけ、境界外の相手とのuser / post surfacingを
  当該node内で相互に抑制する。双方とも未選択なら距離だけで自動抑制しない。
- これはコミュニティ間の大規模な対立を避けるための表示選択であり、privacy、block、
  relation graphからの離脱、P2P・別node・network全体での不可視性を保証しない。
- 既存の`cn_trust.relation_optouts`行は新しいdistance opt-out選択としてそのまま再解釈する。
  schema migrationや行削除は不要で、rollback時も行を消さない。
- policy値を変更したらdeployment revisionを更新し、`cn-cli readiness`を再実行する。
  起動中の値と本人の選択状態は`GET /v1/relation/optout`で確認できる。

## Phase A / Phase B（宣言と実行可能の分離）

`cn-operator` は各機能を capability として扱い、`availability` を持たせている。

- **Phase A (`Available`)**: 現行 community node 実装（auth / consent / bootstrap / topic
  rendezvous / iroh relay / `report_endpoint` / `community_index` / `moderation` /
  `community_local_trust`）またはデプロイ構成（Cloudflare / analytics / crash report /
  blob cache / private message storage / push）として提供できる capability。
  生成文書では「運用中」として開示してよい。`report_endpoint` は `POST /v1/report` で
  通報を受信・保存し、`cn-cli reports` で運営者が確認できる。
- **Phase B (`Planned`)**: 将来追加され、まだ現行配布物に実装されていない capability。
  config で宣言できても、生成文書では「計画中（この配布物では未提供）」として分離し、
  運用中の外部送信・データ取扱い開示には載せない。2026-08-06 時点の定義には該当項目がない。

将来 Phase B capability が追加された場合、それを有効化するには config に
`acknowledge_planned_capabilities: true` を明示する必要がある。現行3機能の有効化には不要だが、
実体のない「運用中」開示を生成しない。これは現行schemaの説明であり、未提供capabilityの実装や将来向けの汎用化を文書生成作業の完了条件へ追加しない。

## サブコマンド

```bash
# サンプル config を出力
cn-operator init --out operator-config.yaml

# config を検証（将来の Planned capability 承認ガードを含む）
cn-operator validate-config --config operator-config.yaml

# 文書群を生成
cn-operator generate-docs --config operator-config.yaml --out-dir dist/operator-docs

# terraform.tfvars を生成（deploy セクションが必要・#380）
cn-operator generate-tfvars --config operator-config.yaml --out infra/terraform/envs/low-cost/terraform.tfvars

# 生成済み文書と config の drift、および secret ID / private endpoint 混入を検出
# （いずれかがあれば non-zero exit）
cn-operator check-disclosures --config operator-config.yaml --out-dir dist/operator-docs
```

cargo から直接実行する場合:

```bash
cargo run -p kukuri-cn-operator --bin cn-operator -- generate-docs \
  --config operator-config.yaml --out-dir dist/operator-docs
```

## profile

`profile` は features の既定値を与え、個別の `features` キーで上書きできる。

- `minimal`: index / moderation / community-local trust を含め、任意 capability は既定で無効。
  relay / cache / analytics / crash report も無効。
- `relay-enabled`: minimal + 専用 iroh relay + 暗号化済み traffic fallback 開示。
- `full-service`: relay-enabled + blob cache + push 通知 + report endpoint。analytics /
  crash report は任意（既定無効）。

## deploy セクション（terraform env 生成・#380）

operator-config.yaml に任意の `deploy:` セクションを追加すると、同じ config を単一の入力元として
`cn-operator generate-tfvars` が `infra/terraform/envs/low-cost` 用の `terraform.tfvars` を生成できる。
未指定なら従来通り docs / manifest のみを生成する（後方互換）。

```yaml
deploy:
  profile: low-cost        # コスト/データ階層の軸（capability 軸の profile とは別物）
  project_id: your-gcp-project
  region: asia-northeast1
  zone: asia-northeast1-a
  relay_domain: iroh-relay.example-kukuri.net   # low-cost では必須
  acme_email: ops@example-kukuri.net
  jwt_secret_id: kukuri-cn-jwt-secret           # secret の値ではなく Secret Manager の ID
  postgres_password_secret_id: kukuri-cn-postgres-password
  machine_type: e2-small
  disk_size_gb: 30
  postgres_data_disk_gb: 0
  blob_cache_size_gb: 0    # blob cache の on/off は features.blob_cache が真実源
  backup_enabled: true
```

注意点:

- `deploy.profile` は `low-cost` / `managed-db` / `ha` の **コスト/データ階層の軸**で、上の
  capability 軸 profile（`minimal` / `relay-enabled` / `full-service`）とは独立。
- secret は **値ではなく Secret Manager の secret ID** のみを書く。tfvars にも値は出力されない。
- blob cache の on/off は `features.blob_cache` を真実源にする。`features.blob_cache: false` なのに
  `deploy.blob_cache_size_gb > 0` を指定すると `validate-config` で失敗する。
- api hostname は `server.domain` から導出する。`low-cost` は relay container を常に配置するため
  `deploy.relay_domain` が必須。その他 profile でも `iroh_relay` capability が有効なら必須。
- tfvars 生成は現状 `low-cost` のみ対応。`managed-db` / `ha` は拡張点で、指定すると
  `generate-tfvars` が error にする（docs / manifest 生成自体は可能）。

生成した `terraform.tfvars` は `operator_config_path = "operator-config.yaml"` を併用することで、
operator-config.yaml を VM へ配置し public manifest endpoint / report_endpoint gating を有効化できる
（`docs/runbooks/community-node-gcp-terraform.md` 参照）。

## 生成される文書

```text
dist/operator-docs/
  server-manifest.json          # 型付き manifest schema（下記参照）
  network-diagram.md
  telecom-notification-draft.md
  service-description-draft.md
  terms.md
  privacy-policy.md
  external-transmission-notice.md
  abuse-policy.md
  moderation-policy.md
  data-retention-policy.md
  rights-infringement-policy.md
  prior-consultation-email.md
```

各文書には「法的助言ではない」旨の注記が含まれる。最終判断は運営者自身および総合通信局・
専門家への確認が必要。

## 法務文書の版管理と公開

`legal` を設定した Node は、`server.contact`、15個の `retention.*_days`、7文書すべての
slug、正の整数 version、ISO 形式の施行日、対応 renderer である正文言語 `ja` または
`en`、required を明示する。法務文書を
コード上の保持期間既定値で公開することはできない。利用規約とプライバシーポリシーだけを
required とし、残りは公開開示文書として扱う。

```yaml
server:
  operator_name: KingYoSun
  contact: ops@kukuri.app

legal:
  identity_disclosure_request: "運営主体の氏名・住所が必要な場合は、利用目的を添えて ops@kukuri.app へ請求してください。遅滞なく回答します。"
  documents:
    - { kind: terms, slug: terms_of_service, version: 1, effective_date: 2026-09-02, language: ja, required: true }
    - { kind: privacy, slug: privacy_policy, version: 1, effective_date: 2026-09-02, language: ja, required: true }
    - { kind: external_transmission, slug: external_transmission, version: 1, effective_date: 2026-09-02, language: ja, required: false }
    - { kind: moderation_policy, slug: moderation_policy, version: 1, effective_date: 2026-09-02, language: ja, required: false }
    - { kind: abuse_policy, slug: abuse_policy, version: 1, effective_date: 2026-09-02, language: ja, required: false }
    - { kind: data_retention, slug: data_retention, version: 1, effective_date: 2026-09-02, language: ja, required: false }
    - { kind: rights_infringement, slug: rights_infringement_policy, version: 1, effective_date: 2026-09-02, language: ja, required: false }

manifest:
  rights_request_initial_response_target_days: 7

retention:
  connection_logs_days: 30
  moderation_logs_days: 180
  report_days: 180
  report_contact_days: 90
  tester_feedback_days: 180
  rights_request_active_days: 730
  rights_request_resolved_days: 365
  rights_request_rejected_days: 180
  rights_request_contact_days: 180
  rights_request_identity_days: 180
  rights_request_evidence_days: 180
  rights_request_history_days: 365
  operator_audit_days: 365
  moderation_event_days: 180
  risk_signal_days: 180
```

Node 固有の事実で型付き設定にない説明は `supplemental_markdown` に記載できる。これは生成された
typed descriptor の事実を上書きせず、「運営者による補足」として正文末尾へ追加される。

正文言語は文書ごとに `language` で選ぶ。現行 renderer は `ja` と `en` を提供し、対応しない言語は
別言語の本文を誤って正文として公開せず validation で停止する。`en` を選んだ場合も、capability の
構造化事実と operator retention から正文を生成する。

参考訳は正文の同じ項目へ追加し、正文 version と `translation_of_version` を一致させる。訳本文だけを
直す場合は正文 version ではなく翻訳自身の `revision` を増やす。

```yaml
    - kind: terms
      slug: terms_of_service
      version: 2
      effective_date: 2026-10-01
      language: ja
      required: true
      supplemental_markdown: "問い合わせ受付時間は平日10:00-17:00です。"
      translations:
        - language: en
          revision: 1
          translation_of_version: 2
          title: Terms of Service
          body_markdown: |-
            This is a reference translation of the Japanese authoritative text.
```

### 文書 version を上げるときの注意（snapshot と再同意）

`legal.documents[].version` は `policy_snapshot_revision` の canonical 入力である。required でない
文書（例: `moderation_policy`）の version を 1 → 2 に上げるだけでも snapshot が変わり、全 required
文書の同意が旧 snapshot 扱いになって client は再提示する。version の更新は他の法務変更と同じ
反映にまとめ、再同意の回数を増やさない。

- moderation-policy の文言（#1054: nsfw / objectionable を content advisory 付きで索引し、trust には
  寄与しない旨）はコード側の `gen_moderation_policy` で生成されるため、本文の更新自体に version は
  要らない。本番 operator-config の `moderation_policy` を `version: 2` にする反映は、利用規約
  第3条 4 項の改訂（#1056）と同時に行い、再同意を 1 回にまとめる。
- `safety.moderation.general_action` は未指定（既定 `label`）なら canonical 入力に現れず snapshot を
  変えない。明示的に `hold` / `exclude` へ厳格化する設定は legal 上の扱いの変更として snapshot を
  変える（再同意を伴う前提で行う）。

`cn-user-api` は同じ config から全7正文と参考訳を Postgres へ同期し、認証不要の
`GET /v1/policies?language=en` で現行文書を配信する。要求した同一正文 version の参考訳が無ければ
正文を返し、`fallback` / `requested_language` / `authoritative_language` で明示する。公開済み正文は
`GET /v1/policies/{slug}/revisions` と `GET /v1/policies/{slug}/revisions/{version}?language=en`
から取得できる。表示用 version が同じでも snapshot が変われば新しい immutable revision として追記し、
旧正文と旧同意を上書き・削除しない。厳密な過去版検証には
`GET /v1/policies/{slug}/snapshots/{policy_snapshot_revision}?language=en` を使う。各 response は
`publication_status`、`published_at`、`effective_date`、`retired_at`、前後の version／snapshotを返す。
version rollback と、同じ snapshot identity のまま本文・正文 metadata を変更する構成は起動失敗となる。
過去の固定英語 placeholder も legacy revision として保持し、その同意を現行正文へ引き継がない。

法務上意味のある config と typed descriptor から catalog 共通の `policy_snapshot_revision` が自動生成
される。operator が「再同意が必要か」を選ぶ項目はない。snapshot が変わると、本文 version が同じ
文書を含め全 required 文書について旧 snapshot の同意は満たさず、client は再提示する。accept 中に
current snapshot が変わった場合は `409 POLICY_SNAPSHOT_CHANGED` となり、再取得後にだけ受諾できる。

公開確認:

```bash
curl https://api.kukuri.app/v1/policies
curl 'https://api.kukuri.app/v1/policies?language=en'
curl https://api.kukuri.app/v1/policies/terms_of_service/revisions
curl 'https://api.kukuri.app/v1/policies/terms_of_service/revisions/2?language=en'
curl 'https://api.kukuri.app/v1/policies/terms_of_service/snapshots/<policy_snapshot_revision>?language=en'
curl https://api.kukuri.app/.well-known/kukuri/community-node.json
curl https://api.kukuri.app/terms
curl https://api.kukuri.app/privacy
```

desktop client は接続前に locale 付きで `/v1/policies` を取得し、slug / version / 施行日 / 言語 /
本文と正文 fallback を表示する。ローカル記録の snapshot が current と完全一致する場合だけ認証・
server 同意同期・session 継続を許可する。snapshot なしの旧記録、変更、取得失敗、metadata 不備は
fail-closed とし、当該 Node への認証・登録を開始しない。snapshot 未対応の旧 Node だけは従来の
slug/version 判定を維持する。

`cn-user-api` と `cn-cli retention` の実運用では `COMMUNITY_NODE_OPERATOR_CONFIG` を必須とし、
その保持日数を DB の保持区分ごとの日数（`cn_admin.retention_days`）へ書く。期限の判定と cleanup は
この日数を使うので、法務表示と同じ明示 retention になる。`cn-cli rights-requests`・`cn-cli legal-hold` は
期限を計算しないので、operator config を読まない（ADR 0034 §1、#1704）。

### ブロック・ミュート観測の提供（任意文書、#1061）

`community_local_trust` を提供する Node は、利用者からブロック・ミュートの観測を受け付ける場合だけ
任意文書 `trust_observation_sharing` を公開する（ADR 0026 §8.5）。slug は固定で、`required: false`
以外は設定検証で拒否する。公開しない Node は `POST /v1/trust/observations` に
`TRUST_OBSERVATION_SHARING_NOT_OFFERED`（404）を返し、client は提供の選択肢を出さない。

```yaml
    - { kind: trust_observation_sharing, slug: trust_observation_sharing, version: 1, effective_date: 2026-09-18, language: ja, required: false }
```

- 文書を追加すると `policy_snapshot_revision` が変わり、既存利用者は必須文書の再同意が必要になる。
- 本文（送信項目・提供先・利用目的・保持期間・取消）は operator 設定から生成する。保持期間は active 180 日 /
  revoked 30 日で、定期 cleanup が削除する。
- 閲覧者向けの評価 parameter は `COMMUNITY_NODE_TRUST_RELATION_*` / `_BLOCK_STRENGTH` / `_MUTE_STRENGTH` /
  `_HIDE_THRESHOLD` / `_EVALUATION_TTL_SECONDS` で変更できる（既定値は ADR 0026 §8.2 / §8.4）。

## server-manifest.json

`server-manifest.json` は型付きの共有スキーマ（`kukuri_cn_operator::CommunityNodeManifest`）として
定義される。public manifest endpoint や client（dependency 表示 / report routing /
consent UI）が同じ型を共有して扱えるようにするためのもの。主な構造:

- `node_role`: `default-onboarding-node` / `community-node` / `relay-assist` / `index-node` /
  `moderation-node` / `trust-signal-node`。未指定なら有効 capability から推定（既定 `community-node`）。
  default onboarding node は明示することで third-party community node と区別できる。
- `capabilities`: 全 capability の有効・無効。
- `capability_scope`: `available_enabled`（Phase A）/ `planned_enabled`（Phase B）を分離。
- `authority_scope`: `applies_to`（有効 capability から導出 + operator が `additional_applies_to`
  で拡張可能）/ `does_not_apply_to`（安全な default。operator が上書き可能）。
- `p2p_boundary`: identity / profile / social graph / content truth source / network-wide
  authority をすべて `false` 宣言。これは kukuri の P2P-first 設計の不変条件であり、operator は
  変更できない（community node を home server / central operator と誤解させないため）。network-wide
  authority は P2P 基盤上に構造的に存在し得ないため、この宣言を `true` にすることはそもそも許されない。

config からの設定例:

```yaml
manifest:
  node_role: default-onboarding-node
  authority_scope:
    additional_applies_to:
      - custom_scope
    does_not_apply_to: null   # 未指定なら安全な default
```

## public manifest endpoint

稼働中の community node (`cn-user-api`) は、生成した manifest を unauthenticated な
public endpoint から配信できる。`COMMUNITY_NODE_OPERATOR_CONFIG` に `operator-config.yaml`
のパスを設定すると、起動時に config を読み込み・検証して `CommunityNodeManifest` を構築し、
以下で配信する。

```text
GET /.well-known/kukuri/community-node.json
GET /v1/node/manifest
```

- unauthenticated で取得できる。`cn-operator` と同じ生成ロジック・型を共有するため、
  生成文書の `server-manifest.json` と endpoint response の schema drift が起きない。
- `Cache-Control: public, max-age=300` を付与し、client が cache できる。
- private secret / provider credential は含まれない（manifest は operator config 由来で
  秘密情報を持たない）。
- `COMMUNITY_NODE_OPERATOR_CONFIG` 未設定なら両 endpoint は `404`（`{"error":"manifest_not_configured"}`）。
  この場合 client は default node / kukuri project へ fallback せず、別 node または直接 P2P 経路を使う。
- config を指しているのに読込・検証に失敗した場合は起動時に失敗する（設定ミスを黙って無視しない）。

## 決定論性と CI

出力は wall-clock に依存せず、version は config 由来（`manifest.manifest_version`）。
同じ config からは同じ出力が得られる。CI では `check-disclosures` で生成済み文書と config の
drift を検出できる。

## 関連

- `crates/cn-operator`（本実装）、manifest authority scope / public manifest endpoint /
  capability 別リスクガイド
- `docs/architecture/p2p-first-community-node-responsibility-boundary.md`

//! kukuri community node operator docs generator (#352)。
//!
//! operator config (`operator-config.yaml`) を単一の入力元として、運営者向けの
//! 利用規約・プライバシーポリシー・外部送信表示・電気通信届出補助資料・server manifest を
//! 決定論的に生成する。
//!
//! `Availability::Available` は現行実装で提供できる capability、`Planned` は将来追加される
//! 未提供 capability の予約区分である。現時点の全 capability は提供中へ昇格済みで、生成文書は
//! operator が実際に有効化したものだけを「運用中」として扱う。

pub mod capability;
pub mod capability_risk;
pub mod config;
pub mod deploy;
pub mod docs;
mod docs_moderation_policy;
mod docs_trust_observation_sharing;
pub use docs_trust_observation_sharing::TRUST_OBSERVATION_SHARING_SLUG;
pub mod drift;
pub mod manifest;
pub mod moderation_config;
pub mod policy_catalog;
pub mod policy_descriptor;
pub mod profile;
pub mod retention_config;
pub mod safety_config;
pub mod safety_readiness;

pub use capability::{Availability, Capability, CapabilityMeta, ExternalDestination};
pub use capability_risk::CapabilityRiskPractices;
pub use config::{
    DeployConfig, DeployProfile, LegalConfig, LegalDocumentConfig, LegalDocumentKind,
    OperatorConfig, ReferenceTranslationConfig, ResolvedConfig, ServerConfig, load_and_validate,
    parse_config, resolve_and_validate,
};
pub use deploy::generate_tfvars;
pub use docs::{GeneratedFile, generate_all};
pub use drift::{DriftReport, check_drift};
pub use manifest::{
    AuthorityScope, AuthorityScopeOverride, Capabilities, CapabilityScope, CommunityNodeManifest,
    LegalDocumentManifestEntry, ManifestFeatures, ManifestRetention, NodeRole, P2pBoundary,
    build_manifest, manifest_value, render_manifest,
};
pub use policy_catalog::{
    GeneratedLegalDocument, generate_legal_documents, generate_legal_documents_from_files,
    policy_snapshot_revision,
};
pub use policy_descriptor::{
    CapabilityPolicyDescriptor, PolicyBillingPath, PolicyDataClass, PolicyEffectScope,
    PolicyProcessing, PolicyPurpose, PolicyRetentionRef, PolicyRightsRequestPath,
    PolicySafetyAction, PolicyUsageCondition,
};
pub use profile::Profile;
pub use retention_config::RetentionConfig;
pub use safety_config::{
    GeneralAction, ProviderHosting, SafetyConfig, SafetyErrorAction, SafetyEventsConfig,
    SafetyIndexingConfig, SafetyProviderEntry, SafetyProvidersConfig, SafetyStorageConfig,
    safety_config_warnings,
};
pub use safety_readiness::{
    PUBLIC_NODE_PROFILE, READINESS_CHECK_IDS, RUNTIME_CHECK_IDS, ReadinessCheck, ReadinessReport,
    ReadinessStatus, apply_runtime_checks, evaluate_public_node_readiness,
};

/// `operator init` が出力するサンプル config。
pub const SAMPLE_CONFIG: &str = r#"server:
  domain: example-kukuri.net
  operator_name: Example Operator
  country: JP
  cloud_provider: AWS
  region: ap-northeast-1
  contact: abuse@example-kukuri.net
  # 公開ノード情報の node_id。モデレーションを提供する場合は、モデレーション事象の発行元識別子
  # (署名鍵の公開鍵 hex。`cn-cli moderation issuer-node-id` で導出)と同じ値を記入する。
  # 異議申し立ては node_id と risk signal の issuer_node_id が一致する場合だけ受理される。
  node_id: 0000000000000000000000000000000000000000000000000000000000000000

legal:
  identity_disclosure_request: "運営主体の氏名・住所が必要な場合は、利用目的を添えて abuse@example-kukuri.net へ請求してください。"
  documents:
    - kind: terms
      slug: terms_of_service
      version: 1
      effective_date: 2026-09-02
      language: ja
      required: true
    - kind: privacy
      slug: privacy_policy
      version: 1
      effective_date: 2026-09-02
      language: ja
      required: true
    - kind: external_transmission
      slug: external_transmission
      version: 1
      effective_date: 2026-09-02
      language: ja
    - kind: moderation_policy
      slug: moderation_policy
      version: 1
      effective_date: 2026-09-02
      language: ja
    - kind: abuse_policy
      slug: abuse_policy
      version: 1
      effective_date: 2026-09-02
      language: ja
    - kind: data_retention
      slug: data_retention
      version: 1
      effective_date: 2026-09-02
      language: ja
    - kind: rights_infringement
      slug: rights_infringement
      version: 1
      effective_date: 2026-09-02
      language: ja

# profile が features の既定値を与える。個別の features キーで上書きできる。
profile: relay-enabled

features:
  community_index: true
  moderation: true
  community_local_trust: true
  report_endpoint: true
  rights_request_endpoint: true
  tester_feedback: true
  iroh_relay: true
  traffic_relay_fallback: true
  private_message_storage: false
  blob_cache: false
  analytics: false
  crash_report: false
  cloudflare_proxy: true
  dome_hosting: false

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

safety:
  profile: public-node
  policy_version: 2026-06-public-node-v1
  indexing:
    index_before_scan: false
    on_scan_error: hold
  storage:
    permanent_blob_storage: false
  events:
    emit_signed_moderation_events: true
    # moderation event の実鍵署名（secp256k1）に使う signing key の Secret Manager secret ID。
    # 値ではなく ID のみ。runtime は COMMUNITY_NODE_SAFETY_SIGNING_KEY として注入される。
    signing_key_secret_id: kukuri-cn-safety-signing-key
  providers:
    # known_csam は public-node readiness の必須 provider。本番では実際の
    # known-CSAM provider 名と secret ID を設定する。`project-arachnid-shield`（#391）は
    # operator 自身の Project Arachnid Shield credentials を env で与える。
    known_csam:
      provider: project-arachnid-shield
      required: true
      credential_secret_id: kukuri-cn-safety-known-csam
    # general / unknown_csam は任意。下記は本番値ではない placeholder。
    # 実運用では実際の provider 名に置き換える。
    general:
      provider: placeholder-general-moderation
      required: false
    unknown_csam:
      provider: placeholder-unknown-csam
      required: false
  # 非決定論的 moderation（ADR 0028）。general_action は nsfw / objectionable の suspected の扱い
  # （label = content advisory 付きで索引（既定）/ hold / exclude。allow は受理しない。#1051）。
  # 未指定なら label のまま法務 snapshot は変わらない。provider entry の on_high_confidence は
  # deprecated（受理するが読み捨てて警告）。
  moderation:
    operator_review: true
    # general_action: label

manifest:
  manifest_version: v1
  # 法務文書へ表示するため、コード既定値へ委ねず明示する。
  rights_request_initial_response_target_days: 7
  # node_role 未指定なら有効 capability から推定する（既定: community-node）。
  # default onboarding node の場合は明示する:
  #   node_role: default-onboarding-node
  # authority_scope:
  #   additional_applies_to: []        # 導出された applies_to に追加する項目
  #   does_not_apply_to: null          # 未指定なら安全な default を使う

# terraform デプロイ用の env 設定（#380, 任意）。指定すると
# `cn-operator generate-tfvars` が同じ config から terraform.tfvars を生成できる。
# 未指定なら docs / manifest のみを生成する（後方互換）。profile は low-cost / managed-db / ha
# の **コスト/データ階層の軸**で、上の profile（capability 軸）とは別物。
# secret は値ではなく Secret Manager の ID のみを書く。blob cache の on/off は
# features.blob_cache が真実源（ここには sizing のみ）。
# deploy:
#   profile: low-cost
#   project_id: your-gcp-project
#   region: asia-northeast1
#   zone: asia-northeast1-a
#   relay_domain: iroh-relay.example-kukuri.net   # low-cost では必須
#   acme_email: ops@example-kukuri.net
#   jwt_secret_id: kukuri-cn-jwt-secret
#   postgres_password_secret_id: kukuri-cn-postgres-password
#   machine_type: e2-medium
#   disk_size_gb: 30
#   postgres_data_disk_gb: 0
#   blob_cache_size_gb: 0
#   backup_enabled: true
#   # index / moderation stack（cn-indexer + ArcadeDB + relation 定期解析。#615, 任意）。
#   # secret は値ではなく Secret Manager の ID のみ。credential / 鍵の実値は VM 起動時に取得される。
#   deploy_indexer_stack: true
#   cn_indexer_image: ghcr.io/kukuri-app/kukuri-cn-indexer:latest
#   arcadedb_image: arcadedata/arcadedb:26.8.1
#   indexer_data_disk_gb: 10
#   relation_analyze_interval_minutes: 60   # 1〜90
#   indexer_retention_days: 30              # 必須。保持期間 T（3 以上）
#   indexer_capacity_rows: 1000000          # 必須。容量 B（行数）
#   channel_secret_key_secret_id: kukuri-cn-channel-secret-key
#   legal_data_key_secret_id: kukuri-cn-legal-data-key
#   arcadedb_password_secret_id: kukuri-cn-arcadedb-password
#   arachnid_username_secret_id: kukuri-cn-arachnid-username
#   arachnid_password_secret_id: kukuri-cn-arachnid-password
#   vlm_api_base_url: http://192.0.2.10:8000   # self-host は private tunnel 経由の到達を推奨
#   vlm_model: inclusionAI/SingGuard-2b
#   vlm_response_format: guard
#   vlm_api_key_secret_id: kukuri-cn-vlm-api-key   # 無認証 self-host endpoint なら省略

# 将来 Planned capability が追加された場合に、spec としての記述を明示承認する互換項目。
# 現時点で sample が有効にする capability はすべて提供中。
acknowledge_planned_capabilities: true
"#;

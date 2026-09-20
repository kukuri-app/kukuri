//! operator config (`operator-config.yaml`) の schema と検証。
//!
//! このファイルは CLI と server manifest / 生成文書の単一の入力元である。

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::capability::{Availability, Capability};
use crate::manifest::{AuthorityScopeOverride, NodeRole};
use crate::profile::Profile;
use crate::retention_config::{RetentionConfig, validate_retention};
use crate::safety_config::{SafetyConfig, validate_safety_config};

/// `operator-config.yaml` の生表現。
///
/// `features` は未指定キーを許容し、profile の既定値で補完する。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub profile: Option<Profile>,
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub safety: Option<SafetyConfig>,
    #[serde(default)]
    pub manifest: ManifestConfig,
    /// 利用者へ公開する Node 固有法務文書。未指定の legacy config は生成互換だけを維持し、
    /// versioned consent catalog としては公開しない。
    #[serde(default)]
    pub legal: Option<LegalConfig>,
    /// terraform デプロイ用の env 設定（#380）。
    ///
    /// 指定すると `cn-operator generate-tfvars` が operator-config を単一の入力元として
    /// terraform.tfvars を生成できる。未指定なら従来通り docs / manifest のみを生成する
    /// （後方互換）。コスト/データ階層の軸（low-cost / managed-db / ha）であり、capability 軸
    /// （`profile` / `features`）とは独立。
    #[serde(default)]
    pub deploy: Option<DeployConfig>,
    /// Phase B（未実装 / 計画中）capability を有効化することを明示的に承認する。
    ///
    /// これが false のまま Planned capability を有効化すると検証で失敗する。
    /// 実体のない「運用中」開示を生成しないためのガード。
    /// #617 の昇格後、現時点で計画中の capability は無い（既存 config の後方互換のため
    /// フィールド自体は受理し続ける。将来の Phase B 追加時に再び効く）。
    #[serde(default)]
    pub acknowledge_planned_capabilities: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegalDocumentKind {
    Terms,
    Privacy,
    ExternalTransmission,
    ModerationPolicy,
    AbusePolicy,
    DataRetention,
    RightsInfringement,
    /// ブロック / ミュート観測の提供に対する任意同意（ADR 0026 §8.5、#1061）。
    /// 公開は任意で、`ALL` には含めない。
    TrustObservationSharing,
}

impl LegalDocumentKind {
    pub const ALL: [Self; 7] = [
        Self::Terms,
        Self::Privacy,
        Self::ExternalTransmission,
        Self::ModerationPolicy,
        Self::AbusePolicy,
        Self::DataRetention,
        Self::RightsInfringement,
    ];

    pub const fn filename(self) -> &'static str {
        match self {
            Self::Terms => "terms.md",
            Self::Privacy => "privacy-policy.md",
            Self::ExternalTransmission => "external-transmission-notice.md",
            Self::ModerationPolicy => "moderation-policy.md",
            Self::AbusePolicy => "abuse-policy.md",
            Self::DataRetention => "data-retention-policy.md",
            Self::RightsInfringement => "rights-infringement-policy.md",
            Self::TrustObservationSharing => "trust-observation-sharing.md",
        }
    }

    pub const fn public_path(self) -> &'static str {
        match self {
            Self::Terms => "terms",
            Self::Privacy => "privacy",
            Self::ExternalTransmission => "external-transmission",
            Self::ModerationPolicy => "moderation-policy",
            Self::AbusePolicy => "abuse-policy",
            Self::DataRetention => "data-retention",
            Self::RightsInfringement => "rights-infringement-policy",
            Self::TrustObservationSharing => "trust-observation-sharing",
        }
    }

    pub const fn title_ja(self) -> &'static str {
        match self {
            Self::Terms => "Community Node 利用規約",
            Self::Privacy => "Community Node プライバシーポリシー",
            Self::ExternalTransmission => "Community Node 外部送信表示",
            Self::ModerationPolicy => "Community Node モデレーションポリシー",
            Self::AbusePolicy => "Community Node Abuse ポリシー",
            Self::DataRetention => "Community Node データ保持ポリシー",
            Self::RightsInfringement => "Community Node 権利侵害申出ポリシー",
            Self::TrustObservationSharing => "Community Node ブロック・ミュート観測の提供",
        }
    }

    pub const fn title_en(self) -> &'static str {
        match self {
            Self::Terms => "Community Node Terms of Service",
            Self::Privacy => "Community Node Privacy Policy",
            Self::ExternalTransmission => "Community Node External Transmission Notice",
            Self::ModerationPolicy => "Community Node Moderation Policy",
            Self::AbusePolicy => "Community Node Abuse Policy",
            Self::DataRetention => "Community Node Data Retention Policy",
            Self::RightsInfringement => "Community Node Rights Request Policy",
            Self::TrustObservationSharing => "Community Node Block and Mute Observation Sharing",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegalDocumentConfig {
    pub kind: LegalDocumentKind,
    pub slug: String,
    pub version: i32,
    pub effective_date: String,
    pub language: String,
    #[serde(default)]
    pub required: bool,
    /// 型付き事実を上書きしない、operator 固有の補足。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supplemental_markdown: Option<String>,
    /// この正文 version に厳密に結び付く参考訳。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub translations: Vec<ReferenceTranslationConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceTranslationConfig {
    pub language: String,
    pub revision: i32,
    pub translation_of_version: i32,
    pub title: String,
    pub body_markdown: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegalConfig {
    /// 運営主体の氏名・住所が必要な場合の請求方法としてそのまま公開する日本語文。
    pub identity_disclosure_request: String,
    pub documents: Vec<LegalDocumentConfig>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub domain: String,
    pub operator_name: String,
    /// ISO 3166-1 alpha-2（例: JP）。
    pub country: String,
    #[serde(default)]
    pub cloud_provider: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    /// abuse / 問い合わせ連絡先。未指定なら domain から導出する。
    #[serde(default)]
    pub contact: Option<String>,
    /// 公開ノード情報の `node_id`。モデレーション事象の発行元識別子(署名鍵の x-only 公開鍵 hex、
    /// 署名無効時は `COMMUNITY_NODE_SAFETY_ISSUER_NODE_ID`)と一致させる必要がある(#706)。
    /// `safety` 節があり `features.moderation` が有効な設定では必須。
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub node_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestConfig {
    /// node role。未指定なら profile / 有効 capability から推定する。
    #[serde(default)]
    pub node_role: Option<NodeRole>,
    /// manifest / policy version。決定論的出力のため config 由来とする。
    #[serde(default = "default_manifest_version")]
    pub manifest_version: String,
    /// authority scope を operator が明示的に拡張・上書きするための設定。
    /// 未指定なら applies_to は有効 capability から導出し、does_not_apply_to は安全な default を使う。
    #[serde(default)]
    pub authority_scope: AuthorityScopeOverride,
    /// 権利侵害申出に対する初回応答の運用目標。法定期限を表さない。
    #[serde(default = "default_rights_request_initial_response_target_days")]
    pub rights_request_initial_response_target_days: u32,
}

impl Default for ManifestConfig {
    fn default() -> Self {
        Self {
            node_role: None,
            manifest_version: default_manifest_version(),
            authority_scope: AuthorityScopeOverride::default(),
            rights_request_initial_response_target_days:
                default_rights_request_initial_response_target_days(),
        }
    }
}

fn default_manifest_version() -> String {
    "v1".to_string()
}

fn default_rights_request_initial_response_target_days() -> u32 {
    7
}

/// terraform deployment profile（コスト/データ階層の軸）。
///
/// cn-operator の capability profile（`Profile`: minimal / relay-enabled / full-service）とは
/// **別物**。こちらはインフラのコスト/データ階層を選ぶ軸。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeployProfile {
    /// 単一 VM 上で node-local Postgres / Valkey を動かす個人・小規模 operator の入口。
    #[default]
    LowCost,
    /// Cloud SQL + Memorystore（拡張点。tfvars 生成は未対応）。
    ManagedDb,
    /// HA DB/cache + object storage（拡張点。tfvars 生成は未対応）。
    Ha,
}

impl DeployProfile {
    pub fn key(self) -> &'static str {
        match self {
            DeployProfile::LowCost => "low-cost",
            DeployProfile::ManagedDb => "managed-db",
            DeployProfile::Ha => "ha",
        }
    }
}

fn default_deploy_profile() -> DeployProfile {
    DeployProfile::LowCost
}

fn default_region() -> String {
    "asia-northeast1".to_string()
}

fn default_zone() -> String {
    "asia-northeast1-a".to_string()
}

fn default_cn_user_api_image() -> String {
    "ghcr.io/kukuri-app/kukuri-cn-user-api:latest".to_string()
}

fn default_cn_iroh_relay_image() -> String {
    "ghcr.io/kukuri-app/kukuri-cn-iroh-relay:latest".to_string()
}

fn default_cn_cli_image() -> String {
    "ghcr.io/kukuri-app/kukuri-cn-cli:latest".to_string()
}

fn default_cn_indexer_image() -> String {
    "ghcr.io/kukuri-app/kukuri-cn-indexer:latest".to_string()
}

fn default_arcadedb_image() -> String {
    "arcadedata/arcadedb:26.8.1".to_string()
}

fn default_machine_type() -> String {
    // Postgres + Valkey + ArcadeDB(JVM) + cn-indexer の同居を想定した既定（#615）。
    // API / relay のみの最小構成なら e2-small へ下げてもよい。
    "e2-medium".to_string()
}

fn default_relation_analyze_interval_minutes() -> u32 {
    60
}

fn default_disk_size_gb() -> u32 {
    30
}

fn default_blob_cache_ttl_hours() -> u32 {
    24
}

fn default_blob_cache_path() -> String {
    "/var/lib/kukuri/blob-cache".to_string()
}

fn default_backup_enabled() -> bool {
    true
}

fn default_backup_retention_days() -> u32 {
    30
}

fn default_rate_limit_enabled() -> bool {
    true
}

fn default_rate_limit_per_second() -> u32 {
    10
}

fn default_rate_limit_burst() -> u32 {
    30
}

/// terraform デプロイ用の env 設定（#380）。
///
/// secret は **値ではなく Secret Manager の secret ID** のみを持つ（payload は terraform に
/// 渡さない）。blob cache の on/off は `features.blob_cache` を真実源とし、ここでは sizing
/// （size / ttl / path）のみを持つ。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeployConfig {
    #[serde(default)]
    pub moderation: crate::moderation_config::ModerationDeployConfig,
    /// deployment profile（既定 low-cost）。
    #[serde(default = "default_deploy_profile")]
    pub profile: DeployProfile,
    /// GCP project ID（必須）。
    pub project_id: String,
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default = "default_zone")]
    pub zone: String,
    /// cn-iroh-relay の公開 hostname。low-cost profile では常に必須。
    /// それ以外でも iroh_relay 有効時は必須。
    /// api hostname は `server.domain` から導出する。
    #[serde(default)]
    pub relay_domain: Option<String>,
    /// ACME(Let's Encrypt) 登録 email（必須）。
    pub acme_email: String,
    /// IAP 内部 admin browser write の append-only audit に記録する actor。
    /// 空なら admin UI は read-only で、write endpoint は fail-closed する。
    #[serde(default)]
    pub admin_actor: String,
    /// true なら Cloud DNS の既存 zone に A レコードを作成する。
    #[serde(default)]
    pub manage_cloud_dns: bool,
    /// Cloud DNS managed zone 名（manage_cloud_dns=true のとき必須）。
    #[serde(default)]
    pub dns_zone_name: Option<String>,
    #[serde(default = "default_cn_user_api_image")]
    pub cn_user_api_image: String,
    #[serde(default = "default_cn_iroh_relay_image")]
    pub cn_iroh_relay_image: String,
    #[serde(default = "default_cn_cli_image")]
    pub cn_cli_image: String,
    /// COMMUNITY_NODE_JWT_SECRET を保持する Secret Manager secret ID（必須・値ではない）。
    pub jwt_secret_id: String,
    /// Postgres password を保持する Secret Manager secret ID（必須・値ではない）。
    pub postgres_password_secret_id: String,
    #[serde(default = "default_machine_type")]
    pub machine_type: String,
    #[serde(default = "default_disk_size_gb")]
    pub disk_size_gb: u32,
    /// Postgres data 用の専用 persistent disk サイズ（GB）。0 なら boot disk 上の docker volume。
    #[serde(default)]
    pub postgres_data_disk_gb: u32,
    /// blob cache 専用ディスクサイズ（GB）。0 なら専用ディスクなし。
    /// `features.blob_cache=false` のときに > 0 を指定すると検証で失敗する。
    #[serde(default)]
    pub blob_cache_size_gb: u32,
    #[serde(default = "default_blob_cache_ttl_hours")]
    pub blob_cache_ttl_hours: u32,
    #[serde(default = "default_blob_cache_path")]
    pub blob_cache_path: String,
    #[serde(default = "default_backup_enabled")]
    pub backup_enabled: bool,
    #[serde(default = "default_backup_retention_days")]
    pub backup_retention_days: u32,
    #[serde(default = "default_rate_limit_enabled")]
    pub rate_limit_enabled: bool,
    #[serde(default = "default_rate_limit_per_second")]
    pub rate_limit_per_second: u32,
    #[serde(default = "default_rate_limit_burst")]
    pub rate_limit_burst: u32,
    /// index / moderation stack（cn-indexer + ArcadeDB + relation 定期解析。#615）を配備するか。
    ///
    /// 既定 false（従来の API / relay のみ構成）。false へ戻すことが rollback 手順になる。
    /// capability（`features.community_index` 等）の公開宣言とは独立: stack を配備しても
    /// ユーザー向け read surface は `COMMUNITY_NODE_INDEX_QUERY_ENABLED` /
    /// `COMMUNITY_NODE_TRUST_READ_ENABLED`（既定 false）が gate する。
    #[serde(default)]
    pub deploy_indexer_stack: bool,
    #[serde(default = "default_cn_indexer_image")]
    pub cn_indexer_image: String,
    #[serde(default = "default_arcadedb_image")]
    pub arcadedb_image: String,
    /// cn-indexer data dir + ArcadeDB data 用の専用 persistent disk サイズ（GB）。
    /// 0 なら boot disk 上の docker volume（VM 置換でデータ消失。ArcadeDB は rebuildable だが
    /// indexer の iroh endpoint 同一性が失われるため本番では > 0 を推奨）。
    #[serde(default)]
    pub indexer_data_disk_gb: u32,
    /// `cn-cli relation analyze` の定期実行間隔（分。>= 1）。
    #[serde(default = "default_relation_analyze_interval_minutes")]
    pub relation_analyze_interval_minutes: u32,
    /// cn-indexer が discovery / relay-assist に使う外部 relay URL。
    /// 自前 relay（`features.iroh_relay`）が無効な場合、deploy_indexer_stack=true では必須。
    #[serde(default)]
    pub indexer_external_relay_urls: Vec<String>,
    /// distance opt-out が「遠い」と判定する node-local proximity 境界。
    /// community index / local trust のどちらかを公開する場合は明示設定が必須。
    #[serde(default)]
    pub relation_distance_optout_min_proximity: Option<f64>,
    /// `COMMUNITY_NODE_CHANNEL_SECRET_KEY` を保持する Secret Manager secret ID（値ではない）。
    #[serde(default)]
    pub channel_secret_key_secret_id: Option<String>,
    /// 通報・権利侵害案件の機微情報を暗号化する
    /// `COMMUNITY_NODE_LEGAL_DATA_KEY` の Secret Manager secret ID（値ではない）。
    #[serde(default)]
    pub legal_data_key_secret_id: Option<String>,
    /// ArcadeDB root password を保持する Secret Manager secret ID（値ではない）。
    #[serde(default)]
    pub arcadedb_password_secret_id: Option<String>,
    /// Project Arachnid Shield の username / password を保持する Secret Manager secret ID
    /// （値ではない）。`safety.providers.*` に `project-arachnid-shield` を使う場合は必須。
    #[serde(default)]
    pub arachnid_username_secret_id: Option<String>,
    #[serde(default)]
    pub arachnid_password_secret_id: Option<String>,
    /// 任意の `COMMUNITY_NODE_VLM_API_KEY` を保持する Secret Manager secret ID（値ではない）。
    /// self-host の無認証 endpoint では未指定のままでよい。
    #[serde(default)]
    pub vlm_api_key_secret_id: Option<String>,
    /// OpenAI-compatible VLM endpoint（#420）。`safety.providers.*` に
    /// `openai-compatible-vlm` を使う場合は base_url / model が必須。
    #[serde(default)]
    pub vlm_api_base_url: Option<String>,
    #[serde(default)]
    pub vlm_model: Option<String>,
    /// VLM 応答形式（`json` / `guard`）。未指定なら binary 既定（json）。
    #[serde(default)]
    pub vlm_response_format: Option<String>,
    /// VLM API timeout（秒）。0 なら binary 既定。
    #[serde(default)]
    pub vlm_api_timeout_secs: u32,
    /// media scan 用一時 fetch の上限（bytes / 秒）。0 なら binary 既定。
    #[serde(default)]
    pub media_fetch_max_bytes: u64,
    #[serde(default)]
    pub media_fetch_timeout_secs: u32,
}

/// profile / features を解決し検証済みの設定。
#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    pub raw: OperatorConfig,
    /// capability ごとの有効・無効（全 capability を網羅）。
    enabled: BTreeMap<Capability, bool>,
}

impl ResolvedConfig {
    pub fn enabled(&self, capability: Capability) -> bool {
        self.enabled.get(&capability).copied().unwrap_or(false)
    }

    /// `Capability::ALL` の順序で有効な capability を返す。
    pub fn enabled_capabilities(&self) -> Vec<Capability> {
        Capability::ALL
            .iter()
            .copied()
            .filter(|cap| self.enabled(*cap))
            .collect()
    }

    /// `Capability::ALL` の順序で無効な capability を返す。
    pub fn disabled_capabilities(&self) -> Vec<Capability> {
        Capability::ALL
            .iter()
            .copied()
            .filter(|cap| !self.enabled(*cap))
            .collect()
    }

    /// 有効かつ Phase B（計画中）の capability。
    pub fn enabled_planned_capabilities(&self) -> Vec<Capability> {
        self.enabled_capabilities()
            .into_iter()
            .filter(|cap| cap.availability().is_planned())
            .collect()
    }

    pub fn contact(&self) -> String {
        self.raw
            .server
            .contact
            .clone()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| "未設定".to_string())
    }

    pub fn legal_document(&self, kind: LegalDocumentKind) -> Option<&LegalDocumentConfig> {
        self.raw
            .legal
            .as_ref()?
            .documents
            .iter()
            .find(|document| document.kind == kind)
    }

    pub fn policy_url(&self, path: &str) -> String {
        format!("https://{}/{}", self.raw.server.domain, path)
    }

    /// 通報受付 endpoint（#370）。report_endpoint capability が有効なときのみ絶対 URL を返す。
    /// 無効なら空文字を返し、client（#310）は abuse_contact 案内に切り替える。
    pub fn report_endpoint(&self) -> String {
        if self.enabled(Capability::ReportEndpoint) {
            self.policy_url("v1/report")
        } else {
            String::new()
        }
    }

    /// 権利侵害申出画面。専用 capability が有効なときだけ公開する。
    pub fn rights_request_url(&self) -> String {
        if self.enabled(Capability::RightsRequestEndpoint) {
            self.policy_url("rights-requests/new")
        } else {
            String::new()
        }
    }

    /// terraform デプロイ設定（#380）。未指定なら None。
    pub fn deploy(&self) -> Option<&DeployConfig> {
        self.raw.deploy.as_ref()
    }

    /// cn-user-api の公開 hostname。`server.domain` をそのまま使う。
    pub fn api_domain(&self) -> &str {
        self.raw.server.domain.as_str()
    }

    /// blob cache の単一真実源（#380）。`features.blob_cache` を根拠にする。
    pub fn blob_cache_enabled(&self) -> bool {
        self.enabled(Capability::BlobCache)
    }
}

/// YAML 文字列をパースする。
pub fn parse_config(yaml: &str) -> Result<OperatorConfig> {
    let config: OperatorConfig = serde_yaml::from_str(yaml)
        .map_err(|e| anyhow!("operator-config.yaml のパースに失敗しました: {e}"))?;
    Ok(config)
}

/// profile と features を解決し、必須項目・Phase B 承認を検証する。
pub fn resolve_and_validate(config: OperatorConfig) -> Result<ResolvedConfig> {
    // 必須フィールド。
    if config.server.domain.trim().is_empty() {
        bail!("server.domain は必須です");
    }
    if config.server.operator_name.trim().is_empty() {
        bail!("server.operator_name は必須です");
    }
    if config.server.country.trim().len() != 2 {
        bail!("server.country は ISO 3166-1 alpha-2（2文字、例: JP）で指定してください");
    }
    validate_retention(&config.retention)?;
    validate_legal_config(&config)?;

    // 未知の feature キーを拒否する（typo によるサイレントな無効化を防ぐ）。
    let known: BTreeMap<&str, Capability> = Capability::ALL.iter().map(|c| (c.key(), *c)).collect();
    for key in config.features.keys() {
        if !known.contains_key(key.as_str()) {
            bail!(
                "features に未知のキー `{key}` があります。指定可能: {}",
                Capability::ALL
                    .iter()
                    .map(|c| c.key())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    // profile 既定値 -> features 上書き の順で解決する。
    let profile_defaults = config
        .profile
        .map(|p| p.feature_defaults())
        .unwrap_or_default();

    let mut enabled: BTreeMap<Capability, bool> = BTreeMap::new();
    for cap in Capability::ALL {
        // auth_consent は baseline として常に有効。
        let baseline = matches!(cap, Capability::AuthConsent);
        let from_profile = profile_defaults.get(&cap).copied().unwrap_or(baseline);
        let value = config
            .features
            .get(cap.key())
            .copied()
            .unwrap_or(from_profile);
        enabled.insert(cap, value || baseline);
    }

    let resolved = ResolvedConfig {
        raw: config,
        enabled,
    };

    // Phase B capability の承認ガード。
    let planned = resolved.enabled_planned_capabilities();
    if !planned.is_empty() && !resolved.raw.acknowledge_planned_capabilities {
        let names = planned
            .iter()
            .map(|c| c.key())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "計画中（未実装）の capability が有効化されています: {names}\n\
             これらは現行の community node 実装では提供されません。\n\
             運用中であるかのような開示文書の生成を防ぐため、\n\
             config に `acknowledge_planned_capabilities: true` を設定して\n\
             「spec として記述する」ことを明示的に承認してください。\n\
             承認した場合でも、生成文書ではこれらは「{}」として扱われます。",
            Availability::Planned.label_ja()
        );
    }

    if let Some(safety) = resolved.raw.safety.as_ref() {
        validate_safety_config(safety)?;
        // 公開ノード情報の node_id は異議申し立ての発行元照合に使われる。モデレーションを提供する
        // 公開ノードでは、リスク判定の issuer_node_id(署名鍵の公開鍵 hex)と同じ値を必ず記入する(#706)。
        if resolved.enabled(Capability::Moderation)
            && resolved
                .raw
                .server
                .node_id
                .as_deref()
                .is_none_or(|node_id| node_id.trim().is_empty())
        {
            bail!(
                "server.node_id は必須です(safety 節があり features.moderation が有効な公開ノード)。\
                 モデレーション事象の発行元識別子(署名鍵の公開鍵 hex。`cn-cli moderation issuer-node-id` で導出)\
                 と同じ値を記入してください。異議申し立ては公開ノード情報の node_id と \
                 risk signal の issuer_node_id が一致する場合だけ受理されます"
            );
        }
    }

    // deploy セクションの検証（指定されている場合のみ。未指定は従来通り通す）。
    if let Some(deploy) = resolved.raw.deploy.as_ref() {
        validate_deploy(&resolved, deploy)?;
    }

    Ok(resolved)
}

fn validate_legal_config(config: &OperatorConfig) -> Result<()> {
    let Some(legal) = config.legal.as_ref() else {
        return Ok(());
    };
    if config
        .server
        .contact
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        bail!("legal を公開する場合は server.contact の明示設定が必須です");
    }
    if legal.identity_disclosure_request.trim().is_empty() {
        bail!("legal.identity_disclosure_request は必須です");
    }
    let mut kinds = std::collections::BTreeSet::new();
    let mut slugs = std::collections::BTreeSet::new();
    for document in &legal.documents {
        if !kinds.insert(document.kind) {
            bail!(
                "legal.documents に kind {:?} が重複しています",
                document.kind
            );
        }
        let slug = document.slug.trim();
        if slug.is_empty()
            || !slug
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        {
            bail!("legal.documents.slug は snake_case の非空文字列で指定してください");
        }
        if !slugs.insert(slug.to_string()) {
            bail!("legal.documents に slug `{slug}` が重複しています");
        }
        if document.version <= 0 {
            bail!("legal document `{slug}` の version は正の整数で指定してください");
        }
        NaiveDate::parse_from_str(document.effective_date.trim(), "%Y-%m-%d").map_err(|_| {
            anyhow!("legal document `{slug}` の effective_date は YYYY-MM-DD で指定してください")
        })?;
        let authoritative_language = document.language.trim();
        if authoritative_language.is_empty()
            || !authoritative_language
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            bail!("legal document `{slug}` の language は BCP 47 形式の非空値で指定してください");
        }
        if !matches!(authoritative_language, "ja" | "en") {
            bail!(
                "legal document `{slug}` の正文 language `{authoritative_language}` に対応する renderer がありません"
            );
        }
        if document
            .supplemental_markdown
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            bail!("legal document `{slug}` の supplemental_markdown は空にできません");
        }
        let mut translation_languages = std::collections::BTreeSet::new();
        for translation in &document.translations {
            let language = translation.language.trim();
            if language.is_empty() || language == authoritative_language {
                bail!(
                    "legal document `{slug}` の参考訳 language は正文と異なる非空値にしてください"
                );
            }
            if !translation_languages.insert(language.to_string()) {
                bail!("legal document `{slug}` の参考訳 language `{language}` が重複しています");
            }
            if translation.revision <= 0 {
                bail!("legal document `{slug}` の参考訳 revision は正の整数で指定してください");
            }
            if translation.translation_of_version != document.version {
                bail!(
                    "legal document `{slug}` の参考訳は正文 version {} を参照してください",
                    document.version
                );
            }
            if translation.title.trim().is_empty() || translation.body_markdown.trim().is_empty() {
                bail!("legal document `{slug}` の参考訳 title/body_markdown は必須です");
            }
        }
    }
    crate::docs_trust_observation_sharing::validate_document(legal)?;
    for kind in LegalDocumentKind::ALL {
        if !kinds.contains(&kind) {
            bail!("legal.documents に kind {:?} が必要です", kind);
        }
    }
    for required_kind in [LegalDocumentKind::Terms, LegalDocumentKind::Privacy] {
        if config
            .legal
            .as_ref()
            .and_then(|value| value.documents.iter().find(|doc| doc.kind == required_kind))
            .is_none_or(|document| !document.required)
        {
            bail!(
                "Phase A では {:?} を required: true にしてください",
                required_kind
            );
        }
    }
    for optional_kind in [
        LegalDocumentKind::ExternalTransmission,
        LegalDocumentKind::ModerationPolicy,
        LegalDocumentKind::AbusePolicy,
        LegalDocumentKind::DataRetention,
        LegalDocumentKind::RightsInfringement,
    ] {
        if config
            .legal
            .as_ref()
            .and_then(|value| value.documents.iter().find(|doc| doc.kind == optional_kind))
            .is_some_and(|document| document.required)
        {
            bail!(
                "Phase A では {:?} を追加の必須同意にしません",
                optional_kind
            );
        }
    }
    Ok(())
}

/// deploy セクションを検証する（#380）。
fn validate_deploy(resolved: &ResolvedConfig, deploy: &DeployConfig) -> Result<()> {
    deploy.moderation.validate()?;
    let project_id = require_deploy_string("deploy.project_id", &deploy.project_id)?;
    let acme_email = require_deploy_string("deploy.acme_email", &deploy.acme_email)?;
    let jwt_secret_id = require_deploy_string("deploy.jwt_secret_id", &deploy.jwt_secret_id)?;
    let postgres_password_secret_id = require_deploy_string(
        "deploy.postgres_password_secret_id",
        &deploy.postgres_password_secret_id,
    )?;

    validate_deploy_string("deploy.region", &deploy.region)?;
    validate_deploy_string("deploy.zone", &deploy.zone)?;
    validate_admin_actor(deploy.admin_actor.as_str())?;
    validate_deploy_string("deploy.cn_user_api_image", &deploy.cn_user_api_image)?;
    validate_deploy_string("deploy.cn_iroh_relay_image", &deploy.cn_iroh_relay_image)?;
    validate_deploy_string("deploy.cn_cli_image", &deploy.cn_cli_image)?;
    validate_deploy_string("deploy.cn_indexer_image", &deploy.cn_indexer_image)?;
    validate_deploy_string("deploy.arcadedb_image", &deploy.arcadedb_image)?;
    validate_deploy_string("deploy.machine_type", &deploy.machine_type)?;
    validate_deploy_string("deploy.blob_cache_path", &deploy.blob_cache_path)?;
    for url in &deploy.indexer_external_relay_urls {
        let trimmed = require_deploy_string("deploy.indexer_external_relay_urls", url)?;
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
            bail!(
                "deploy.indexer_external_relay_urls は http(s):// で始まる URL で指定してください"
            );
        }
    }
    if let Some(format) = deploy.vlm_response_format.as_deref() {
        let trimmed = format.trim();
        if !trimmed.is_empty() && trimmed != "json" && trimmed != "guard" {
            bail!(
                "deploy.vlm_response_format は `json` または `guard` で指定してください (got `{trimmed}`)"
            );
        }
    }
    if let Some(base_url) = deploy.vlm_api_base_url.as_deref() {
        let trimmed = require_deploy_string("deploy.vlm_api_base_url", base_url)?;
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
            bail!("deploy.vlm_api_base_url は http(s):// で始まる URL で指定してください");
        }
    }
    if let Some(model) = deploy.vlm_model.as_deref() {
        validate_deploy_string("deploy.vlm_model", model)?;
    }

    let read_surface_enabled = resolved.enabled(Capability::CommunityIndex)
        || resolved.enabled(Capability::CommunityLocalTrust);
    match deploy.relation_distance_optout_min_proximity {
        Some(value) if value.is_finite() && value > 0.0 && value <= 1.0 => {}
        Some(value) => bail!(
            "deploy.relation_distance_optout_min_proximity は (0, 1] で指定してください (got `{value}`)"
        ),
        None if read_surface_enabled => bail!(
            "community_index または community_local_trust が有効な場合、deploy.relation_distance_optout_min_proximity は必須です"
        ),
        None => {}
    }

    validate_indexer_stack(resolved, deploy)?;

    if resolved.enabled(Capability::ReportEndpoint)
        || resolved.enabled(Capability::RightsRequestEndpoint)
    {
        let present = deploy
            .legal_data_key_secret_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if !present {
            bail!(
                "report_endpoint または rights_request_endpoint が有効な場合、deploy.legal_data_key_secret_id は必須です"
            );
        }
    }

    // low-cost template は cn-iroh-relay を常に配置し、relay_domain を Caddy / compose / certbot で使う。
    let relay_domain = if deploy.profile == DeployProfile::LowCost {
        let relay_domain = deploy
            .relay_domain
            .as_ref()
            .map(|d| require_deploy_string("deploy.relay_domain", d))
            .transpose()?
            .ok_or_else(|| {
                anyhow!("deploy.profile=low-cost の場合、deploy.relay_domain は必須です")
            })?;
        Some(relay_domain)
    } else if resolved.enabled(Capability::IrohRelay) {
        // managed-db / ha は tfvars 未対応だが、relay capability を開示するなら hostname は必要。
        let relay_domain = deploy
            .relay_domain
            .as_ref()
            .map(|d| require_deploy_string("deploy.relay_domain", d))
            .transpose()?
            .ok_or_else(|| {
                anyhow!("iroh_relay capability が有効な場合、deploy.relay_domain は必須です")
            })?;
        Some(relay_domain)
    } else {
        deploy
            .relay_domain
            .as_ref()
            .map(|d| require_deploy_string("deploy.relay_domain", d))
            .transpose()?
    };

    // Cloud DNS を管理するなら zone 名が必須。
    if deploy.manage_cloud_dns
        && deploy
            .dns_zone_name
            .as_ref()
            .map(|z| z.trim().is_empty())
            .unwrap_or(true)
    {
        bail!("deploy.manage_cloud_dns=true の場合、deploy.dns_zone_name は必須です");
    }

    if let Some(dns_zone_name) = deploy.dns_zone_name.as_ref() {
        validate_deploy_string("deploy.dns_zone_name", dns_zone_name)?;
    }

    // blob cache の真実源は features.blob_cache。無効なのに sizing を指定するのは矛盾。
    if !resolved.blob_cache_enabled() && deploy.blob_cache_size_gb > 0 {
        bail!(
            "features.blob_cache=false ですが deploy.blob_cache_size_gb > 0 が指定されています。\n\
             blob cache の on/off は features.blob_cache を真実源とします。\n\
             有効化する場合は features.blob_cache: true を設定してください。"
        );
    }

    // profile（low-cost / managed-db / ha）の tfvars 生成対応可否は generate-tfvars 側で判定する。
    // ここで弾くと managed-db / ha を deploy に書いた config が docs / manifest 生成すらできなくなる。

    if deploy.profile == DeployProfile::LowCost {
        validate_gcp_project_id("deploy.project_id", project_id)?;
        validate_gcp_location("deploy.region", deploy.region.trim())?;
        validate_gcp_location("deploy.zone", deploy.zone.trim())?;
        validate_dns_hostname("server.domain", resolved.api_domain().trim())?;
        if let Some(relay_domain) = relay_domain {
            validate_dns_hostname("deploy.relay_domain", relay_domain)?;
        }
        validate_acme_email(acme_email)?;
        validate_secret_id("deploy.jwt_secret_id", jwt_secret_id)?;
        validate_secret_id(
            "deploy.postgres_password_secret_id",
            postgres_password_secret_id,
        )?;
        validate_container_image("deploy.cn_user_api_image", deploy.cn_user_api_image.trim())?;
        validate_container_image(
            "deploy.cn_iroh_relay_image",
            deploy.cn_iroh_relay_image.trim(),
        )?;
        validate_container_image("deploy.cn_cli_image", deploy.cn_cli_image.trim())?;
        validate_container_image("deploy.cn_indexer_image", deploy.cn_indexer_image.trim())?;
        validate_container_image("deploy.arcadedb_image", deploy.arcadedb_image.trim())?;
        for (field, secret_id) in [
            (
                "deploy.channel_secret_key_secret_id",
                &deploy.channel_secret_key_secret_id,
            ),
            (
                "deploy.legal_data_key_secret_id",
                &deploy.legal_data_key_secret_id,
            ),
            (
                "deploy.arcadedb_password_secret_id",
                &deploy.arcadedb_password_secret_id,
            ),
            (
                "deploy.arachnid_username_secret_id",
                &deploy.arachnid_username_secret_id,
            ),
            (
                "deploy.arachnid_password_secret_id",
                &deploy.arachnid_password_secret_id,
            ),
            (
                "deploy.vlm_api_key_secret_id",
                &deploy.vlm_api_key_secret_id,
            ),
        ] {
            if let Some(secret_id) = secret_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                validate_secret_id(field, secret_id)?;
            }
        }
        validate_machine_type("deploy.machine_type", deploy.machine_type.trim())?;
        validate_absolute_path("deploy.blob_cache_path", deploy.blob_cache_path.trim())?;
        if let Some(dns_zone_name) = deploy
            .dns_zone_name
            .as_ref()
            .map(|z| z.trim())
            .filter(|z| !z.is_empty())
        {
            validate_gcp_name("deploy.dns_zone_name", dns_zone_name)?;
        }
    }

    Ok(())
}

/// index / moderation stack（#615）配備時の整合検証。
///
/// runtime（cn-indexer）の起動 gate と同じ判定を config 段階で fail-closed に写す:
/// 必須 secret ID の欠落、relay 不在、provider に対する credential / endpoint 欠落は
/// apply 前に検出する。
fn validate_indexer_stack(resolved: &ResolvedConfig, deploy: &DeployConfig) -> Result<()> {
    if !deploy.deploy_indexer_stack {
        return Ok(());
    }

    crate::deploy::validate_relation_analyze_interval(deploy.relation_analyze_interval_minutes)?;

    let require_secret = |field: &str, value: &Option<String>| -> Result<()> {
        if value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
        {
            bail!("deploy.deploy_indexer_stack=true の場合、{field} は必須です");
        }
        Ok(())
    };
    require_secret(
        "deploy.channel_secret_key_secret_id",
        &deploy.channel_secret_key_secret_id,
    )?;
    require_secret(
        "deploy.arcadedb_password_secret_id",
        &deploy.arcadedb_password_secret_id,
    )?;

    // relay validation gate（ADR 0025 §6.4）を config 段階で写す。
    if !resolved.enabled(Capability::IrohRelay) && deploy.indexer_external_relay_urls.is_empty() {
        bail!(
            "deploy.deploy_indexer_stack=true には validated relay が必要です。\n\
             features.iroh_relay を有効化するか、deploy.indexer_external_relay_urls を指定してください。"
        );
    }

    let safety = resolved.raw.safety.clone().unwrap_or_default();

    // signed moderation event（既定 true）には signing key secret が必要。
    if safety.events.emit_signed_moderation_events
        && safety
            .events
            .signing_key_secret_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
    {
        bail!(
            "deploy.deploy_indexer_stack=true かつ safety.events.emit_signed_moderation_events=true \
             の場合、safety.events.signing_key_secret_id は必須です"
        );
    }

    // provider ごとの credential / endpoint 要件。
    let providers = [
        safety.providers.known_csam.as_ref(),
        safety.providers.general.as_ref(),
        safety.providers.unknown_csam.as_ref(),
    ];
    let uses = |name: &str| {
        providers
            .iter()
            .flatten()
            .any(|entry| entry.provider.trim().replace('_', "-") == name)
    };
    if uses("project-arachnid-shield") {
        require_secret(
            "deploy.arachnid_username_secret_id",
            &deploy.arachnid_username_secret_id,
        )?;
        require_secret(
            "deploy.arachnid_password_secret_id",
            &deploy.arachnid_password_secret_id,
        )?;
    }
    if uses("openai-compatible-vlm") {
        for (field, value) in [
            ("deploy.vlm_api_base_url", &deploy.vlm_api_base_url),
            ("deploy.vlm_model", &deploy.vlm_model),
        ] {
            if value
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none()
            {
                bail!("safety.providers に openai-compatible-vlm を使う場合、{field} は必須です");
            }
        }
    }

    if uses("openai-moderation") {
        require_secret(
            "deploy.vlm_api_key_secret_id",
            &deploy.vlm_api_key_secret_id,
        )?;
        if safety
            .providers
            .known_csam
            .as_ref()
            .is_some_and(|entry| entry.provider.replace('_', "-") == "openai-moderation")
            || safety
                .providers
                .unknown_csam
                .as_ref()
                .is_some_and(|entry| entry.provider.replace('_', "-") == "openai-moderation")
        {
            bail!("openai-moderation は general slot 専用です");
        }
        if safety
            .providers
            .general
            .as_ref()
            .is_some_and(|entry| matches!(entry.hosting, Some(crate::ProviderHosting::SelfHost)))
        {
            bail!("OpenAI Moderation は第三者への外部送信として開示してください");
        }
    }

    Ok(())
}

fn require_deploy_string<'a>(field: &str, value: &'a str) -> Result<&'a str> {
    validate_deploy_string(field, value)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{field} は必須です");
    }
    Ok(trimmed)
}

fn validate_deploy_string(field: &str, value: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        bail!("{field} に制御文字は指定できません");
    }
    Ok(())
}

fn validate_admin_actor(value: &str) -> Result<()> {
    if value != value.trim() || value.len() > 254 || value.chars().any(char::is_control) {
        bail!(
            "deploy.admin_actor は前後空白と制御文字を含まない 254 文字以下の値、または空で指定してください"
        );
    }
    Ok(())
}

fn validate_gcp_project_id(field: &str, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let valid_len = (6..=30).contains(&bytes.len());
    let valid_start = bytes.first().is_some_and(u8::is_ascii_lowercase);
    let valid_end = bytes
        .last()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    let valid_chars = bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');

    if !(valid_len && valid_start && valid_end && valid_chars) {
        bail!(
            "{field} は GCP project ID 形式（小文字英数字と hyphen、6-30 文字）で指定してください"
        );
    }
    Ok(())
}

fn validate_gcp_location(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("{field} は小文字英数字と hyphen のみで指定してください");
    }
    Ok(())
}

fn validate_dns_hostname(field: &str, value: &str) -> Result<()> {
    if value.len() > 253 || value.trim_end_matches('.').is_empty() {
        bail!("{field} は DNS hostname 形式で指定してください");
    }

    for label in value.trim_end_matches('.').split('.') {
        let bytes = label.as_bytes();
        let valid_len = !bytes.is_empty() && bytes.len() <= 63;
        let valid_edges = bytes
            .first()
            .zip(bytes.last())
            .is_some_and(|(first, last)| {
                first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric()
            });
        let valid_chars = bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-');
        if !(valid_len && valid_edges && valid_chars) {
            bail!("{field} は DNS hostname 形式で指定してください");
        }
    }

    Ok(())
}

fn validate_acme_email(value: &str) -> Result<()> {
    if value.contains(char::is_whitespace) {
        bail!("deploy.acme_email に空白は指定できません");
    }
    let Some((local, domain)) = value.split_once('@') else {
        bail!("deploy.acme_email は email 形式で指定してください");
    };
    if local.is_empty() || domain.contains('@') {
        bail!("deploy.acme_email は email 形式で指定してください");
    }
    validate_dns_hostname("deploy.acme_email の domain", domain)
}

fn validate_secret_id(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!(
            "{field} は Secret Manager secret ID 形式（英数字、hyphen、underscore）で指定してください"
        );
    }
    Ok(())
}

fn validate_container_image(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.contains(char::is_whitespace) {
        bail!("{field} は空白を含まない container image 参照で指定してください");
    }
    Ok(())
}

fn validate_machine_type(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("{field} は小文字英数字と hyphen のみで指定してください");
    }
    Ok(())
}

fn validate_absolute_path(field: &str, value: &str) -> Result<()> {
    if !value.starts_with('/') || value.contains(char::is_whitespace) {
        bail!("{field} は空白を含まない absolute path で指定してください");
    }
    Ok(())
}

fn validate_gcp_name(field: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("{field} は小文字英数字と hyphen のみで指定してください");
    }
    Ok(())
}

/// パースと解決・検証をまとめて行う。
pub fn load_and_validate(yaml: &str) -> Result<ResolvedConfig> {
    validate_explicit_legal_retention(yaml)?;
    resolve_and_validate(parse_config(yaml)?)
}

/// 法務カタログを公開する構成では、文書に表示される保持期間をコード既定値へ
/// 暗黙にフォールバックさせない。legacy（`legal` なし）の config だけは従来の
/// default を維持する。
fn validate_explicit_legal_retention(yaml: &str) -> Result<()> {
    let root: serde_yaml::Value = serde_yaml::from_str(yaml)
        .map_err(|e| anyhow!("operator-config.yaml のパースに失敗しました: {e}"))?;
    let Some(root) = root.as_mapping() else {
        bail!("operator-config.yaml の root は mapping で指定してください");
    };
    let key = |value: &str| serde_yaml::Value::String(value.to_string());
    if !root.contains_key(key("legal")) {
        return Ok(());
    }
    let retention = root
        .get(key("retention"))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or_else(|| {
            anyhow!("legal を設定する場合は retention の全保持期間を明示してください")
        })?;
    for field in [
        "connection_logs_days",
        "moderation_logs_days",
        "report_days",
        "report_contact_days",
        "tester_feedback_days",
        "rights_request_active_days",
        "rights_request_resolved_days",
        "rights_request_rejected_days",
        "rights_request_contact_days",
        "rights_request_identity_days",
        "rights_request_evidence_days",
        "rights_request_history_days",
        "operator_audit_days",
        "moderation_event_days",
        "risk_signal_days",
    ] {
        if !retention.contains_key(key(field)) {
            bail!("legal を設定する場合は retention.{field} を明示してください");
        }
    }
    Ok(())
}

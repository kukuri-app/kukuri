//! community node manifest の型付き共有スキーマ (#355)。
//!
//! `server-manifest.json` を単なる JSON 値ではなく型付き struct として定義する。これにより:
//! - operator config (#352) から決定論的に生成できる
//! - public manifest endpoint (#356) が同じ型を共有できる
//! - client が dependency 表示 / report routing / consent UI で型安全に扱える
//!
//! authority scope / P2P boundary は `docs/architecture/p2p-first-community-node-responsibility-boundary.md`
//! の責任境界を machine-readable に表現する。community node を home server / central operator と
//! 誤解させないため、p2p_boundary は identity / profile / social graph / network-wide authority を
//! すべて false として宣言する（これは kukuri の P2P-first 設計の不変条件であり、operator は変更できない）。
//!
//! Phase A / Phase B の分離も保持する。`capability_scope` は `available_enabled`（運用中）と
//! `planned_enabled`（計画中・未提供）を分けて宣言する。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::{Availability, Capability};
use crate::config::ResolvedConfig;

/// community node の役割。
///
/// `default-onboarding-node` と `community-node`（third-party）を区別できることが重要。
/// default-onboarding-node は onboarding infrastructure であり network-wide authority ではない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeRole {
    DefaultOnboardingNode,
    /// 複数 capability を持つ一般的なノードの既定 role。
    #[default]
    CommunityNode,
    RelayAssist,
    IndexNode,
    ModerationNode,
    TrustSignalNode,
}

impl NodeRole {
    /// 明示指定がない場合に有効 capability から role を推定する。
    ///
    /// 複数 capability を持つノードは一般的なため、既定は `community-node`。
    /// 単一目的に強く寄っている場合のみ専用 role を推定する。
    fn infer(config: &ResolvedConfig) -> NodeRole {
        let index = config.enabled(Capability::CommunityIndex);
        let moderation = config.enabled(Capability::Moderation);
        let trust = config.enabled(Capability::CommunityLocalTrust);
        let relay = config.enabled(Capability::IrohRelay)
            || config.enabled(Capability::TrafficRelayFallback);

        // 単一目的への強い寄りを優先して推定する。
        match (index, moderation, trust, relay) {
            (true, false, false, false) => NodeRole::IndexNode,
            (false, true, false, false) => NodeRole::ModerationNode,
            (false, false, true, false) => NodeRole::TrustSignalNode,
            (false, false, false, true) => NodeRole::RelayAssist,
            _ => NodeRole::CommunityNode,
        }
    }

    fn resolve(config: &ResolvedConfig) -> NodeRole {
        config
            .raw
            .manifest
            .node_role
            .unwrap_or_else(|| NodeRole::infer(config))
    }
}

/// authority scope を operator が拡張・上書きするための設定。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityScopeOverride {
    /// 有効 capability から導出した applies_to に追加する項目。
    #[serde(default)]
    pub additional_applies_to: Vec<String>,
    /// does_not_apply_to を上書きする。未指定なら安全な default を使う。
    #[serde(default)]
    pub does_not_apply_to: Option<Vec<String>>,
}

/// 安全な default の does_not_apply_to。
fn default_does_not_apply_to() -> Vec<String> {
    [
        "kukuri_network_as_a_whole",
        "third_party_nodes",
        "user_identity",
        "user_profile_canonical_source",
        "user_social_graph_canonical_source",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 各 capability の有効・無効。client が capability scope を型安全に扱えるようにする。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Capabilities {
    pub auth_consent: bool,
    pub bootstrap_assist: bool,
    pub topic_rendezvous: bool,
    pub iroh_relay: bool,
    pub traffic_relay_fallback: bool,
    pub blob_cache: bool,
    pub private_message_storage: bool,
    pub analytics: bool,
    pub crash_report: bool,
    pub cloudflare_proxy: bool,
    pub push_notification: bool,
    pub community_index: bool,
    pub moderation: bool,
    pub community_local_trust: bool,
    pub report_endpoint: bool,
    #[serde(default)]
    pub rights_request_endpoint: bool,
    #[serde(default)]
    pub tester_feedback: bool,
    #[serde(default)]
    pub dome_hosting: bool,
    #[serde(default)]
    pub public_blob_search: bool,
}

impl Capabilities {
    fn from_config(config: &ResolvedConfig) -> Self {
        Self {
            auth_consent: config.enabled(Capability::AuthConsent),
            bootstrap_assist: config.enabled(Capability::BootstrapAssist),
            topic_rendezvous: config.enabled(Capability::TopicRendezvous),
            iroh_relay: config.enabled(Capability::IrohRelay),
            traffic_relay_fallback: config.enabled(Capability::TrafficRelayFallback),
            blob_cache: config.enabled(Capability::BlobCache),
            private_message_storage: config.enabled(Capability::PrivateMessageStorage),
            analytics: config.enabled(Capability::Analytics),
            crash_report: config.enabled(Capability::CrashReport),
            cloudflare_proxy: config.enabled(Capability::CloudflareProxy),
            push_notification: config.enabled(Capability::PushNotification),
            community_index: config.enabled(Capability::CommunityIndex),
            moderation: config.enabled(Capability::Moderation),
            community_local_trust: config.enabled(Capability::CommunityLocalTrust),
            report_endpoint: config.enabled(Capability::ReportEndpoint),
            rights_request_endpoint: config.enabled(Capability::RightsRequestEndpoint),
            tester_feedback: config.enabled(Capability::TesterFeedback),
            dome_hosting: config.enabled(Capability::DomeHosting),
            public_blob_search: config.enabled(Capability::PublicBlobSearch),
        }
    }
}

/// Phase A / Phase B を分離した capability scope。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CapabilityScope {
    /// 有効かつ提供中（Phase A）の capability キー。
    pub available_enabled: Vec<String>,
    /// 有効だが計画中・未提供（Phase B）の capability キー。
    pub planned_enabled: Vec<String>,
}

/// authority scope。node が責任を主張する範囲と、しない範囲。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AuthorityScope {
    pub applies_to: Vec<String>,
    pub does_not_apply_to: Vec<String>,
}

/// P2P boundary metadata。
///
/// kukuri の P2P-first 不変条件として、community node は user identity / profile /
/// social graph / content truth source / network-wide authority のいずれの権威も持たない
/// （すべて false）。`Default` がこの不変条件を表現する。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct P2pBoundary {
    pub identity_authority: bool,
    pub profile_canonical_store: bool,
    pub social_graph_canonical_store: bool,
    pub content_truth_source: bool,
    pub network_wide_authority: bool,
}

/// 互換のための features サマリ（#352 例と整合）。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ManifestFeatures {
    pub community_index: bool,
    pub moderation: bool,
    /// "community-local" または "none"。
    pub trust_score: String,
    pub iroh_relay: bool,
    /// "dedicated" または "none"。
    pub iroh_relay_mode: String,
    pub traffic_relay_fallback: bool,
    pub private_message_storage: bool,
    pub blob_cache: bool,
}

/// retention サマリ。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ManifestRetention {
    pub connection_logs_days: u32,
    pub moderation_logs_days: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_contact_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights_request_active_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights_request_resolved_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights_request_rejected_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_audit_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderation_event_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_signal_days: Option<u32>,
}

/// community node manifest（`server-manifest.json` の型付き表現）。
///
/// フィールド宣言順がそのまま JSON の出力順となり、決定論的。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CommunityNodeManifest {
    pub node_id: String,
    pub node_name: String,
    pub node_role: NodeRole,
    pub server_name: String,
    pub operator_name: String,
    pub operator_country: String,
    pub cloud_provider: String,
    pub region: String,
    pub contact: String,
    pub abuse_contact: String,
    /// 通報受付 endpoint（#370）。report_endpoint capability が有効なときの絶対 URL、
    /// 無効なら空文字。client（#310）は空なら abuse_contact 案内に切り替える。
    pub report_endpoint: String,
    /// 権利侵害申出の専用画面。capability 無効時は空文字。
    #[serde(default)]
    pub rights_request_url: String,
    #[serde(default)]
    pub rights_request_policy_url: String,
    #[serde(default)]
    pub rights_request_initial_response_target_days: u32,
    pub terms_url: String,
    pub privacy_url: String,
    pub external_transmission_url: String,
    pub moderation_policy_url: String,
    pub abuse_policy_url: String,
    pub data_retention_url: String,
    pub manifest_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legal_documents: Vec<LegalDocumentManifestEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_snapshot_revision: Option<String>,
    pub capabilities: Capabilities,
    pub capability_scope: CapabilityScope,
    pub authority_scope: AuthorityScope,
    pub p2p_boundary: P2pBoundary,
    pub features: ManifestFeatures,
    pub retention: ManifestRetention,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LegalDocumentManifestEntry {
    pub slug: String,
    pub version: i32,
    pub effective_date: String,
    pub language: String,
    pub required: bool,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authoritative_language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_translation_languages: Vec<String>,
}

fn capability_keys(caps: &[Capability]) -> Vec<String> {
    caps.iter().map(|c| c.key().to_string()).collect()
}

/// authority scope を有効 capability + operator override から構築する。
fn build_authority_scope(config: &ResolvedConfig) -> AuthorityScope {
    let mut applies_to = vec!["this_node".to_string()];
    if config.enabled(Capability::CommunityIndex) {
        applies_to.push("communities_indexed_by_this_node".to_string());
    }
    if config.enabled(Capability::Moderation) {
        applies_to.push("moderation_events_issued_by_this_node".to_string());
    }
    if config.enabled(Capability::CommunityLocalTrust) {
        applies_to.push("trust_signals_issued_by_this_node".to_string());
    }
    if config.enabled(Capability::BlobCache) {
        applies_to.push("media_cached_by_this_node".to_string());
    }
    if config.enabled(Capability::DomeHosting) {
        applies_to.push("dome_sessions_leased_to_this_node".to_string());
    }

    // operator が明示した追加項目を重複なく足す。
    for extra in &config.raw.manifest.authority_scope.additional_applies_to {
        if !applies_to.contains(extra) {
            applies_to.push(extra.clone());
        }
    }

    let does_not_apply_to = config
        .raw
        .manifest
        .authority_scope
        .does_not_apply_to
        .clone()
        .unwrap_or_else(default_does_not_apply_to);

    AuthorityScope {
        applies_to,
        does_not_apply_to,
    }
}

/// operator config から typed manifest を構築する。
pub fn build_manifest(config: &ResolvedConfig) -> CommunityNodeManifest {
    let server = &config.raw.server;

    let available_enabled: Vec<Capability> = config
        .enabled_capabilities()
        .into_iter()
        .filter(|c| c.availability() == Availability::Available)
        .collect();
    let planned_enabled = config.enabled_planned_capabilities();

    let iroh_relay_mode = if config.enabled(Capability::IrohRelay) {
        "dedicated"
    } else {
        "none"
    }
    .to_string();

    let trust_score = if config.enabled(Capability::CommunityLocalTrust) {
        "community-local"
    } else {
        "none"
    }
    .to_string();

    CommunityNodeManifest {
        node_id: server.node_id.clone().unwrap_or_default(),
        node_name: server
            .node_name
            .clone()
            .unwrap_or_else(|| server.domain.clone()),
        node_role: NodeRole::resolve(config),
        server_name: server.domain.clone(),
        operator_name: server.operator_name.clone(),
        operator_country: server.country.clone(),
        cloud_provider: server.cloud_provider.clone().unwrap_or_default(),
        region: server.region.clone().unwrap_or_default(),
        contact: config.contact(),
        abuse_contact: config.contact(),
        report_endpoint: config.report_endpoint(),
        rights_request_url: config.rights_request_url(),
        rights_request_policy_url: if config.enabled(Capability::RightsRequestEndpoint) {
            config.policy_url("rights-infringement-policy")
        } else {
            String::new()
        },
        rights_request_initial_response_target_days: config
            .raw
            .manifest
            .rights_request_initial_response_target_days,
        terms_url: config.policy_url("terms"),
        privacy_url: config.policy_url("privacy"),
        external_transmission_url: config.policy_url("external-transmission"),
        moderation_policy_url: config.policy_url("moderation-policy"),
        abuse_policy_url: config.policy_url("abuse-policy"),
        data_retention_url: config.policy_url("data-retention"),
        manifest_version: config.raw.manifest.manifest_version.clone(),
        legal_documents: config
            .raw
            .legal
            .as_ref()
            .map(|legal| {
                legal
                    .documents
                    .iter()
                    .map(|document| LegalDocumentManifestEntry {
                        slug: document.slug.clone(),
                        version: document.version,
                        effective_date: document.effective_date.clone(),
                        language: document.language.clone(),
                        required: document.required,
                        url: config.policy_url(document.kind.public_path()),
                        authoritative_language: Some(document.language.clone()),
                        reference_translation_languages: document
                            .translations
                            .iter()
                            .map(|translation| translation.language.clone())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        policy_snapshot_revision: crate::policy_snapshot_revision(config),
        capabilities: Capabilities::from_config(config),
        capability_scope: CapabilityScope {
            available_enabled: capability_keys(&available_enabled),
            planned_enabled: capability_keys(&planned_enabled),
        },
        authority_scope: build_authority_scope(config),
        p2p_boundary: P2pBoundary::default(),
        features: ManifestFeatures {
            community_index: config.enabled(Capability::CommunityIndex),
            moderation: config.enabled(Capability::Moderation),
            trust_score,
            iroh_relay: config.enabled(Capability::IrohRelay),
            iroh_relay_mode,
            traffic_relay_fallback: config.enabled(Capability::TrafficRelayFallback),
            private_message_storage: config.enabled(Capability::PrivateMessageStorage),
            blob_cache: config.enabled(Capability::BlobCache),
        },
        retention: ManifestRetention {
            connection_logs_days: config.raw.retention.connection_logs_days,
            moderation_logs_days: config.raw.retention.moderation_logs_days,
            report_days: Some(config.raw.retention.report_days),
            report_contact_days: Some(config.raw.retention.report_contact_days),
            rights_request_active_days: Some(config.raw.retention.rights_request_active_days),
            rights_request_resolved_days: Some(config.raw.retention.rights_request_resolved_days),
            rights_request_rejected_days: Some(config.raw.retention.rights_request_rejected_days),
            operator_audit_days: Some(config.raw.retention.operator_audit_days),
            moderation_event_days: Some(config.raw.retention.moderation_event_days),
            risk_signal_days: Some(config.raw.retention.risk_signal_days),
        },
    }
}

/// manifest を `serde_json::Value` として得る。
pub fn manifest_value(config: &ResolvedConfig) -> Value {
    serde_json::to_value(build_manifest(config)).expect("manifest to value")
}

/// manifest を改行終端の pretty JSON 文字列にする。
pub fn render_manifest(config: &ResolvedConfig) -> String {
    let manifest = build_manifest(config);
    let mut out = serde_json::to_string_pretty(&manifest).expect("manifest serialization");
    out.push('\n');
    out
}

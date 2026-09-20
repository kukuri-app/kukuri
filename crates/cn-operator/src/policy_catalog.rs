//! 公開法務文書カタログ。
//!
//! 文書本文、manifest、DB 同期へ渡す法務文書列の唯一の生成入口を提供する。

use serde::Serialize;

use crate::config::{LegalDocumentKind, ResolvedConfig};
use crate::docs::{GeneratedFile, generate_all};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedLegalDocument {
    pub kind: LegalDocumentKind,
    pub slug: String,
    pub version: i32,
    pub effective_date: String,
    pub language: String,
    pub required: bool,
    pub title: String,
    pub filename: String,
    pub public_path: String,
    pub content: String,
    pub policy_snapshot_revision: String,
    pub authoritative_language: String,
    pub reference_translation: bool,
    pub translation_revision: Option<i32>,
    pub translation_of_version: Option<i32>,
}

impl LegalDocumentKind {
    /// `serde(rename_all = "snake_case")` と同じ wire 表現(#1192)。公開 policy カタログの
    /// `policy_kind` として client へ渡す。slug は operator が決めるため、文書の役割は
    /// この値で判別する。
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Terms => "terms",
            Self::Privacy => "privacy",
            Self::ExternalTransmission => "external_transmission",
            Self::ModerationPolicy => "moderation_policy",
            Self::AbusePolicy => "abuse_policy",
            Self::DataRetention => "data_retention",
            Self::RightsInfringement => "rights_infringement",
            Self::TrustObservationSharing => "trust_observation_sharing",
        }
    }
}

#[derive(Serialize)]
struct CanonicalOperator<'a> {
    domain: &'a str,
    operator_name: &'a str,
    country: &'a str,
    contact: String,
    cloud_provider: &'a Option<String>,
    region: &'a Option<String>,
    identity_disclosure_request: &'a str,
}

#[derive(Serialize)]
struct CanonicalCapability {
    key: &'static str,
    descriptor: crate::CapabilityPolicyDescriptor,
}

#[derive(Serialize)]
struct CanonicalDocument<'a> {
    kind: crate::LegalDocumentKind,
    slug: &'a str,
    version: i32,
    effective_date: &'a str,
    language: &'a str,
    required: bool,
    supplemental_markdown: &'a Option<String>,
}

#[derive(Serialize)]
struct CanonicalSafetyProvider<'a> {
    provider: &'a str,
    required: bool,
    on_high_confidence: &'a Option<crate::SafetyErrorAction>,
    hosting: &'a Option<crate::ProviderHosting>,
}

#[derive(Serialize)]
struct CanonicalSafetyProviders<'a> {
    known_csam: Option<CanonicalSafetyProvider<'a>>,
    general: Option<CanonicalSafetyProvider<'a>>,
    unknown_csam: Option<CanonicalSafetyProvider<'a>>,
}

impl<'a> CanonicalSafetyProvider<'a> {
    fn from_config(provider: &'a crate::SafetyProviderEntry) -> Self {
        Self {
            provider: &provider.provider,
            required: provider.required,
            on_high_confidence: &provider.on_high_confidence,
            hosting: &provider.hosting,
        }
    }
}

#[derive(Serialize)]
struct CanonicalSafety<'a> {
    profile: &'a Option<String>,
    policy_version: &'a str,
    indexing: &'a crate::SafetyIndexingConfig,
    storage: &'a crate::SafetyStorageConfig,
    emit_signed_moderation_events: bool,
    providers: CanonicalSafetyProviders<'a>,
    moderation: &'a crate::safety_config::SafetyModerationConfig,
}

impl<'a> CanonicalSafety<'a> {
    fn from_config(safety: &'a crate::SafetyConfig) -> Self {
        Self {
            profile: &safety.profile,
            policy_version: &safety.policy_version,
            indexing: &safety.indexing,
            storage: &safety.storage,
            emit_signed_moderation_events: safety.events.emit_signed_moderation_events,
            providers: CanonicalSafetyProviders {
                known_csam: safety
                    .providers
                    .known_csam
                    .as_ref()
                    .map(CanonicalSafetyProvider::from_config),
                general: safety
                    .providers
                    .general
                    .as_ref()
                    .map(CanonicalSafetyProvider::from_config),
                unknown_csam: safety
                    .providers
                    .unknown_csam
                    .as_ref()
                    .map(CanonicalSafetyProvider::from_config),
            },
            moderation: &safety.moderation,
        }
    }
}

#[derive(Serialize)]
struct CanonicalPolicySnapshot<'a> {
    schema_version: u32,
    operator: CanonicalOperator<'a>,
    enabled_capabilities: Vec<CanonicalCapability>,
    retention: &'a crate::RetentionConfig,
    safety: Option<CanonicalSafety<'a>>,
    authority_scope: &'a crate::AuthorityScopeOverride,
    manifest_version: &'a str,
    rights_request_initial_response_target_days: u32,
    documents: Vec<CanonicalDocument<'a>>,
}

/// 表示上の Markdown ではなく、法務上意味のある構造化入力だけを hash 化する。
pub fn policy_snapshot_revision(config: &ResolvedConfig) -> Option<String> {
    let legal = config.raw.legal.as_ref()?;
    let mut documents = legal
        .documents
        .iter()
        .map(|document| CanonicalDocument {
            kind: document.kind,
            slug: &document.slug,
            version: document.version,
            effective_date: &document.effective_date,
            language: &document.language,
            required: document.required,
            supplemental_markdown: &document.supplemental_markdown,
        })
        .collect::<Vec<_>>();
    documents.sort_by(|left, right| {
        (left.kind, left.slug, left.language).cmp(&(right.kind, right.slug, right.language))
    });
    let input = CanonicalPolicySnapshot {
        schema_version: 2,
        operator: CanonicalOperator {
            domain: &config.raw.server.domain,
            operator_name: &config.raw.server.operator_name,
            country: &config.raw.server.country,
            contact: config.contact(),
            cloud_provider: &config.raw.server.cloud_provider,
            region: &config.raw.server.region,
            identity_disclosure_request: &legal.identity_disclosure_request,
        },
        enabled_capabilities: config
            .enabled_capabilities()
            .into_iter()
            .map(|capability| CanonicalCapability {
                key: capability.key(),
                descriptor: capability.policy_descriptor(),
            })
            .collect::<Vec<_>>(),
        retention: &config.raw.retention,
        safety: config.raw.safety.as_ref().map(CanonicalSafety::from_config),
        authority_scope: &config.raw.manifest.authority_scope,
        manifest_version: &config.raw.manifest.manifest_version,
        rights_request_initial_response_target_days: config
            .raw
            .manifest
            .rights_request_initial_response_target_days,
        documents,
    };
    let bytes = serde_json::to_vec(&input).expect("policy snapshot input serializes");
    Some(blake3::hash(&bytes).to_hex().to_string())
}

/// 公開法務文書を config の版情報と生成本文を結合して返す。
pub fn generate_legal_documents(config: &ResolvedConfig) -> Vec<GeneratedLegalDocument> {
    let files = generate_all(config);
    generate_legal_documents_from_files(config, &files)
}

/// 既に一度生成した文書列から policy catalog を組み立てる。server 起動時に
/// manifest/public disclosure/DB sync のため同じ本文を再生成しないための入口。
pub fn generate_legal_documents_from_files(
    config: &ResolvedConfig,
    files: &[GeneratedFile],
) -> Vec<GeneratedLegalDocument> {
    let Some(legal) = config.raw.legal.as_ref() else {
        return Vec::new();
    };
    let snapshot_revision =
        policy_snapshot_revision(config).expect("legal config has snapshot revision");
    let mut generated = Vec::new();
    for document in &legal.documents {
        let file = files
            .iter()
            .find(|file| file.filename == document.kind.filename());
        let Some(file) = file else {
            continue;
        };
        let mut content = if document.language == "en" {
            render_english_authoritative_document(config, document)
        } else {
            file.content.clone()
        };
        if let Some(supplemental) = document.supplemental_markdown.as_deref() {
            content.push_str("\n\n## 運営者による補足\n\n");
            content.push_str(supplemental.trim());
            content.push('\n');
        }
        generated.push(GeneratedLegalDocument {
            kind: document.kind,
            slug: document.slug.clone(),
            version: document.version,
            effective_date: document.effective_date.clone(),
            language: document.language.clone(),
            required: document.required,
            title: if document.language == "en" {
                document.kind.title_en().to_string()
            } else {
                document.kind.title_ja().to_string()
            },
            filename: file.filename.clone(),
            public_path: document.kind.public_path().to_string(),
            content,
            policy_snapshot_revision: snapshot_revision.clone(),
            authoritative_language: document.language.clone(),
            reference_translation: false,
            translation_revision: None,
            translation_of_version: None,
        });
        for translation in &document.translations {
            generated.push(GeneratedLegalDocument {
                kind: document.kind,
                slug: document.slug.clone(),
                version: document.version,
                effective_date: document.effective_date.clone(),
                language: translation.language.clone(),
                required: document.required,
                title: translation.title.clone(),
                filename: format!("{}.{}.md", document.slug, translation.language),
                public_path: document.kind.public_path().to_string(),
                content: translation.body_markdown.clone(),
                policy_snapshot_revision: snapshot_revision.clone(),
                authoritative_language: document.language.clone(),
                reference_translation: true,
                translation_revision: Some(translation.revision),
                translation_of_version: Some(translation.translation_of_version),
            });
        }
    }
    generated
}

fn render_english_authoritative_document(
    config: &ResolvedConfig,
    document: &crate::LegalDocumentConfig,
) -> String {
    use std::fmt::Write as _;

    let server = &config.raw.server;
    let legal = config
        .raw
        .legal
        .as_ref()
        .expect("authoritative legal document requires legal config");
    let mut output = String::new();
    let _ = writeln!(output, "# {}\n", document.kind.title_en());
    let _ = writeln!(output, "- Operator: {}", server.operator_name);
    let _ = writeln!(output, "- Server: {}", server.domain);
    let _ = writeln!(output, "- Country: {}", server.country);
    let _ = writeln!(output, "- Contact: {}", config.contact());
    let _ = writeln!(output, "- Policy slug: {}", document.slug);
    let _ = writeln!(output, "- Display version: {}", document.version);
    let _ = writeln!(output, "- Effective date: {}", document.effective_date);
    let _ = writeln!(output, "- Authoritative language: {}\n", document.language);
    let _ = writeln!(
        output,
        "> This document is generated from the operator configuration and typed capability descriptors. It is not legal advice; the operator must verify it against actual operations.\n"
    );
    let _ = writeln!(output, "## Scope\n");
    let _ = writeln!(
        output,
        "This policy applies only to the Community Node operated at `{}`. It does not govern other nodes, Direct P2P traffic that this Node does not receive, or the kukuri network as a whole.\n",
        server.domain
    );
    let _ = writeln!(output, "## Capability-derived policy facts\n");
    for capability in config.enabled_capabilities() {
        let descriptor = capability.policy_descriptor();
        let encoded = serde_json::to_string(&descriptor)
            .expect("capability policy descriptor serializes deterministically");
        let _ = writeln!(output, "- `{}`: `{}`", capability.key(), encoded);
    }
    let _ = writeln!(output, "\n## Operator-defined retention\n");
    let retention = serde_json::to_string_pretty(&config.raw.retention)
        .expect("retention config serializes deterministically");
    let _ = writeln!(output, "```json\n{retention}\n```\n");
    let _ = writeln!(output, "## Requests and remedies\n");
    let _ = writeln!(
        output,
        "Requests for access, correction, deletion, suspension, or operator identity disclosure must use the routes listed in the capability descriptors or contact `{}`. Identity disclosure procedure: {}\n",
        config.contact(),
        legal.identity_disclosure_request
    );
    let _ = writeln!(
        output,
        "Removing data from this Node does not guarantee deletion from peers, other nodes, or copies already distributed through the P2P network."
    );
    output
}

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const LEGAL_BUNDLE_VERSION: i32 = 9;
pub const APP_LEGAL_EFFECTIVE_DATE: &str = "2026-10-07";
pub const APP_LEGAL_AUTHORITATIVE_LANGUAGE: &str = "ja";
pub const AGE_ATTESTATION_VERSION: i32 = 1;
pub const APP_LEGAL_DOCUMENTS: &[(&str, i32)] = &[
    ("terms", LEGAL_BUNDLE_VERSION),
    ("privacy", LEGAL_BUNDLE_VERSION),
];

const APP_CONSENT_FILE_EXTENSION: &str = "app-consent.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConsentDocumentRecord {
    pub slug: String,
    pub version: i32,
    pub accepted_at: i64,
    pub language: String,
    pub app_version: String,
    /// 記録した build の種別(`AppBuildProfile::as_str`)。#1105 より前の記録には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_profile: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgeAttestationRecord {
    pub version: i32,
    pub attested_at: i64,
    pub language: String,
    pub app_version: String,
    /// 記録した build の種別(`AppBuildProfile::as_str`)。#1105 より前の記録には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_profile: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConsentStore {
    #[serde(default)]
    pub records: Vec<AppConsentDocumentRecord>,
    #[serde(default)]
    pub age_attestations: Vec<AgeAttestationRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConsentDocumentStatus {
    pub slug: String,
    pub current_version: i32,
    pub effective_date: String,
    pub authoritative_language: String,
    pub material_change: bool,
    pub controller_name: String,
    pub contact: String,
    pub accepted_version: Option<i32>,
    pub accepted_at: Option<i64>,
    pub accepted_language: Option<String>,
    pub accepted_app_version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct DistributionLegalMetadata {
    controller_name: String,
    contact: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgeAttestationStatus {
    pub current_version: i32,
    pub attested_version: Option<i32>,
    pub attested_at: Option<i64>,
}

pub fn app_consent_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(APP_CONSENT_FILE_EXTENSION)
}

/// ファイル欠落・破損・旧形式はいずれも未同意へ倒す。
pub async fn load_app_consent_store(db_path: &Path) -> AppConsentStore {
    let Ok(Some(bytes)) = crate::storage::read_file(&app_consent_path(db_path)).await else {
        return AppConsentStore::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub async fn save_app_consent_store(db_path: &Path, store: &AppConsentStore) -> Result<(), String> {
    let path = app_consent_path(db_path);
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("failed to encode consent record: {error}"))?;
    crate::storage::write_file(&path, &bytes)
        .await
        .map_err(|error| {
            format!(
                "failed to write consent record `{}`: {error:#}",
                path.display()
            )
        })
}

pub async fn reset_app_consent_at_path(db_path: &Path) -> Result<ClientStartupStatus, String> {
    let store = AppConsentStore::default();
    save_app_consent_store(db_path, &store).await?;
    Ok(consent_required_status(&store))
}

pub fn consent_required_status(store: &AppConsentStore) -> ClientStartupStatus {
    ClientStartupStatus::ConsentRequired {
        documents: app_consent_documents_status(store),
        age_attestation: age_attestation_status(store),
    }
}

fn latest_app_consent_record<'a>(
    store: &'a AppConsentStore,
    slug: &str,
) -> Option<&'a AppConsentDocumentRecord> {
    store
        .records
        .iter()
        .filter(|record| record.slug == slug)
        .max_by_key(|record| (record.version, record.accepted_at))
}

pub fn app_consent_documents_status(store: &AppConsentStore) -> Vec<AppConsentDocumentStatus> {
    let legal: DistributionLegalMetadata = serde_json::from_str(include_str!(
        "../../../../apps/desktop/src-tauri/distribution/legal.json"
    ))
    .expect("distribution legal metadata must be valid");
    APP_LEGAL_DOCUMENTS
        .iter()
        .map(|(slug, current_version)| {
            let latest = latest_app_consent_record(store, slug);
            AppConsentDocumentStatus {
                slug: (*slug).to_string(),
                current_version: *current_version,
                effective_date: APP_LEGAL_EFFECTIVE_DATE.to_string(),
                authoritative_language: APP_LEGAL_AUTHORITATIVE_LANGUAGE.to_string(),
                material_change: true,
                controller_name: legal.controller_name.clone(),
                contact: legal.contact.clone(),
                accepted_version: latest.map(|record| record.version),
                accepted_at: latest.map(|record| record.accepted_at),
                accepted_language: latest.map(|record| record.language.clone()),
                accepted_app_version: latest.map(|record| record.app_version.clone()),
            }
        })
        .collect()
}

pub fn app_consent_documents_satisfied(store: &AppConsentStore) -> bool {
    APP_LEGAL_DOCUMENTS.iter().all(|(slug, current_version)| {
        latest_app_consent_record(store, slug)
            .map(|record| record.version >= *current_version)
            .unwrap_or(false)
    })
}

pub fn age_attestation_satisfied(store: &AppConsentStore) -> bool {
    latest_age_attestation_record(store)
        .map(|record| record.version >= AGE_ATTESTATION_VERSION)
        .unwrap_or(false)
}

fn latest_age_attestation_record(store: &AppConsentStore) -> Option<&AgeAttestationRecord> {
    store
        .age_attestations
        .iter()
        .max_by_key(|record| (record.version, record.attested_at))
}

pub fn age_attestation_status(store: &AppConsentStore) -> AgeAttestationStatus {
    let latest = latest_age_attestation_record(store);
    AgeAttestationStatus {
        current_version: AGE_ATTESTATION_VERSION,
        attested_version: latest.map(|record| record.version),
        attested_at: latest.map(|record| record.attested_at),
    }
}

pub fn app_consent_satisfied(store: &AppConsentStore) -> bool {
    app_consent_documents_satisfied(store) && age_attestation_satisfied(store)
}

pub fn current_unix_seconds() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClientStartupStatus {
    Initializing,
    Ready,
    ConsentRequired {
        documents: Vec<AppConsentDocumentStatus>,
        age_attestation: AgeAttestationStatus,
    },
    Failed {
        error: ClientStartupErrorView,
    },
    /// 同じ origin の別の tab が runtime を動かしている（Web だけ。ADR 0059 §4）。
    InUseElsewhere,
    /// 保存先の database の版を更新している（Web だけ。画面は「データの移行中です」で待つ。ADR 0059 §1）。
    Migrating,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClientStartupErrorView {
    pub kind: ClientStartupErrorKind,
    pub message: String,
    pub detail: String,
    pub db_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientStartupErrorKind {
    DatabaseOpen,
    DatabaseMigration,
    ProfileInUse,
    ProfileInvalid,
    SubscriptionState,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_or_invalid_consent_is_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("kukuri.db");
        assert!(!app_consent_satisfied(
            &load_app_consent_store(&db_path).await
        ));

        std::fs::write(app_consent_path(&db_path), b"not-json").expect("write invalid consent");
        assert!(!app_consent_satisfied(
            &load_app_consent_store(&db_path).await
        ));
    }
}

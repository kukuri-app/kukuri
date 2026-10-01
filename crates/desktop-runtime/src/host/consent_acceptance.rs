use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::paths::AppBuildProfile;

use super::{
    AGE_ATTESTATION_VERSION, APP_LEGAL_DOCUMENTS, AgeAttestationRecord, AgeAttestationStatus,
    AppConsentDocumentRecord, AppConsentDocumentStatus, ClientStartupStatus,
    age_attestation_satisfied, age_attestation_status, app_consent_documents_status,
    app_consent_satisfied, current_unix_seconds, load_app_consent_store, save_app_consent_store,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConsentStatus {
    pub documents: Vec<AppConsentDocumentStatus>,
    pub age_attestation: AgeAttestationStatus,
    pub satisfied: bool,
}

/// 同意リクエストの文書単位エントリ(#857)。ユーザーが実際に提示された slug と
/// 版をそのまま返してもらい、サーバ側(=このコマンド)で現行版と照合する。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedAppConsentDocument {
    pub slug: String,
    pub version: i32,
}

pub fn require_consent_acceptance_state(status: &ClientStartupStatus) -> Result<(), String> {
    if matches!(status, ClientStartupStatus::ConsentRequired { .. }) {
        Ok(())
    } else {
        Err(format!(
            "app consent can only be accepted from ConsentRequired; current state is {status:?}"
        ))
    }
}

pub fn validate_app_consent_documents(
    documents: &[AcceptedAppConsentDocument],
) -> Result<(), String> {
    for (slug, current_version) in APP_LEGAL_DOCUMENTS {
        let accepted = documents
            .iter()
            .find(|document| document.slug == *slug)
            .ok_or_else(|| format!("consent for document `{slug}` is missing"))?;
        if accepted.version < *current_version {
            return Err(format!(
                "consent version {} for document `{slug}` is older than the current version {current_version}",
                accepted.version
            ));
        }
    }
    for document in documents {
        if !APP_LEGAL_DOCUMENTS
            .iter()
            .any(|(slug, _)| *slug == document.slug)
        {
            return Err(format!("unknown consent document `{}`", document.slug));
        }
    }

    Ok(())
}

pub async fn record_app_consents(
    db_path: &Path,
    documents: &[AcceptedAppConsentDocument],
    language: &str,
    age_attested: bool,
    app_version: &str,
) -> Result<(), String> {
    let mut store = load_app_consent_store(db_path).await;

    // #858: 18歳以上の自己申告は文書同意とは別の必須行為。今回のリクエストで
    // 申告されたか、過去に現行版で申告済みのどちらかが必要(fail-closed)。
    if !age_attested && !age_attestation_satisfied(&store) {
        return Err("age attestation is required to use kukuri".to_string());
    }

    let accepted_at = current_unix_seconds();
    let build_profile = Some(AppBuildProfile::current().as_str().to_string());
    if age_attested {
        // 同一版の申告は日時等を更新し、それ以外は履歴として残す。
        if let Some(existing) = store
            .age_attestations
            .iter_mut()
            .find(|record| record.version == AGE_ATTESTATION_VERSION)
        {
            existing.attested_at = accepted_at;
            existing.language = language.to_string();
            existing.app_version = app_version.to_string();
            existing.build_profile = build_profile.clone();
        } else {
            store.age_attestations.push(AgeAttestationRecord {
                version: AGE_ATTESTATION_VERSION,
                attested_at: accepted_at,
                language: language.to_string(),
                app_version: app_version.to_string(),
                build_profile: build_profile.clone(),
            });
        }
    }
    for document in documents {
        // 同一 slug+version の記録は日時等を更新し、それ以外は履歴として残す。
        if let Some(existing) = store
            .records
            .iter_mut()
            .find(|record| record.slug == document.slug && record.version == document.version)
        {
            existing.accepted_at = accepted_at;
            existing.language = language.to_string();
            existing.app_version = app_version.to_string();
            existing.build_profile = build_profile.clone();
        } else {
            store.records.push(AppConsentDocumentRecord {
                slug: document.slug.clone(),
                version: document.version,
                accepted_at,
                language: language.to_string(),
                app_version: app_version.to_string(),
                build_profile: build_profile.clone(),
            });
        }
    }
    save_app_consent_store(db_path, &store).await?;

    Ok(())
}

pub async fn app_consent_status(db_path: &Path) -> AppConsentStatus {
    let store = load_app_consent_store(db_path).await;
    AppConsentStatus {
        satisfied: app_consent_satisfied(&store),
        age_attestation: age_attestation_status(&store),
        documents: app_consent_documents_status(&store),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{
        ClientStartupError, consent_required_status, failed_startup_status as failed_status,
    };

    #[test]
    fn consent_acceptance_is_only_allowed_from_consent_required() {
        assert!(
            require_consent_acceptance_state(&consent_required_status(&Default::default())).is_ok()
        );
        for status in [
            ClientStartupStatus::Initializing,
            ClientStartupStatus::Ready,
            failed_status(ClientStartupError::unknown("failed".to_string()), None),
        ] {
            assert!(require_consent_acceptance_state(&status).is_err());
        }
    }

    #[tokio::test]
    async fn missing_age_attestation_does_not_create_or_mutate_consent_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let db_path = directory.path().join("kukuri.db");
        let consent_path = crate::host::app_consent_path(&db_path);
        let documents = crate::host::APP_LEGAL_DOCUMENTS
            .iter()
            .map(|(slug, version)| AcceptedAppConsentDocument {
                slug: (*slug).to_string(),
                version: *version,
            })
            .collect::<Vec<_>>();
        validate_app_consent_documents(&documents).expect("current documents");
        assert!(
            record_app_consents(&db_path, &documents, "en", false, "test")
                .await
                .is_err()
        );
        assert!(!consent_path.exists());

        // 壊れた保存状態でも失敗前に書き直さない。
        let original = b"invalid consent record";
        std::fs::write(&consent_path, original).expect("write fixture");
        assert!(
            record_app_consents(&db_path, &documents, "en", false, "test")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(&consent_path).expect("read fixture"),
            original
        );

        record_app_consents(&db_path, &documents, "en", true, "test")
            .await
            .expect("explicit attestation");
        let saved = app_consent_status(&db_path).await;
        assert!(saved.satisfied);
        let attested_at = saved.age_attestation.attested_at;
        record_app_consents(&db_path, &documents, "ja", false, "test")
            .await
            .expect("renewed consent");
        let renewed = app_consent_status(&db_path).await;
        assert!(renewed.satisfied);
        assert_eq!(renewed.age_attestation.attested_at, attested_at);
    }

    #[tokio::test]
    async fn recorded_consent_keeps_build_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let db_path = directory.path().join("kukuri.db");
        let documents = crate::host::APP_LEGAL_DOCUMENTS
            .iter()
            .map(|(slug, version)| AcceptedAppConsentDocument {
                slug: (*slug).to_string(),
                version: *version,
            })
            .collect::<Vec<_>>();
        record_app_consents(&db_path, &documents, "en", true, "test")
            .await
            .expect("consent");

        let store = crate::host::load_app_consent_store(&db_path).await;
        let expected = Some(AppBuildProfile::current().as_str().to_string());
        assert_eq!(store.records.len(), documents.len());
        assert!(
            store
                .records
                .iter()
                .all(|record| record.build_profile == expected)
        );
        assert_eq!(store.age_attestations.len(), 1);
        assert_eq!(store.age_attestations[0].build_profile, expected);
    }

    #[tokio::test]
    async fn consent_version_check_ignores_build_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let db_path = directory.path().join("kukuri.db");
        let consent_path = crate::host::app_consent_path(&db_path);
        let fixture = |version: i32, build_profile: Option<&str>| {
            let build_profile = build_profile
                .map(|value| format!(r#","build_profile":"{value}""#))
                .unwrap_or_default();
            let records = crate::host::APP_LEGAL_DOCUMENTS
                .iter()
                .map(|(slug, _)| {
                    format!(
                        r#"{{"slug":"{slug}","version":{version},"accepted_at":1,"language":"ja","app_version":"0.2.4"{build_profile}}}"#
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                r#"{{"records":[{records}],"age_attestations":[{{"version":{},"attested_at":1,"language":"ja","app_version":"0.2.4"{build_profile}}}]}}"#,
                crate::host::AGE_ATTESTATION_VERSION
            )
        };
        let current = crate::host::LEGAL_BUNDLE_VERSION;
        // #1105 より前の記録(build_profile なし)も、版が一致すれば同意済みのまま。
        // 版が古ければ build の種別に関係なく再同意を求める。
        for (version, build_profile, satisfied) in [
            (current, None, true),
            (current, Some("release"), true),
            (current, Some("development"), true),
            (current - 1, None, false),
            (current - 1, Some("release"), false),
            (current - 1, Some("development"), false),
        ] {
            std::fs::write(&consent_path, fixture(version, build_profile)).expect("fixture");
            let status = app_consent_status(&db_path).await;
            assert_eq!(
                status.satisfied, satisfied,
                "version {version}, build profile {build_profile:?}"
            );
            let store = crate::host::load_app_consent_store(&db_path).await;
            assert_eq!(store.records.len(), crate::host::APP_LEGAL_DOCUMENTS.len());
            assert!(
                store
                    .records
                    .iter()
                    .all(|record| record.build_profile.as_deref() == build_profile)
            );
        }
    }
}

//! scan 対象ごとの最新 safety verdict state の永続化（#404）。
//!
//! signed moderation event / risk signal（#405）は非 allow 時のみ生成される artifact であり、
//! 「対象の最新判定そのもの」は残らなかった。ADR 0025 §2.5 は「index entry は対応する safety
//! verdict state を必ず伴う」ことを要求するため、`allow` を含む全 scan の最新 verdict を
//! `cn_safety.scan_verdicts` に対象ごと 1 行で保持する。
//!
//! 行は対象（subject_kind, subject_id）ごとに upsert し、id は初回採番のまま据え置く。これにより
//! index 真実源（`cn_index.index_entries`）からの FK 参照が常に最新 verdict を指し、query 境界の
//! fail-closed 再確認（join して現在の action / critical を見る）が成立する。
//!
//! #1050 で再利用鍵（内容 fingerprint / scan 構成 fingerprint）と descriptive 検索タグを同じ行に
//! 持たせた。cn-indexer はこれを読み戻し、内容と構成が不変なら provider を呼ばずに再利用する。

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use uuid::Uuid;

use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{
    AdvisorySubjectKind, ContentAdvisory, ReasonCode, SafetyAction, SafetyLabel, SafetyVerdict,
};
use kukuri_cn_safety_runtime::{StoredVerdictRecord, VerdictPersistMeta};

use crate::safety_events::{from_db_enum, to_db_enum};

/// 永続化された scan 対象の最新 verdict。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredScanVerdict {
    /// verdict record id（初回採番のまま据え置き。index entry の FK 参照先）。
    pub id: String,
    /// scan 対象種別。
    pub subject_kind: SubjectKind,
    /// scan 対象識別子。
    pub subject_id: String,
    /// 最終 action。
    pub action: SafetyAction,
    /// critical safety（CSAM / CSE / grooming）検知か。
    pub critical: bool,
    /// reason code。
    pub reason_code: ReasonCode,
    /// classifier confidence（0-100。任意）。
    pub confidence: Option<u8>,
    /// 判定した provider 名（fail-closed 経路では None のことがある）。
    pub provider: Option<String>,
    /// policy バージョン。
    pub policy_version: String,
    /// scan 時刻（RFC3339 文字列。`SafetyVerdict::scanned_at` の原文）。
    pub scanned_at: String,
    /// この行を最後に更新した時刻。
    pub updated_at: DateTime<Utc>,
    /// 保存時の内容 fingerprint（#1050。旧行は None）。
    pub source_fingerprint: Option<String>,
    /// 保存時の scan 構成 fingerprint（#1050。旧行は None）。
    pub scan_config_fingerprint: Option<String>,
    /// 同じ scan で確定した descriptive 検索タグ（`allow` のみ非空。#1050）。
    pub derived_tags: Vec<String>,
    /// content advisory（ADR 0028 §8.6。#1054）。post 行は本文 text と参照 blob の和集合。
    pub advisory_labels: Vec<ContentAdvisory>,
}

impl StoredScanVerdict {
    /// この verdict が index / discovery / recommendation への surfacing を許すか。
    ///
    /// `SafetyAction::allows_indexing()`（単一判定点）に委譲する。
    pub fn is_indexable(&self) -> bool {
        self.action.allows_indexing()
    }

    /// この行の subject 自身の advisory（post 行に同梱された参照 blob 分を除く）。
    pub fn own_advisories(&self) -> Vec<ContentAdvisory> {
        let own_kind = match self.subject_kind {
            SubjectKind::Post => AdvisorySubjectKind::PostId,
            SubjectKind::Blob => AdvisorySubjectKind::BlobCid,
            SubjectKind::User | SubjectKind::Peer => return Vec::new(),
        };
        self.advisory_labels
            .iter()
            .filter(|advisory| {
                advisory.subject_kind == own_kind && advisory.subject_id == self.subject_id
            })
            .cloned()
            .collect()
    }

    /// 再利用判定の入力へ写す。labels / provider_capability は永続化していないため空になる。
    /// `advisory_labels` は保存済み advisory の自 subject 分から復元する（再利用時も
    /// `is_labeled_allow()` が成立する）。
    pub fn to_record(&self) -> StoredVerdictRecord {
        let advisory_labels = self
            .own_advisories()
            .into_iter()
            .map(|advisory| {
                let mut label = SafetyLabel::new(advisory.category);
                if let Some(confidence) = advisory.confidence {
                    label = label.with_confidence(confidence);
                }
                label
            })
            .collect();
        StoredVerdictRecord {
            id: self.id.clone(),
            verdict: SafetyVerdict {
                action: self.action,
                labels: Vec::new(),
                advisory_labels,
                critical: self.critical,
                reason_code: self.reason_code,
                confidence: self.confidence,
                provider: self.provider.clone(),
                provider_capability: None,
                policy_version: self.policy_version.clone(),
                scanned_at: self.scanned_at.clone(),
            },
            derived_tags: self.derived_tags.clone(),
            advisories: self.advisory_labels.clone(),
            source_fingerprint: self.source_fingerprint.clone(),
            scan_config_fingerprint: self.scan_config_fingerprint.clone(),
        }
    }
}

const SCAN_VERDICT_COLUMNS: &str = "id, subject_kind, subject_id, action, critical, reason_code, \
     confidence, provider, policy_version, scanned_at, updated_at, source_fingerprint, \
     scan_config_fingerprint, derived_tags, advisory_labels";

/// scan 対象の最新 verdict を upsert する（対象ごとに 1 行。id は初回採番のまま据え置き）。
///
/// `meta` の fingerprint / タグは再利用判定（#1050）のために毎回上書きする。
pub async fn upsert_scan_verdict(
    pool: &PgPool,
    subject_kind: SubjectKind,
    subject_id: &str,
    verdict: &SafetyVerdict,
    meta: &VerdictPersistMeta,
) -> Result<StoredScanVerdict> {
    if subject_id.trim().is_empty() {
        bail!("scan verdict subject_id must not be empty");
    }
    let id = Uuid::new_v4().to_string();
    let derived_tags = serde_json::to_value(&meta.derived_tags)?;
    let advisory_labels = serde_json::to_value(&meta.advisories)?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO cn_safety.scan_verdicts
            (id, subject_kind, subject_id, action, critical, reason_code, confidence, provider,
             policy_version, scanned_at, source_fingerprint, scan_config_fingerprint, derived_tags,
             advisory_labels)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT (subject_kind, subject_id) DO UPDATE
         SET action = EXCLUDED.action,
             critical = EXCLUDED.critical,
             reason_code = EXCLUDED.reason_code,
             confidence = EXCLUDED.confidence,
             provider = EXCLUDED.provider,
             policy_version = EXCLUDED.policy_version,
             scanned_at = EXCLUDED.scanned_at,
             source_fingerprint = EXCLUDED.source_fingerprint,
             scan_config_fingerprint = EXCLUDED.scan_config_fingerprint,
             derived_tags = EXCLUDED.derived_tags,
             advisory_labels = EXCLUDED.advisory_labels,
             updated_at = NOW()
         RETURNING {SCAN_VERDICT_COLUMNS}"
    )))
    .bind(&id)
    .bind(to_db_enum(&subject_kind)?)
    .bind(subject_id)
    .bind(to_db_enum(&verdict.action)?)
    .bind(verdict.critical)
    .bind(to_db_enum(&verdict.reason_code)?)
    .bind(verdict.confidence.map(i16::from))
    .bind(verdict.provider.as_deref())
    .bind(&verdict.policy_version)
    .bind(&verdict.scanned_at)
    .bind(meta.source_fingerprint.as_deref())
    .bind(meta.scan_config_fingerprint.as_deref())
    .bind(derived_tags)
    .bind(advisory_labels)
    .fetch_one(pool)
    .await?;
    scan_verdict_from_row(&row)
}

/// scan 対象の最新 verdict 行の content advisory を差し替える（ADR 0028 §8.3。#1054）。
///
/// indexer が post 本文と参照 blob の advisory の和集合を post 行へ確定させるために使う。
/// 値が同じなら書かない（`updated_at` も動かさない）。verdict 行が無ければ何もしない。
pub async fn update_scan_verdict_advisories(
    pool: &PgPool,
    subject_kind: SubjectKind,
    subject_id: &str,
    advisories: &[ContentAdvisory],
) -> Result<()> {
    let advisory_labels = serde_json::to_value(advisories)?;
    sqlx::query(
        "UPDATE cn_safety.scan_verdicts
         SET advisory_labels = $3
         WHERE subject_kind = $1 AND subject_id = $2
           AND advisory_labels IS DISTINCT FROM $3",
    )
    .bind(to_db_enum(&subject_kind)?)
    .bind(subject_id)
    .bind(advisory_labels)
    .execute(pool)
    .await?;
    Ok(())
}

/// scan 対象の最新 verdict を取得する。
pub async fn get_scan_verdict(
    pool: &PgPool,
    subject_kind: SubjectKind,
    subject_id: &str,
) -> Result<Option<StoredScanVerdict>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {SCAN_VERDICT_COLUMNS}
         FROM cn_safety.scan_verdicts
         WHERE subject_kind = $1 AND subject_id = $2"
    )))
    .bind(to_db_enum(&subject_kind)?)
    .bind(subject_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(scan_verdict_from_row).transpose()
}

fn scan_verdict_from_row(row: &PgRow) -> Result<StoredScanVerdict> {
    let confidence: Option<i16> = row.try_get("confidence")?;
    let derived_tags: serde_json::Value = row.try_get("derived_tags")?;
    let derived_tags: Vec<String> = serde_json::from_value(derived_tags)?;
    let advisory_labels: serde_json::Value = row.try_get("advisory_labels")?;
    let advisory_labels: Vec<ContentAdvisory> = serde_json::from_value(advisory_labels)?;
    Ok(StoredScanVerdict {
        id: row.try_get("id")?,
        subject_kind: from_db_enum("subject_kind", &row.try_get::<String, _>("subject_kind")?)?,
        subject_id: row.try_get("subject_id")?,
        action: from_db_enum("action", &row.try_get::<String, _>("action")?)?,
        critical: row.try_get("critical")?,
        reason_code: from_db_enum("reason_code", &row.try_get::<String, _>("reason_code")?)?,
        confidence: confidence.map(|v| v as u8),
        provider: row.try_get("provider")?,
        policy_version: row.try_get("policy_version")?,
        scanned_at: row.try_get("scanned_at")?,
        updated_at: row.try_get("updated_at")?,
        source_fingerprint: row.try_get("source_fingerprint")?,
        scan_config_fingerprint: row.try_get("scan_config_fingerprint")?,
        derived_tags,
        advisory_labels,
    })
}

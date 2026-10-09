//! signed moderation event / risk signal の永続化と visibility 配布境界（#405）。
//!
//! community node は自分の authority scope 内の判断を signed moderation event として保存・配布でき、
//! risk signal を trustness / relation 反映のために保存する。いずれも issuer node の advisory であり
//! network-wide command ではない（`docs/adr/0027-deterministic-moderation-critical-safety.md` §2.1）。
//!
//! 配布境界は visibility（`local` / `subscribed_nodes` / `public`）で決まる。
//! - `local` は issuer node の外へ出さない（配布クエリは返さない）。
//! - `subscribed_nodes` は購読 node に配布する。
//! - `public` は公開 advisory。
//!
//! suspected unknown CSAM / CSE は `local` 既定であり、誤検知を public advisory として拡散しない。
//! risk signal は `expires_at` 失効後は配布対象から除外する。
//!
//! enum 列は `cn-safety` の serde 表現（snake_case）と一致させるため、serde を経由して文字列化・
//! 復元する。これにより列値と canonical 表現の drift を防ぎ、ロード後も moderation event の署名検証が
//! 通る（body を列から型として復元し、`canonical_bytes()` が決定論的に再シリアライズする）。

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use uuid::Uuid;

use kukuri_cn_safety::event::{ModerationEventBody, SignedModerationEvent};
use kukuri_cn_safety::verdict::SafetyLabel;
use kukuri_cn_safety::{AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal};
use kukuri_cn_safety_runtime::{superseded_advisory_categories, verify_signed_event};

/// 配布クエリの受け手区分。`local` はどの audience にも配布しない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistributionAudience {
    /// この node を trust input として購読している node。`subscribed_nodes` と `public` を見る。
    SubscribedNodes,
    /// 公開 advisory の受け手。`public` のみを見る。
    Public,
}

impl DistributionAudience {
    /// この audience に配布してよい visibility 文字列の集合。
    fn allowed_visibilities(self) -> Vec<String> {
        match self {
            DistributionAudience::SubscribedNodes => {
                vec!["subscribed_nodes".to_string(), "public".to_string()]
            }
            DistributionAudience::Public => vec!["public".to_string()],
        }
    }
}

/// 永続化された signed moderation event。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredModerationEvent {
    /// 復元した署名済み event（body + signature）。ロード後も署名検証できる。
    pub event: SignedModerationEvent,
    /// 永続化時刻（署名対象ではない）。
    pub persisted_at: DateTime<Utc>,
}

/// 永続化された risk signal。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRiskSignal {
    /// 永続化側が採番した id。
    pub id: String,
    /// この signal を保持する issuer node。
    pub issuer_node_id: String,
    /// risk signal 本体。
    pub signal: SafetyRiskSignal,
    /// 永続化時刻。
    pub persisted_at: DateTime<Utc>,
    /// operator が審査・運用是正で値を確定した最後の時刻（#1058）。`None` は scanner 由来の
    /// 未訂正行。印のある行は再 scan の集約更新・新規 insert の対象にならない。
    pub operator_adjusted_at: Option<DateTime<Utc>>,
    /// 最初の訂正前の scanner 由来 category（#1058）。印のある行では常に `Some`。
    pub operator_origin_category: Option<SafetyCategory>,
}

/// signed moderation event を保存する（event id で冪等）。
///
/// 同一 id が既に存在する場合は上書きせず（最初の writer が権威）、既存レコードを返す。
/// `target_id` が空 / 空白の event は保存しない。
///
/// 保存前に署名を検証する（trust boundary）。`body.issuer_node_id` の公開鍵で canonical digest の
/// schnorr 署名を検証し、改竄 / 別鍵 / issuer 詐称の event は保存しない。これにより、配布クエリが
/// visibility だけで返すレコードが常に検証済みであることを保証する。
pub async fn persist_signed_moderation_event(
    pool: &PgPool,
    event: &SignedModerationEvent,
) -> Result<StoredModerationEvent> {
    let body = &event.body;
    if body.id.trim().is_empty() {
        bail!("moderation event id must not be empty");
    }
    if body.target_id.trim().is_empty() {
        bail!("moderation event target_id must not be empty");
    }
    verify_signed_event(event)
        .map_err(|err| anyhow!("refusing to persist unverified moderation event: {err}"))?;

    let labels =
        serde_json::to_value(&body.labels).context("failed to encode moderation labels")?;
    sqlx::query(
        "INSERT INTO cn_safety.signed_moderation_events
            (id, issuer_node_id, target_type, target_id, action, reason_code, severity, basis,
             visibility, confidence, policy_version, labels, signature, event_created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&body.id)
    .bind(&body.issuer_node_id)
    .bind(to_db_enum(&body.target_type)?)
    .bind(&body.target_id)
    .bind(to_db_enum(&body.action)?)
    .bind(to_db_enum(&body.reason_code)?)
    .bind(to_db_enum(&body.severity)?)
    .bind(to_db_enum(&body.basis)?)
    .bind(to_db_enum(&body.visibility)?)
    .bind(body.confidence.map(i16::from))
    .bind(&body.policy_version)
    .bind(labels)
    .bind(&event.signature)
    .bind(&body.created_at)
    .execute(pool)
    .await?;

    // 冪等のため、保存後は常に id で再取得して権威レコードを返す。
    get_signed_moderation_event(pool, &body.id)
        .await?
        .context("persisted moderation event disappeared")
}

/// signed moderation event を id で取得する。
pub async fn get_signed_moderation_event(
    pool: &PgPool,
    id: &str,
) -> Result<Option<StoredModerationEvent>> {
    let row = sqlx::query(
        "SELECT id, issuer_node_id, target_type, target_id, action, reason_code, severity, basis,
                visibility, confidence, policy_version, labels, signature, event_created_at, persisted_at
         FROM cn_safety.signed_moderation_events
         WHERE id = $1
           AND persisted_at > NOW() - cn_admin.retention_interval('moderation_event')",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(moderation_event_from_row).transpose()
}

/// signed moderation event を新着順で取得する（運営者の監査用。visibility を問わない）。
pub async fn list_signed_moderation_events(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<StoredModerationEvent>> {
    let rows = sqlx::query(
        "SELECT id, issuer_node_id, target_type, target_id, action, reason_code, severity, basis,
                visibility, confidence, policy_version, labels, signature, event_created_at, persisted_at
         FROM cn_safety.signed_moderation_events
         WHERE persisted_at > NOW() - cn_admin.retention_interval('moderation_event')
         ORDER BY persisted_at DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(moderation_event_from_row).collect()
}

/// 配布境界に従って配布可能な signed moderation event を返す。
///
/// `local` は決して返さない。audience が `SubscribedNodes` なら `subscribed_nodes` + `public`、
/// `Public` なら `public` のみ。
pub async fn list_distributable_moderation_events(
    pool: &PgPool,
    audience: DistributionAudience,
    limit: i64,
    offset: i64,
) -> Result<Vec<StoredModerationEvent>> {
    let rows = sqlx::query(
        "SELECT id, issuer_node_id, target_type, target_id, action, reason_code, severity, basis,
                visibility, confidence, policy_version, labels, signature, event_created_at, persisted_at
         FROM cn_safety.signed_moderation_events
         WHERE visibility = ANY($1)
           AND persisted_at > NOW() - cn_admin.retention_interval('moderation_event')
         ORDER BY persisted_at DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(audience.allowed_visibilities())
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(moderation_event_from_row).collect()
}

/// risk signal 永続化の結果（#1050）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedRiskSignal {
    /// 永続化された（または集約先となった）signal。
    pub stored: StoredRiskSignal,
    /// 新しい行を作ったか。既存の活性行へ集約した場合や、cleared 済み行を尊重して挿入を
    /// 見送った場合は false。
    pub newly_created: bool,
}

pub(crate) const RISK_SIGNAL_COLUMNS: &str = "id, issuer_node_id, target, target_id, category, \
     severity, basis, visibility, confidence, expires_at, appeal_status, persisted_at, \
     operator_adjusted_at, operator_origin_category";

/// risk signal を保存する。`target_id` が空 / 空白なら保存しない。
///
/// 同一鍵の活性 signal があれば新規行を作らず集約する（詳細は
/// [`persist_risk_signal_deduplicated`]）。
pub async fn persist_risk_signal(
    pool: &PgPool,
    issuer_node_id: &str,
    signal: &SafetyRiskSignal,
) -> Result<StoredRiskSignal> {
    persist_risk_signal_deduplicated(pool, issuer_node_id, signal, None)
        .await
        .map(|persisted| persisted.stored)
}

/// Persist a risk signal and, for content targets, atomically associate it with
/// the author whose trust calculation should consume the signal.
pub async fn persist_risk_signal_with_author(
    pool: &PgPool,
    issuer_node_id: &str,
    signal: &SafetyRiskSignal,
    subject_author: Option<&str>,
) -> Result<StoredRiskSignal> {
    persist_risk_signal_deduplicated(pool, issuer_node_id, signal, subject_author)
        .await
        .map(|persisted| persisted.stored)
}

/// risk signal を鍵 `(issuer_node_id, target, target_id, category, basis)` で集約して保存する
/// （#1050 AC-2 / INVAR-4）。
///
/// 1. 同鍵の活性行（`appeal_status` が cleared 以外 かつ `expires_at` 無し）があれば、その行の
///    severity / confidence / visibility を更新し、id / persisted_at / appeal_status は据え置く。
///    ただし operator が値を確定した行（`operator_adjusted_at` あり、#1058）は更新しない。
/// 2. 活性行が無く、operator が確定した行が同じ issuer / target / basis にあり、その category か
///    訂正前の category が一致すれば、失効・cleared を問わず新規行を作らずその行を返す（#1058）。
/// 3. 活性行が無く cleared 行だけがあれば、審査の結論を尊重して新規行を作らず cleared 行を返す。
/// 4. いずれも無ければ INSERT する。部分 UNIQUE index `uq_cn_safety_risk_signals_active_key`
///    との競合（同時挿入）は `ON CONFLICT ... DO UPDATE` で 1 の更新に倒す（operator が確定した
///    行とは競合しても更新しない）。
///
/// 著者関連付け（`risk_signal_subject_authors`）は集約の有無に関わらず同一取引で行う。
/// operator の訂正版は本関数ではなく `insert_operator_corrected_risk_signal` で保存する。
pub async fn persist_risk_signal_deduplicated(
    pool: &PgPool,
    issuer_node_id: &str,
    signal: &SafetyRiskSignal,
    subject_author: Option<&str>,
) -> Result<PersistedRiskSignal> {
    if issuer_node_id.trim().is_empty() {
        bail!("risk signal issuer_node_id must not be empty");
    }
    if signal.target_id.trim().is_empty() {
        bail!("risk signal target_id must not be empty");
    }
    if let Some(author) = subject_author {
        validate_subject_author(signal.target, author)?;
    }
    let target = to_db_enum(&signal.target)?;
    let category = to_db_enum(&signal.category)?;
    let basis = to_db_enum(&signal.basis)?;
    let severity = to_db_enum(&signal.severity)?;
    let visibility = to_db_enum(&signal.visibility)?;

    let mut tx = pool.begin().await?;
    let active = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RISK_SIGNAL_COLUMNS}
         FROM cn_safety.risk_signals
         WHERE issuer_node_id = $1 AND target = $2 AND target_id = $3
           AND category = $4 AND basis = $5
           AND appeal_status IS DISTINCT FROM 'cleared'
           AND expires_at IS NULL
         ORDER BY persisted_at ASC, id ASC
         LIMIT 1
         FOR UPDATE"
    )))
    .bind(issuer_node_id)
    .bind(&target)
    .bind(&signal.target_id)
    .bind(&category)
    .bind(&basis)
    .fetch_optional(&mut *tx)
    .await?;

    let (row, newly_created) = if let Some(active) = active {
        if row_is_operator_adjusted(&active)? {
            // operator が確定した値は scanner の値で上書きしない（#1058 AC-1）。行ロックを取って
            // から判定するため、同時に進む審査の更新とも直列化される。
            (active, false)
        } else {
            let id: String = active.try_get("id")?;
            let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE cn_safety.risk_signals
                 SET severity = $2, confidence = $3, visibility = $4
                 WHERE id = $1
                 RETURNING {RISK_SIGNAL_COLUMNS}"
            )))
            .bind(&id)
            .bind(&severity)
            .bind(signal.confidence.map(i16::from))
            .bind(&visibility)
            .fetch_one(&mut *tx)
            .await?;
            (row, false)
        }
    } else if let Some(adjusted) = sqlx::query(sqlx::AssertSqlSafe(format!(
        // category を変える訂正や期限付与の後は元の鍵に活性行が無い。訂正前の category
        // （operator_origin_category）でも一致させ、失効・cleared を問わず新規行を作らない
        // （#1058 AC-2）。別 category の新しい判定は抑止しない。
        "SELECT {RISK_SIGNAL_COLUMNS}
         FROM cn_safety.risk_signals
         WHERE issuer_node_id = $1 AND target = $2 AND target_id = $3 AND basis = $5
           AND operator_adjusted_at IS NOT NULL
           AND (category = $4 OR operator_origin_category = $4)
         ORDER BY (appeal_status IS DISTINCT FROM 'cleared' AND expires_at IS NULL) DESC,
                  operator_adjusted_at DESC, persisted_at DESC, id DESC
         LIMIT 1"
    )))
    .bind(issuer_node_id)
    .bind(&target)
    .bind(&signal.target_id)
    .bind(&category)
    .bind(&basis)
    .fetch_optional(&mut *tx)
    .await?
    {
        (adjusted, false)
    } else {
        // 失効していない cleared 行 = 現在も有効な審査結論。cn-cli の再発行は operator 専用の
        // 挿入経路を使うため、ここには掛からない。
        let cleared = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {RISK_SIGNAL_COLUMNS}
             FROM cn_safety.risk_signals
             WHERE issuer_node_id = $1 AND target = $2 AND target_id = $3
               AND category = $4 AND basis = $5
               AND appeal_status = 'cleared'
               AND expires_at IS NULL
             ORDER BY persisted_at DESC, id DESC
             LIMIT 1"
        )))
        .bind(issuer_node_id)
        .bind(&target)
        .bind(&signal.target_id)
        .bind(&category)
        .bind(&basis)
        .fetch_optional(&mut *tx)
        .await?;
        match cleared {
            Some(cleared) => (cleared, false),
            None => {
                let id = Uuid::new_v4().to_string();
                let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                    "INSERT INTO cn_safety.risk_signals
                        (id, issuer_node_id, target, target_id, category, severity, basis,
                         visibility, confidence, expires_at, appeal_status)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                     ON CONFLICT (issuer_node_id, target, target_id, category, basis)
                        WHERE appeal_status IS DISTINCT FROM 'cleared' AND expires_at IS NULL
                     DO UPDATE SET severity = EXCLUDED.severity,
                                   confidence = EXCLUDED.confidence,
                                   visibility = EXCLUDED.visibility
                        WHERE cn_safety.risk_signals.operator_adjusted_at IS NULL
                     RETURNING {RISK_SIGNAL_COLUMNS}, (xmax = 0) AS inserted"
                )))
                .bind(&id)
                .bind(issuer_node_id)
                .bind(&target)
                .bind(&signal.target_id)
                .bind(&category)
                .bind(&severity)
                .bind(&basis)
                .bind(&visibility)
                .bind(signal.confidence.map(i16::from))
                .bind(signal.expires_at.as_deref())
                .bind(signal.appeal_status.map(|s| to_db_enum(&s)).transpose()?)
                .fetch_optional(&mut *tx)
                .await?;
                match row {
                    Some(row) => {
                        let inserted: bool = row.try_get("inserted")?;
                        (row, inserted)
                    }
                    None => {
                        // 同時に operator が同鍵の訂正版を挿入した: その行を上書きせず返す。
                        let adjusted = sqlx::query(sqlx::AssertSqlSafe(format!(
                            "SELECT {RISK_SIGNAL_COLUMNS}
                             FROM cn_safety.risk_signals
                             WHERE issuer_node_id = $1 AND target = $2 AND target_id = $3
                               AND category = $4 AND basis = $5
                               AND appeal_status IS DISTINCT FROM 'cleared'
                               AND expires_at IS NULL"
                        )))
                        .bind(issuer_node_id)
                        .bind(&target)
                        .bind(&signal.target_id)
                        .bind(&category)
                        .bind(&basis)
                        .fetch_one(&mut *tx)
                        .await?;
                        (adjusted, false)
                    }
                }
            }
        }
    };
    if let Some(author) = subject_author {
        insert_subject_author(&mut tx, &target, &signal.target_id, author).await?;
    }
    tx.commit().await?;
    Ok(PersistedRiskSignal {
        stored: risk_signal_from_row(&row)?,
        newly_created,
    })
}

fn row_is_operator_adjusted(row: &PgRow) -> Result<bool> {
    Ok(row
        .try_get::<Option<DateTime<Utc>>, _>("operator_adjusted_at")?
        .is_some())
}

/// operator が訂正版として再発行する risk signal の内容（#1058）。
pub(crate) struct OperatorCorrectedRiskSignal<'a> {
    pub issuer_node_id: &'a str,
    pub target: &'a str,
    pub target_id: &'a str,
    pub category: &'a str,
    pub severity: &'a str,
    pub basis: &'a str,
    pub visibility: &'a str,
    pub confidence: Option<u8>,
    /// 旧行の訂正前 category（旧行が未訂正なら旧行の category）。
    pub origin_category: &'a str,
}

/// operator の訂正版を新しい活性行として挿入する（審査・cn-cli の再発行で共有、#1058）。
///
/// scanner の集約経路を通さず、operator 確定の印（`operator_adjusted_at`）と訂正前の category を
/// 付けて保存する。呼出元は旧行を先に cleared / 失効させ、同じ取引で呼ぶ。
pub(crate) async fn insert_operator_corrected_risk_signal(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    corrected: &OperatorCorrectedRiskSignal<'_>,
) -> Result<StoredRiskSignal> {
    let id = Uuid::new_v4().to_string();
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO cn_safety.risk_signals
            (id, issuer_node_id, target, target_id, category, severity, basis, visibility,
             confidence, expires_at, appeal_status, operator_adjusted_at,
             operator_origin_category)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, 'none', NOW(), $10)
         RETURNING {RISK_SIGNAL_COLUMNS}"
    )))
    .bind(&id)
    .bind(corrected.issuer_node_id)
    .bind(corrected.target)
    .bind(corrected.target_id)
    .bind(corrected.category)
    .bind(corrected.severity)
    .bind(corrected.basis)
    .bind(corrected.visibility)
    .bind(corrected.confidence.map(i16::from))
    .bind(corrected.origin_category)
    .fetch_one(&mut **tx)
    .await?;
    risk_signal_from_row(&row)
}

/// 再 scan の現在の判定に無い advisory-only category の scanner 由来 signal を失効させる
/// （#1109 / ADR 0028 §8.14）。失効させた件数を返す。
///
/// operator 確定の行（#1058）、`appeal_status` が `none` 以外の行、appeal 通報から参照される行
/// （棄却 = 判定維持を含む）は対象外。行ロック後に WHERE を再評価するため、同時に進む
/// 申し立て・operator 編集が先に確定すればその行は失効させない。
pub async fn expire_superseded_advisory_signals(
    pool: &PgPool,
    issuer_node_id: &str,
    target: RiskSignalTarget,
    target_id: &str,
    current_categories: &[SafetyCategory],
    expires_at: &str,
) -> Result<u64> {
    if issuer_node_id.trim().is_empty() || target_id.trim().is_empty() {
        bail!("risk signal issuer_node_id and target_id must not be empty");
    }
    DateTime::parse_from_rfc3339(expires_at)
        .with_context(|| format!("invalid expires_at `{expires_at}` (expected RFC3339)"))?;
    let superseded = superseded_advisory_categories(current_categories)
        .iter()
        .map(to_db_enum)
        .collect::<Result<Vec<_>>>()?;
    if superseded.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        "UPDATE cn_safety.risk_signals s
         SET expires_at = $5
         WHERE s.issuer_node_id = $1 AND s.target = $2 AND s.target_id = $3
           AND s.category = ANY($4)
           AND s.basis = $6
           AND s.expires_at IS NULL
           AND COALESCE(s.appeal_status, 'none') = 'none'
           AND s.operator_adjusted_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM cn_admin.reports r WHERE r.appeal_risk_signal_id = s.id
           )",
    )
    .bind(issuer_node_id)
    .bind(to_db_enum(&target)?)
    .bind(target_id)
    .bind(&superseded)
    .bind(expires_at)
    .bind(to_db_enum(&Basis::ClassifierScore)?)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// content target の risk signal を著者へ関連付ける（既にあれば何もしない）。
///
/// 保存済み verdict を再利用したとき（#1050）に、共有 blob の 2 人目の著者が trust 入力から
/// 漏れないようにするための入口。
pub async fn attribute_risk_signal_subject_author(
    pool: &PgPool,
    target: RiskSignalTarget,
    target_id: &str,
    author: &str,
) -> Result<()> {
    if target_id.trim().is_empty() {
        bail!("risk signal target_id must not be empty");
    }
    validate_subject_author(target, author)?;
    let mut tx = pool.begin().await?;
    insert_subject_author(&mut tx, &to_db_enum(&target)?, target_id, author).await?;
    tx.commit().await?;
    Ok(())
}

fn validate_subject_author(target: RiskSignalTarget, author: &str) -> Result<()> {
    if author.trim().is_empty() {
        bail!("risk signal subject author must not be empty");
    }
    if !matches!(target, RiskSignalTarget::PostId | RiskSignalTarget::BlobCid) {
        bail!("only post_id/blob_cid risk signals can be attributed to an author");
    }
    Ok(())
}

async fn insert_subject_author(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target: &str,
    target_id: &str,
    author: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_safety.risk_signal_subject_authors
            (target, target_id, author_pubkey)
         VALUES ($1, $2, $3)
         ON CONFLICT (target, target_id, author_pubkey) DO NOTHING",
    )
    .bind(target)
    .bind(target_id)
    .bind(author)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// risk signal を新着順で取得する（operator の監査・レビュー用。visibility を問わない）。
pub async fn list_risk_signals(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<StoredRiskSignal>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RISK_SIGNAL_COLUMNS}
         FROM cn_safety.risk_signals
         WHERE persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
         ORDER BY persisted_at DESC
         LIMIT $1 OFFSET $2"
    )))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(risk_signal_from_row).collect()
}

/// risk signal を id で取得する。
pub async fn get_risk_signal(pool: &PgPool, id: &str) -> Result<Option<StoredRiskSignal>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RISK_SIGNAL_COLUMNS}
         FROM cn_safety.risk_signals
         WHERE id = $1 AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(risk_signal_from_row).transpose()
}

/// 対象ごとの risk signal を新着順で取得する（visibility を問わない、node-local な参照）。
pub async fn list_risk_signals_for_target(
    pool: &PgPool,
    target: RiskSignalTarget,
    target_id: &str,
) -> Result<Vec<StoredRiskSignal>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RISK_SIGNAL_COLUMNS}
         FROM cn_safety.risk_signals
         WHERE target = $1 AND target_id = $2
           AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
         ORDER BY persisted_at DESC"
    )))
    .bind(to_db_enum(&target)?)
    .bind(target_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(risk_signal_from_row).collect()
}

/// Return both user-scoped signals and content-scoped signals attributed to
/// the user. The original content signal row is returned so expiry and appeal
/// state remain connected to the moderation artifact that produced it.
pub async fn list_risk_signals_for_user(
    pool: &PgPool,
    user_pubkey: &str,
) -> Result<Vec<StoredRiskSignal>> {
    let rows = sqlx::query(
        "SELECT DISTINCT rs.id, rs.issuer_node_id, rs.target, rs.target_id, rs.category,
                rs.severity, rs.basis, rs.visibility, rs.confidence, rs.expires_at,
                rs.appeal_status, rs.persisted_at, rs.operator_adjusted_at,
                rs.operator_origin_category
         FROM cn_safety.risk_signals rs
         LEFT JOIN cn_safety.risk_signal_subject_authors rsa
           ON rsa.target = rs.target AND rsa.target_id = rs.target_id
         WHERE ((rs.target = 'user_pubkey' AND rs.target_id = $1)
            OR rsa.author_pubkey = $1)
           AND rs.persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
         ORDER BY rs.persisted_at DESC",
    )
    .bind(user_pubkey)
    .fetch_all(pool)
    .await?;
    rows.iter().map(risk_signal_from_row).collect()
}

/// 配布境界に従って配布可能な risk signal を返す。
///
/// `local` は返さず、audience に応じて `subscribed_nodes` / `public` を返す。さらに `now_rfc3339`
/// 時点で `expires_at` が失効している signal は除外する（`expires_at` NULL は無期限で残る）。
pub async fn list_distributable_risk_signals(
    pool: &PgPool,
    audience: DistributionAudience,
    now_rfc3339: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<StoredRiskSignal>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {RISK_SIGNAL_COLUMNS}
         FROM cn_safety.risk_signals
         WHERE visibility = ANY($1)
           AND (expires_at IS NULL OR expires_at::timestamptz > $2::timestamptz)
           AND persisted_at > NOW() - cn_admin.retention_interval('risk_signal')
         ORDER BY persisted_at DESC
         LIMIT $3 OFFSET $4"
    )))
    .bind(audience.allowed_visibilities())
    .bind(now_rfc3339)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(risk_signal_from_row).collect()
}

fn moderation_event_from_row(row: &PgRow) -> Result<StoredModerationEvent> {
    let confidence: Option<i16> = row.try_get("confidence")?;
    let labels_value: Value = row.try_get("labels")?;
    let labels: Vec<SafetyLabel> =
        serde_json::from_value(labels_value).context("invalid stored moderation labels")?;
    let body = ModerationEventBody {
        id: row.try_get("id")?,
        issuer_node_id: row.try_get("issuer_node_id")?,
        target_type: from_db_enum("target_type", &row.try_get::<String, _>("target_type")?)?,
        target_id: row.try_get("target_id")?,
        action: from_db_enum("action", &row.try_get::<String, _>("action")?)?,
        labels,
        reason_code: from_db_enum("reason_code", &row.try_get::<String, _>("reason_code")?)?,
        severity: from_db_enum("severity", &row.try_get::<String, _>("severity")?)?,
        confidence: confidence.map(|v| v as u8),
        basis: from_db_enum("basis", &row.try_get::<String, _>("basis")?)?,
        visibility: from_db_enum("visibility", &row.try_get::<String, _>("visibility")?)?,
        policy_version: row.try_get("policy_version")?,
        created_at: row.try_get("event_created_at")?,
    };
    Ok(StoredModerationEvent {
        event: SignedModerationEvent {
            body,
            signature: row.try_get("signature")?,
        },
        persisted_at: row.try_get("persisted_at")?,
    })
}

pub(crate) fn risk_signal_from_row(row: &PgRow) -> Result<StoredRiskSignal> {
    let confidence: Option<i16> = row.try_get("confidence")?;
    let appeal_status: Option<String> = row.try_get("appeal_status")?;
    let appeal_status: Option<AppealStatus> = appeal_status
        .map(|s| from_db_enum("appeal_status", &s))
        .transpose()?;
    let signal = SafetyRiskSignal {
        target: from_db_enum("target", &row.try_get::<String, _>("target")?)?,
        target_id: row.try_get("target_id")?,
        category: from_db_enum("category", &row.try_get::<String, _>("category")?)?,
        severity: from_db_enum("severity", &row.try_get::<String, _>("severity")?)?,
        basis: from_db_enum("basis", &row.try_get::<String, _>("basis")?)?,
        confidence: confidence.map(|v| v as u8),
        visibility: from_db_enum("visibility", &row.try_get::<String, _>("visibility")?)?,
        expires_at: row.try_get("expires_at")?,
        appeal_status,
    };
    let operator_origin_category: Option<String> = row.try_get("operator_origin_category")?;
    Ok(StoredRiskSignal {
        id: row.try_get("id")?,
        issuer_node_id: row.try_get("issuer_node_id")?,
        signal,
        persisted_at: row.try_get("persisted_at")?,
        operator_adjusted_at: row.try_get("operator_adjusted_at")?,
        operator_origin_category: operator_origin_category
            .map(|value| from_db_enum("operator_origin_category", &value))
            .transpose()?,
    })
}

/// `cn-safety` の snake_case enum を DB 列文字列へ写す。
pub(crate) fn to_db_enum<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value).context("failed to encode enum value")? {
        Value::String(s) => Ok(s),
        other => bail!("expected snake_case string enum, got {other}"),
    }
}

/// DB 列文字列を `cn-safety` の snake_case enum へ戻す。
pub(crate) fn from_db_enum<T: DeserializeOwned>(field: &str, value: &str) -> Result<T> {
    serde_json::from_value(Value::String(value.to_string()))
        .with_context(|| format!("invalid stored `{field}` value `{value}`"))
}

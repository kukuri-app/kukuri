//! ブロック / ミュート観測の永続化と、relation 値の入力取得（ADR 0026 §8.3、#1061）。
//!
//! - 受付は observer 本人の署名済み観測だけを、任意文書 `trust_observation_sharing` への同意が
//!   有効な間だけ保存する。同意の確認と保存は observer 単位の advisory lock の下で同じ取引に置き、
//!   取消と並行した受付が取消後に残らないようにする。
//! - `(observer, target, kind)` ごとに `(observed_at_ms, envelope_id)` が新しい観測だけを採用する。
//! - 観測・observer は node-local で、cross-node pull や利用者向け read の本文には出さない。

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use chrono::{DateTime, Duration, TimeZone, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};

use kukuri_cn_protocol::TRUST_OBSERVATION_SHARING_POLICY_SLUG;
use kukuri_cn_protocol::normalize::normalize_pubkey;
use kukuri_cn_trust::{RelationObservation, RelationObservationKind};
use kukuri_core::{TrustObservation, TrustObservationKind};

/// revoked 観測を保持する日数（ADR 0026 §8.3）。
pub const REVOKED_TRUST_OBSERVATION_RETENTION_DAYS: i64 = 30;
/// active 観測を評価に使い、保持する日数。
pub const ACTIVE_TRUST_OBSERVATION_RETENTION_DAYS: i64 = 180;
/// 未来時刻として受け付ける許容差。これより先の観測は時計の誤りとして拒否する
/// （未来時刻の観測が後の解除を「古い」と誤判定させないため）。
pub const TRUST_OBSERVATION_MAX_CLOCK_SKEW_SECONDS: i64 = 300;
/// 評価で 1 対象あたりに読む active 観測の上限（新しい順）。
pub const RELATION_OBSERVATIONS_PER_TARGET_LIMIT: i64 = 200;

/// observer の観測提供の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustObservationSharingStatus {
    /// この node は任意文書を公開していない。
    NotOffered,
    /// 任意文書の現行版に同意していない、または同意後に取り消した。
    NotAccepted,
    Active,
}

/// 観測の保存結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreTrustObservationsOutcome {
    Rejected(TrustObservationSharingStatus),
    Stored { stored: u32, ignored: u32 },
}

fn kind_str(kind: TrustObservationKind) -> &'static str {
    kind.as_str()
}

fn relation_kind(raw: &str) -> Result<RelationObservationKind> {
    match raw {
        "block" => Ok(RelationObservationKind::Block),
        "mute" => Ok(RelationObservationKind::Mute),
        other => bail!("unknown trust observation kind `{other}`"),
    }
}

async fn lock_observer(tx: &mut Transaction<'_, Postgres>, observer: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cn_trust.observer:' || $1, 0))")
        .bind(observer)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn sharing_status_in<'e, E>(
    executor: E,
    observer: &str,
) -> Result<TrustObservationSharingStatus>
where
    E: sqlx::PgExecutor<'e>,
{
    let row = sqlx::query(
        "SELECT
            EXISTS (
                SELECT 1 FROM cn_admin.policies p
                WHERE p.policy_slug = $2 AND p.is_current = TRUE
            ) AS offered,
            EXISTS (
                SELECT 1
                FROM cn_admin.policies p
                JOIN cn_user.policy_consents c
                  ON c.policy_slug = p.policy_slug
                 AND c.policy_version = p.policy_version
                 AND c.subscriber_pubkey = $1
                 AND (
                   p.policy_snapshot_revision IS NULL
                   OR c.policy_snapshot_revision = p.policy_snapshot_revision
                 )
                WHERE p.policy_slug = $2 AND p.is_current = TRUE
            ) AS active",
    )
    .bind(observer)
    .bind(TRUST_OBSERVATION_SHARING_POLICY_SLUG)
    .fetch_one(executor)
    .await?;
    let offered: bool = row.try_get("offered")?;
    let active: bool = row.try_get("active")?;
    Ok(match (offered, active) {
        (false, _) => TrustObservationSharingStatus::NotOffered,
        (true, false) => TrustObservationSharingStatus::NotAccepted,
        (true, true) => TrustObservationSharingStatus::Active,
    })
}

/// observer の観測提供の状態を返す。
pub async fn trust_observation_sharing_status(
    pool: &PgPool,
    observer: &str,
) -> Result<TrustObservationSharingStatus> {
    let observer = normalize_pubkey(observer)?;
    sharing_status_in(pool, observer.as_str()).await
}

/// observer 本人の検証済み観測を保存する。
///
/// 呼出側は envelope の署名検証と「observer == bearer identity」を済ませてから渡す。本関数も
/// observer 以外の観測・未来時刻の観測を拒否する（Err）。提供の同意が有効でなければ何も書かない。
pub async fn store_trust_observations(
    pool: &PgPool,
    observer: &str,
    observations: &[TrustObservation],
    now: DateTime<Utc>,
) -> Result<StoreTrustObservationsOutcome> {
    let observer = normalize_pubkey(observer)?;
    let max_observed_at = now + Duration::seconds(TRUST_OBSERVATION_MAX_CLOCK_SKEW_SECONDS);
    let mut prepared = Vec::with_capacity(observations.len());
    for observation in observations {
        if observation.observer_pubkey.as_str() != observer {
            bail!("trust observation observer must match the authenticated identity");
        }
        let target = normalize_pubkey(observation.target_pubkey.as_str())?;
        if target == observer {
            bail!("self trust observation is not allowed");
        }
        let Some(observed_at) = Utc.timestamp_millis_opt(observation.observed_at).single() else {
            bail!("trust observation timestamp is out of range");
        };
        if observed_at > max_observed_at {
            bail!("trust observation timestamp is in the future");
        }
        prepared.push((target, observation, observed_at));
    }

    let mut tx = pool.begin().await?;
    lock_observer(&mut tx, observer.as_str()).await?;
    let status = sharing_status_in(&mut *tx, observer.as_str()).await?;
    if status != TrustObservationSharingStatus::Active {
        tx.rollback().await?;
        return Ok(StoreTrustObservationsOutcome::Rejected(status));
    }
    let mut stored = 0_u32;
    let mut ignored = 0_u32;
    for (target, observation, observed_at) in prepared {
        let updated = sqlx::query(
            "INSERT INTO cn_trust.observations
                (observer_pubkey, target_pubkey, kind, active, observed_at,
                 observed_at_ms, envelope_id, received_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (observer_pubkey, target_pubkey, kind) DO UPDATE SET
                active = EXCLUDED.active,
                observed_at = EXCLUDED.observed_at,
                observed_at_ms = EXCLUDED.observed_at_ms,
                envelope_id = EXCLUDED.envelope_id,
                received_at = EXCLUDED.received_at
             WHERE (cn_trust.observations.observed_at_ms, cn_trust.observations.envelope_id)
                 < (EXCLUDED.observed_at_ms, EXCLUDED.envelope_id)",
        )
        .bind(observer.as_str())
        .bind(target.as_str())
        .bind(kind_str(observation.kind))
        .bind(observation.active)
        .bind(observed_at)
        .bind(observation.observed_at)
        .bind(observation.envelope_id.0.as_str())
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            ignored += 1;
        } else {
            stored += 1;
        }
    }
    tx.commit().await?;
    Ok(StoreTrustObservationsOutcome::Stored { stored, ignored })
}

/// observer の観測をすべて削除し、観測提供の任意文書への同意の行（全版）を消す
/// （`DELETE /v1/trust/observations`）。再び同意するまで、受付も評価もその observer の観測を使わない。
pub async fn revoke_trust_observation_sharing(pool: &PgPool, observer: &str) -> Result<u64> {
    let observer = normalize_pubkey(observer)?;
    let mut tx = pool.begin().await?;
    lock_observer(&mut tx, observer.as_str()).await?;
    let deleted = sqlx::query("DELETE FROM cn_trust.observations WHERE observer_pubkey = $1")
        .bind(observer.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query(
        "DELETE FROM cn_user.policy_consents WHERE subscriber_pubkey = $1 AND policy_slug = $2",
    )
    .bind(observer.as_str())
    .bind(TRUST_OBSERVATION_SHARING_POLICY_SLUG)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(deleted)
}

/// 対象ごとの active 観測（提供同意が有効な observer のもの、保持期間内、新しい順に上限まで）。
pub async fn list_active_relation_observations(
    pool: &PgPool,
    targets: &[String],
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, Vec<RelationObservation>>> {
    let mut result: BTreeMap<String, Vec<RelationObservation>> = BTreeMap::new();
    if targets.is_empty() {
        return Ok(result);
    }
    let cutoff = now - Duration::days(ACTIVE_TRUST_OBSERVATION_RETENTION_DAYS);
    let rows = sqlx::query(
        "SELECT target_pubkey, observer_pubkey, kind, observed_at
         FROM (
            SELECT o.target_pubkey, o.observer_pubkey, o.kind, o.observed_at,
                   ROW_NUMBER() OVER (
                     PARTITION BY o.target_pubkey
                     ORDER BY o.observed_at DESC, o.observer_pubkey, o.kind
                   ) AS rank
            FROM cn_trust.observations o
            JOIN cn_admin.policies p
              ON p.policy_slug = $3 AND p.is_current = TRUE
            JOIN cn_user.policy_consents c
              ON c.policy_slug = p.policy_slug
             AND c.policy_version = p.policy_version
             AND c.subscriber_pubkey = o.observer_pubkey
             AND (
               p.policy_snapshot_revision IS NULL
               OR c.policy_snapshot_revision = p.policy_snapshot_revision
             )
            WHERE o.target_pubkey = ANY($1)
              AND o.active = TRUE
              AND o.observed_at > $2
         ) ranked
         WHERE rank <= $4
         ORDER BY target_pubkey, observed_at DESC, observer_pubkey, kind",
    )
    .bind(targets)
    .bind(cutoff)
    .bind(TRUST_OBSERVATION_SHARING_POLICY_SLUG)
    .bind(RELATION_OBSERVATIONS_PER_TARGET_LIMIT)
    .fetch_all(pool)
    .await?;
    for row in rows {
        let target: String = row.try_get("target_pubkey")?;
        let kind: String = row.try_get("kind")?;
        result.entry(target).or_default().push(RelationObservation {
            observer_pubkey: row.try_get("observer_pubkey")?,
            kind: relation_kind(kind.as_str())?,
            observed_at: row.try_get("observed_at")?,
        });
    }
    Ok(result)
}

/// 直近で成功した relation 解析の id（relation snapshot の版。未実行なら None）。
pub async fn latest_successful_relation_snapshot_id(pool: &PgPool) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM cn_admin.relation_analyze_runs
         WHERE success = TRUE
         ORDER BY finished_at DESC, id DESC
         LIMIT 1",
    )
    .fetch_optional(pool)
    .await?)
}

/// 保持期間を過ぎた観測を削除する。削除した行数を返す。
pub async fn cleanup_trust_observations(pool: &PgPool, now: DateTime<Utc>) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM cn_trust.observations
         WHERE (active = FALSE AND received_at <= $1)
            OR (active = TRUE AND observed_at <= $2)",
    )
    .bind(now - Duration::days(REVOKED_TRUST_OBSERVATION_RETENTION_DAYS))
    .bind(now - Duration::days(ACTIVE_TRUST_OBSERVATION_RETENTION_DAYS))
    .execute(pool)
    .await?
    .rows_affected())
}

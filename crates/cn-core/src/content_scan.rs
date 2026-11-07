use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_cn_safety::ProviderScanResult;
use kukuri_cn_safety_runtime::content_cache::{CONTENT_SCAN_LEASE, ContentScanStore};
use sqlx::{PgPool, Row};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PgContentScanStore {
    pool: PgPool,
}
impl PgContentScanStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ContentScanStore for PgContentScanStore {
    async fn load(&self, key: &str) -> Result<Option<Vec<ProviderScanResult>>> {
        validate_key(key)?;
        let row=sqlx::query("SELECT schema_version,scan_results FROM cn_safety.content_scan_cache WHERE cache_key=$1")
            .bind(key).fetch_optional(&self.pool).await?;
        let Some(row) = row else { return Ok(None) };
        if row.try_get::<i32, _>("schema_version")? != 1 {
            return Ok(None);
        }
        let value: serde_json::Value = row.try_get("scan_results")?;
        // Old/corrupt metadata is a miss, never a successful empty/clean scan.
        Ok(serde_json::from_value(value).ok())
    }

    async fn claim(&self, key: &str, owner: &str, lease: Duration) -> Result<bool> {
        validate_key(key)?;
        let owner = Uuid::parse_str(owner)?;
        ensure!(
            !lease.is_zero() && lease <= CONTENT_SCAN_LEASE,
            "invalid content scan lease"
        );
        // 基準時刻を固定し、expiry索引から一回128件だけ掃除する。
        sqlx::query(
            "DELETE FROM cn_safety.content_scan_claims WHERE ctid = ANY (ARRAY(
                 SELECT ctid FROM cn_safety.content_scan_claims
                 WHERE expires_at <= statement_timestamp() ORDER BY expires_at LIMIT 128))",
        )
        .execute(&self.pool)
        .await?;
        let row=sqlx::query("INSERT INTO cn_safety.content_scan_claims(cache_key,owner,expires_at)
            VALUES($1,$2::uuid,clock_timestamp()+$3 * interval '1 millisecond')
            ON CONFLICT(cache_key) DO UPDATE SET owner=EXCLUDED.owner,expires_at=EXCLUDED.expires_at
            WHERE cn_safety.content_scan_claims.expires_at <= clock_timestamp() RETURNING cache_key")
            .bind(key).bind(owner.to_string()).bind(lease.as_millis() as f64).fetch_optional(&self.pool).await?;
        Ok(row.is_some())
    }

    async fn complete(
        &self,
        key: &str,
        owner: &str,
        results: &[ProviderScanResult],
    ) -> Result<bool> {
        validate_key(key)?;
        let owner = Uuid::parse_str(owner)?;
        ensure!(
            !results.is_empty()
                && results.len() <= 16
                && results
                    .iter()
                    .all(|result| !result.outcome.is_fail_closed()),
            "incomplete content scan result"
        );
        let value = serde_json::to_value(results)?;
        ensure!(
            serde_json::to_vec(&value)?.len() <= 60 * 1024,
            "content scan metadata exceeds limit"
        );
        let mut tx = self.pool.begin().await?;
        let claim = sqlx::query(
            "SELECT cache_key FROM cn_safety.content_scan_claims
            WHERE cache_key=$1 AND owner=$2::uuid AND expires_at>clock_timestamp() FOR UPDATE",
        )
        .bind(key)
        .bind(owner.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if claim.is_none() {
            return Ok(false);
        }
        sqlx::query("INSERT INTO cn_safety.content_scan_cache(cache_key,scan_results) VALUES($1,$2)
            ON CONFLICT(cache_key) DO UPDATE SET schema_version=1,scan_results=EXCLUDED.scan_results,completed_at=clock_timestamp()")
            .bind(key).bind(value).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM cn_safety.content_scan_claims WHERE cache_key=$1 AND owner=$2::uuid",
        )
        .bind(key)
        .bind(owner.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn release(&self, key: &str, owner: &str) -> Result<()> {
        validate_key(key)?;
        sqlx::query(
            "DELETE FROM cn_safety.content_scan_claims WHERE cache_key=$1 AND owner=$2::uuid",
        )
        .bind(key)
        .bind(Uuid::parse_str(owner)?.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
fn validate_key(key: &str) -> Result<()> {
    ensure!(
        key.len() == 64
            && key
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid content scan key"
    );
    Ok(())
}

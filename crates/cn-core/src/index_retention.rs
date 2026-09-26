//! CN の再取得可能な保存物の受入下限・容量・保持期間（#1221 R5-F、2026-09-26 ユーザー決定）。
//!
//! 受入下限 W（unix 秒）は `cn_index.retention_state` に永続化し、単調に上げる。W = max(現在 − T、容量超過なら
//! W 以上で最古の行の時刻 + 1)。署名済み envelope の作成時刻が W 未満の投稿は索引の trigger が拒否し、W 未満の撤回
//! marker は残す必要が無い（同じ投稿は再び索引へ入らない）。回収は W 未満の関係のアクション・撤回 marker・索引行・
//! 参照されていない scan verdict・内容 scan cache を古い順に、1 回あたり上限つきで消す。全件の走査・GC はしない。

use anyhow::{Result, ensure};
use sqlx::PgPool;

/// 保持期間 T の下限。bucket reader が読む窓（今日と昨日、約 48 時間）と時計のずれの許容を越える。
pub const MIN_RETENTION_SECS: i64 = 3 * 86_400;

/// node の容量 B（行数）と保持期間 T（秒）。運用設定で必須。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionSettings {
    pub capacity_rows: i64,
    pub retention_secs: i64,
}

impl RetentionSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.capacity_rows > 0,
            "retention capacity must be positive"
        );
        ensure!(
            self.retention_secs >= MIN_RETENTION_SECS,
            "retention period must be at least 3 days"
        );
        Ok(())
    }
}

/// 起動時に運用設定を書く。
pub async fn configure_retention(pool: &PgPool, settings: RetentionSettings) -> Result<()> {
    settings.validate()?;
    sqlx::query(
        "UPDATE cn_index.retention_state SET capacity = $1, retention_secs = $2 WHERE id = TRUE",
    )
    .bind(settings.capacity_rows)
    .bind(settings.retention_secs)
    .execute(pool)
    .await?;
    Ok(())
}

/// 受入下限 W を進めて返す。容量を超えていれば、W 以上で最古の行の時刻の次まで上げる（各表の索引の先頭 1 件）。
pub async fn advance_retention_floor(pool: &PgPool, now: i64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "UPDATE cn_index.retention_state AS s SET floor = GREATEST(
             s.floor,
             $1 - s.retention_secs,
             CASE WHEN s.units > s.capacity THEN 1 + LEAST(
                 (SELECT MIN(created_at) FROM cn_index.index_entries WHERE created_at >= s.floor),
                 (SELECT MIN(created_at) FROM cn_index.known_post_withdrawals WHERE created_at >= s.floor),
                 (SELECT MIN(created_at) FROM cn_index.relation_actions WHERE created_at >= s.floor),
                 (SELECT EXTRACT(EPOCH FROM MIN(updated_at))::BIGINT FROM cn_safety.scan_verdicts
                  WHERE updated_at >= TO_TIMESTAMP(s.floor)),
                 (SELECT EXTRACT(EPOCH FROM MIN(completed_at))::BIGINT FROM cn_safety.content_scan_cache
                  WHERE completed_at >= TO_TIMESTAMP(s.floor))
             ) END
         )
         WHERE s.id = TRUE AND s.retention_secs IS NOT NULL
         RETURNING s.floor",
    )
    .bind(now)
    .fetch_optional(pool)
    .await?
    .unwrap_or(0))
}

/// W 未満の保存物を古い順に最大 `budget` 件消し、消した件数を返す。関係のアクションと撤回 marker を先に消し、
/// 索引行の削除 trigger が連鎖で消す行を少なくする。scan verdict は索引行が参照していないものだけを消す。
pub async fn reclaim_expired(pool: &PgPool, floor: i64, budget: usize) -> Result<usize> {
    let mut removed = 0usize;
    for statement in [
        "DELETE FROM cn_index.relation_actions WHERE (kind, source_id) IN (
             SELECT kind, source_id FROM cn_index.relation_actions
             WHERE created_at < $1 ORDER BY created_at LIMIT $2)",
        "DELETE FROM cn_index.known_post_withdrawals
         WHERE (scope_kind, scope_id, object_id) IN (
             SELECT scope_kind, scope_id, object_id FROM cn_index.known_post_withdrawals
             WHERE created_at < $1 ORDER BY created_at LIMIT $2)",
        "DELETE FROM cn_index.index_entries WHERE (scope_kind, scope_id, object_id) IN (
             SELECT scope_kind, scope_id, object_id FROM cn_index.index_entries
             WHERE created_at < $1 ORDER BY created_at LIMIT $2)",
        "DELETE FROM cn_safety.scan_verdicts WHERE id IN (
             SELECT v.id FROM cn_safety.scan_verdicts v
             WHERE v.updated_at < TO_TIMESTAMP($1)
               AND NOT EXISTS (SELECT 1 FROM cn_index.index_entries e WHERE e.verdict_id = v.id)
             ORDER BY v.updated_at LIMIT $2)",
        "DELETE FROM cn_safety.content_scan_cache WHERE cache_key IN (
             SELECT cache_key FROM cn_safety.content_scan_cache
             WHERE completed_at < TO_TIMESTAMP($1) ORDER BY completed_at LIMIT $2)",
    ] {
        let left = budget.saturating_sub(removed);
        if left == 0 {
            break;
        }
        removed += usize::try_from(
            sqlx::query(statement)
                .bind(floor)
                .bind(i64::try_from(left)?)
                .execute(pool)
                .await?
                .rows_affected(),
        )?;
    }
    Ok(removed)
}

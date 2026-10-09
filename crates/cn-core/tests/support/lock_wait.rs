//! 同時に動く取引の試験で、別の接続の取引が lock を待つまで待つ helper（#1699・#1702）。

use std::time::Duration;

use anyhow::{Result, bail};
use sqlx::PgPool;
use tokio::task::JoinHandle;

/// 別の接続の取引が lock を待つまで、または `task` が終わるまで待つ。lock を待ったら true。
pub async fn blocked_on_lock<T>(pool: &PgPool, task: &JoinHandle<T>) -> Result<bool> {
    for _ in 0..500 {
        if task.is_finished() {
            return Ok(false);
        }
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM pg_stat_activity
                WHERE datname = current_database() AND wait_event_type = 'Lock'
             )",
        )
        .fetch_one(pool)
        .await?;
        if waiting {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    bail!("the concurrent transaction neither finished nor waited on a lock")
}

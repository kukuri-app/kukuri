//! 索引の保守（#1221 R5-F / R5-H）。bucket の読取りと同じ巡回で、support・秘密鍵を失った scope の索引と、
//! 受入下限より古い保存物を上限つきで回収する。

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use sqlx::{Row, postgres::PgPool};
use tracing::warn;

use kukuri_cn_core::{
    ChannelSecretCipher, IndexEntryStore, IndexScopeKind, advance_retention_floor,
    get_channel_secret, is_topic_supported, reclaim_expired,
};

use crate::projection::IndexProjection;
use crate::state::IndexerRuntimeState;

/// 1 回の回収で消す保存物の上限（#1221 R5-F）。
const RECLAIM_BUDGET: usize = 128;
/// 1 回の巡回で繰り返す受入下限の回収の回数の上限。
const RECLAIM_STEPS_PER_PASS: usize = 16;
/// 1 回の巡回で照合する索引済み scope の上限。
const INDEXED_SCOPE_PAGE: usize = 32;

pub struct IndexMaintenance {
    pool: PgPool,
    entries: Arc<dyn IndexEntryStore>,
    projection: Arc<dyn IndexProjection>,
    cipher: ChannelSecretCipher,
    /// 退役させる旧 iroh store の directory(#1221 R5-I)。
    legacy_store: Option<std::path::PathBuf>,
}

impl IndexMaintenance {
    pub fn new(
        pool: PgPool,
        entries: Arc<dyn IndexEntryStore>,
        projection: Arc<dyn IndexProjection>,
        cipher: ChannelSecretCipher,
    ) -> Self {
        Self {
            pool,
            entries,
            projection,
            cipher,
            legacy_store: None,
        }
    }

    /// 巡回ごとに、退役させる旧 iroh store の file を 128 件まで消す(#1221 R5-I)。消した分は戻らないので、
    /// 再起動しても残った分から続ける。
    pub fn with_legacy_store(mut self, retiring: std::path::PathBuf) -> Self {
        self.legacy_store = Some(retiring);
        self
    }

    /// 1 回の巡回: 索引済みの scope を永続 cursor で最大 32 件照合し、support・秘密鍵を失ったものを合わせて
    /// 128 件まで回収する。続けて受入下限を進め、下限未満の保存物を回収する。
    pub async fn run_pass(&self, now: i64, state: &IndexerRuntimeState) {
        let mut budget = RECLAIM_BUDGET;
        match self.indexed_scope_page().await {
            Ok(indexed) => {
                for (kind, id) in indexed {
                    if budget == 0 {
                        break;
                    }
                    let retired = match self.is_scope_authorized(kind, &id).await {
                        Ok(true) => continue,
                        Ok(false) => self.retire_scope(kind, &id, budget).await,
                        Err(error) => Err(error),
                    };
                    match retired {
                        Ok(removed) => {
                            budget = budget.saturating_sub(removed);
                            state.record_deindexed(u64::from(removed > 0));
                        }
                        Err(error) => {
                            warn!(kind = kind.as_str(), scope_id = %id, error = %format!("{error:#}"),
                                "failed to retire an indexed scope; will revisit");
                            state.record_error(Some(id.as_str()), &format!("{error:#}"));
                        }
                    }
                }
            }
            Err(error) => {
                warn!(error = %format!("{error:#}"), "failed to page indexed scopes; will retry");
                state.record_error(None, &format!("{error:#}"));
            }
        }
        if let Err(error) = self
            .reclaim_retention_pass(now, RECLAIM_BUDGET, RECLAIM_STEPS_PER_PASS)
            .await
        {
            warn!(error = %format!("{error:#}"), "failed to reclaim expired index state; will retry");
            state.record_error(None, &format!("{error:#}"));
        }
        if let Some(retiring) = self.legacy_store.clone()
            && retiring.exists()
        {
            let removed = tokio::task::spawn_blocking(move || {
                kukuri_iroh_node::remove_dir_step(&retiring, RECLAIM_BUDGET)
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
            if let Err(error) = removed {
                warn!(error = %format!("{error:#}"), "failed to remove the retired iroh store; will retry");
                state.record_error(None, &format!("{error:#}"));
            }
        }
    }

    /// Seek at most 32 distinct truth-store scopes and retain the next position across restarts.
    async fn indexed_scope_page(&self) -> Result<Vec<(IndexScopeKind, String)>> {
        let cursor = sqlx::query(
            "SELECT last_kind, last_scope_id FROM cn_index.indexed_scope_cursor WHERE id = TRUE",
        )
        .fetch_one(&self.pool)
        .await?;
        let mut after_kind: String = cursor.try_get("last_kind")?;
        let mut after_id: String = cursor.try_get("last_scope_id")?;
        let mut scopes = Vec::new();
        let mut seen = HashSet::new();
        for pass in 0..2 {
            if pass == 1 {
                after_kind.clear();
                after_id.clear();
            }
            while scopes.len() < INDEXED_SCOPE_PAGE {
                let Some((kind, id)) = self
                    .entries
                    .next_scope_after(&after_kind, &after_id)
                    .await?
                else {
                    break;
                };
                if !seen.insert((kind, id.clone())) {
                    break;
                }
                after_kind = kind.as_str().to_string();
                after_id = id.clone();
                scopes.push((kind, id));
            }
        }
        if let Some((kind, id)) = scopes.last() {
            sqlx::query(
                "UPDATE cn_index.indexed_scope_cursor
                 SET last_kind = $1, last_scope_id = $2 WHERE id = TRUE",
            )
            .bind(kind.as_str())
            .bind(id)
            .execute(&self.pool)
            .await?;
        }
        Ok(scopes)
    }

    async fn is_scope_authorized(&self, kind: IndexScopeKind, id: &str) -> Result<bool> {
        if !is_topic_supported(&self.pool, kind, id).await? {
            return Ok(false);
        }
        if kind == IndexScopeKind::PrivateChannel {
            return Ok(get_channel_secret(&self.pool, &self.cipher, id)
                .await?
                .is_some());
        }
        Ok(true)
    }

    /// 解除した scope の索引を最大 `budget` 件回収して消した件数を返す（#1221 R5-F）。
    ///
    /// 投影 → 真実源の順で消す（真実源が先に空になると、残った投影を見つける入口が無くなる）。解除した scope は
    /// 検索の gate が表示から外すため、回収が済むまでの間も表示されない。撤回 marker は受入下限まで残す。
    pub async fn retire_scope(
        &self,
        kind: IndexScopeKind,
        id: &str,
        budget: usize,
    ) -> Result<usize> {
        let projected = self.projection.remove_scope_page(kind, id, budget).await?;
        let entries = self
            .entries
            .remove_scope_page(kind, id, budget.saturating_sub(projected))
            .await?;
        Ok(projected + entries)
    }

    /// 受入下限を進め、下限未満の保存物を最大 `budget` 件回収して消した件数を返す（#1221 R5-F）。
    pub async fn reclaim_retention(&self, now: i64, budget: usize) -> Result<usize> {
        let floor = advance_retention_floor(&self.pool, now, budget).await?;
        let projected = self.projection.remove_older_than(floor, budget).await?;
        Ok(
            projected
                + reclaim_expired(&self.pool, floor, budget.saturating_sub(projected)).await?,
        )
    }

    /// 1 回の巡回で行う回収。上限まで消せた間だけ最大 `steps` 回繰り返し、消した件数を返す。
    pub async fn reclaim_retention_pass(
        &self,
        now: i64,
        budget: usize,
        steps: usize,
    ) -> Result<usize> {
        let mut total = 0;
        for _ in 0..steps {
            let removed = self.reclaim_retention(now, budget).await?;
            total += removed;
            if removed < budget {
                break;
            }
        }
        Ok(total)
    }
}

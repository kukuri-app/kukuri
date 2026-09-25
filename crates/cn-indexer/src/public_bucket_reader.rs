//! Bounded public bucket reads alongside the legacy writer's existing CN sync path.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{StreamExt, stream};
use sqlx::{Row, postgres::PgPool};
use tracing::warn;

use kukuri_cn_core::{IndexEntryStore, IndexScopeKind};
use kukuri_docs_sync::{BucketReplica, BucketScope, DocsSync, IrohDocsSync, TimeBucket};

use crate::ingest::{IngestPipeline, IngestSummary, recent_object_keys};

const PRIORITY_TOPICS: usize = 8;
const FAIR_TOPICS: usize = 8;
const MAX_SCOPE_READS: usize = 8;

pub struct PublicBucketReader {
    pool: PgPool,
    docs: Arc<IrohDocsSync>,
    entries: Arc<dyn IndexEntryStore>,
    pipeline: IngestPipeline,
}

impl PublicBucketReader {
    pub fn new(
        pool: PgPool,
        docs: Arc<IrohDocsSync>,
        entries: Arc<dyn IndexEntryStore>,
        pipeline: IngestPipeline,
    ) -> Self {
        Self {
            pool,
            docs,
            entries,
            pipeline,
        }
    }

    async fn selected_topics(pool: &PgPool) -> Result<(Vec<(String, bool)>, String)> {
        let priority = sqlx::query(
            "SELECT id FROM cn_index.supported_topics
             WHERE kind = 'public_topic' AND last_index_demand_at >= NOW() - INTERVAL '10 minutes'
             ORDER BY last_index_demand_at DESC, id LIMIT $1",
        )
        .bind(PRIORITY_TOPICS as i64)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|row| row.try_get::<String, _>("id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
        let cursor: String = sqlx::query_scalar(
            "SELECT last_fair_topic FROM cn_index.public_bucket_reader_cursor WHERE id = TRUE",
        )
        .fetch_one(pool)
        .await?;
        let mut selected = priority
            .iter()
            .cloned()
            .map(|id| (id, true))
            .collect::<Vec<_>>();
        let mut seen = priority.into_iter().collect::<HashSet<_>>();
        let mut next_cursor = cursor.clone();
        for after in [cursor.as_str(), ""] {
            if selected.len() >= PRIORITY_TOPICS + FAIR_TOPICS {
                break;
            }
            let page = sqlx::query(
                "SELECT id FROM cn_index.supported_topics
                 WHERE kind = 'public_topic' AND id > $1 ORDER BY id LIMIT 32",
            )
            .bind(after)
            .fetch_all(pool)
            .await?;
            for row in page {
                let id: String = row.try_get("id")?;
                if !seen.insert(id.clone()) {
                    continue;
                }
                next_cursor.clone_from(&id);
                selected.push((id, false));
                if selected.len() >= PRIORITY_TOPICS + FAIR_TOPICS {
                    break;
                }
            }
        }
        Ok((selected, next_cursor))
    }

    pub async fn poll_once(&self, now: i64) -> Result<IngestSummary> {
        let (topics, next_cursor) = Self::selected_topics(&self.pool).await?;
        let buckets = TimeBucket::from_unix_seconds(now)?
            .live_window()
            .collect::<Vec<_>>();
        let mut scopes = Vec::with_capacity(topics.len() * buckets.len());
        for (topic, priority) in topics {
            for bucket in &buckets {
                scopes.push((topic.clone(), *bucket, priority));
            }
        }
        let mut jobs = stream::iter(scopes.into_iter().map(
            |(topic, bucket, priority)| async move {
                let replica = BucketReplica::new(
                    BucketScope::Topic {
                        topic_id: topic.clone(),
                    },
                    bucket,
                )?
                .replica_id();
                self.read_scope(&topic, &replica, if priority { 20 } else { 4 })
                    .await
            },
        ))
        .buffer_unordered(MAX_SCOPE_READS);
        let mut total = IngestSummary::default();
        while let Some(result) = jobs.next().await {
            match result {
                Ok(summary) => total.merge(summary),
                Err(error) => warn!(%error, "failed to read one public bucket scope"),
            }
        }
        sqlx::query(
            "UPDATE cn_index.public_bucket_reader_cursor SET last_fair_topic = $1 WHERE id = TRUE",
        )
        .bind(next_cursor)
        .execute(&self.pool)
        .await?;
        Ok(total)
    }

    async fn read_scope(
        &self,
        topic: &str,
        replica: &kukuri_core::ReplicaId,
        limit: usize,
    ) -> Result<IngestSummary> {
        if !self
            .entries
            .is_scope_supported(IndexScopeKind::PublicTopic, topic)
            .await?
        {
            return Ok(IngestSummary::default());
        }
        let peers = self.docs.remote_read_candidates().await;
        if peers.is_empty() {
            return Ok(IngestSummary::default());
        }
        let per_peer = limit / peers.len();
        let mut total = IngestSummary::default();
        for peer in peers {
            let source = self.docs.remote_source(peer.clone());
            let keys = match recent_object_keys(&source, replica, per_peer).await {
                Ok(keys) => keys,
                Err(error) => {
                    warn!(replica = %replica.as_str(), peer = %peer.id, %error,
                        "public bucket provider did not return a page");
                    continue;
                }
            };
            for key in keys {
                let source: Arc<dyn DocsSync> = Arc::new(self.docs.remote_source(peer.clone()));
                let summary = self
                    .pipeline
                    .clone()
                    .with_docs_source(source)
                    .ingest_changed_keys(IndexScopeKind::PublicTopic, topic, replica, &[key])
                    .await?;
                total.merge(summary);
            }
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kukuri_cn_core::{
        TestDatabase, add_supported_topic, connect_postgres, initialize_database, mark_index_demand,
    };

    #[tokio::test]
    async fn recent_demand_wins_while_the_fair_cursor_reaches_every_other_topic() -> Result<()> {
        // config::tests rewrites COMMUNITY_NODE_DATABASE_URL in this process under this lock.
        let admin = {
            let _env = crate::config::tests::env_lock();
            kukuri_test_support::gated_env_url(
                "KUKURI_CN_RUN_INTEGRATION_TESTS",
                "COMMUNITY_NODE_DATABASE_URL",
                "postgres://cn:cn_password@127.0.0.1:15432/cn",
            )
        };
        let Some(admin) = admin else {
            return Ok(());
        };
        let database = TestDatabase::create(&admin, "cn_bucket_fair_window").await?;
        let pool = connect_postgres(&database.database_url).await?;
        initialize_database(&pool).await?;
        for index in 0..40 {
            add_supported_topic(
                &pool,
                IndexScopeKind::PublicTopic,
                &format!("topic-{index:02}"),
            )
            .await?;
        }
        mark_index_demand(&pool, IndexScopeKind::PublicTopic, "topic-39").await?;
        let mut fair = HashSet::new();
        for _ in 0..3 {
            let (selected, cursor) = PublicBucketReader::selected_topics(&pool).await?;
            assert!(selected.len() <= PRIORITY_TOPICS + FAIR_TOPICS);
            assert_eq!(selected[0], ("topic-39".into(), true));
            fair.extend(
                selected
                    .into_iter()
                    .filter_map(|(id, priority)| (!priority).then_some(id)),
            );
            sqlx::query(
                "UPDATE cn_index.public_bucket_reader_cursor SET last_fair_topic = $1 WHERE id = TRUE",
            )
            .bind(cursor)
            .execute(&pool)
            .await?;
        }
        assert_eq!(
            fair.len(),
            39,
            "every non-priority topic must get a finite turn"
        );
        pool.close().await;
        database.cleanup().await?;
        Ok(())
    }
}

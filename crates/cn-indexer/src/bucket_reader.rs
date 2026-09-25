//! One bounded public/private bucket reader alongside the legacy CN sync path.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{StreamExt, stream};
use sqlx::{Row, postgres::PgPool};
use tokio::sync::Semaphore;
use tracing::warn;

use kukuri_cn_core::{ChannelSecretCipher, IndexEntryStore, IndexScopeKind, get_channel_secret};
use kukuri_docs_sync::{BucketReplica, BucketScope, DocsSync, IrohDocsSync, TimeBucket};
use kukuri_transport::SeedPeer;

use crate::ingest::{IngestPipeline, IngestSummary, recent_object_keys};

const PRIORITY_SCOPES: usize = 8;
const MAX_SCOPE_READS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectedScope {
    kind: IndexScopeKind,
    id: String,
    epoch_id: Option<String>,
    priority: bool,
}

pub struct BucketReader {
    pool: PgPool,
    docs: Arc<IrohDocsSync>,
    entries: Arc<dyn IndexEntryStore>,
    pipeline: IngestPipeline,
    cipher: ChannelSecretCipher,
    physical_budget: usize,
    scope_permits: Arc<Semaphore>,
}

impl BucketReader {
    pub fn new(
        pool: PgPool,
        docs: Arc<IrohDocsSync>,
        entries: Arc<dyn IndexEntryStore>,
        pipeline: IngestPipeline,
        cipher: ChannelSecretCipher,
        physical_budget: usize,
    ) -> Self {
        assert!(physical_budget == 32 || physical_budget == 64);
        Self {
            pool,
            docs,
            entries,
            pipeline,
            cipher,
            physical_budget,
            scope_permits: Arc::new(Semaphore::new(MAX_SCOPE_READS)),
        }
    }

    pub(crate) fn scope_permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.scope_permits)
    }

    async fn selected_scopes(
        pool: &PgPool,
        physical_budget: usize,
    ) -> Result<(Vec<SelectedScope>, (String, String))> {
        let eligible = "FROM cn_index.supported_topics s
            LEFT JOIN cn_index.channel_secrets c
              ON s.kind = 'private_channel' AND c.channel_id = s.id
            WHERE (s.kind = 'public_topic' OR (s.kind = 'private_channel' AND c.epoch_id IS NOT NULL))";
        let priority = sqlx::query(&format!(
            "SELECT s.kind, s.id, c.epoch_id {eligible}
             AND s.last_index_demand_at >= NOW() - INTERVAL '10 minutes'
             ORDER BY s.last_index_demand_at DESC, s.kind, s.id LIMIT $1"
        ))
        .bind(PRIORITY_SCOPES as i64)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|row| -> Result<SelectedScope> {
            Ok(SelectedScope {
                kind: IndexScopeKind::parse(&row.try_get::<String, _>("kind")?)?,
                id: row.try_get("id")?,
                epoch_id: row.try_get("epoch_id")?,
                priority: true,
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
        let cursor = sqlx::query(
            "SELECT last_kind, last_scope_id FROM cn_index.bucket_reader_cursor WHERE id = TRUE",
        )
        .fetch_one(pool)
        .await?;
        let mut selected = priority;
        let mut seen = selected
            .iter()
            .map(|scope| (scope.kind, scope.id.clone()))
            .collect::<HashSet<_>>();
        let mut next_cursor: (String, String) = (
            cursor.try_get("last_kind")?,
            cursor.try_get("last_scope_id")?,
        );
        for (after_kind, after_id) in [next_cursor.clone(), (String::new(), String::new())] {
            if selected.len() >= physical_budget / 2 {
                break;
            }
            let page = sqlx::query(&format!(
                "SELECT s.kind, s.id, c.epoch_id {eligible}
                 AND (s.kind, s.id) > ($1, $2)
                 ORDER BY s.kind, s.id LIMIT 64"
            ))
            .bind(after_kind)
            .bind(after_id)
            .fetch_all(pool)
            .await?;
            for row in page {
                let kind = IndexScopeKind::parse(&row.try_get::<String, _>("kind")?)?;
                let id: String = row.try_get("id")?;
                next_cursor = (kind.as_str().to_string(), id.clone());
                if !seen.insert((kind, id.clone())) {
                    continue;
                }
                selected.push(SelectedScope {
                    kind,
                    id,
                    epoch_id: row.try_get("epoch_id")?,
                    priority: false,
                });
                if selected.len() >= physical_budget / 2 {
                    break;
                }
            }
        }
        Ok((selected, next_cursor))
    }

    async fn selected_public_providers(pool: &PgPool) -> Result<(Vec<SeedPeer>, (String, String))> {
        let eligible = "FROM cn_bootstrap.peer_registrations p
            JOIN cn_user.subscriber_accounts a
              ON a.subscriber_pubkey = p.subscriber_pubkey
            WHERE a.status = 'active' AND p.expires_at > NOW()";
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for row in sqlx::query(&format!(
            "SELECT p.subscriber_pubkey, p.endpoint_id, p.addr_hint {eligible}
             ORDER BY p.last_seen_at DESC, p.subscriber_pubkey, p.endpoint_id LIMIT 2"
        ))
        .fetch_all(pool)
        .await?
        {
            let endpoint_id: String = row.try_get("endpoint_id")?;
            seen.insert(endpoint_id.clone());
            selected.push(SeedPeer {
                endpoint_id,
                addr_hint: row.try_get("addr_hint")?,
            });
        }
        let cursor = sqlx::query(
            "SELECT last_peer_pubkey, last_peer_endpoint_id
             FROM cn_index.bucket_reader_cursor WHERE id = TRUE",
        )
        .fetch_one(pool)
        .await?;
        let mut next_cursor: (String, String) = (
            cursor.try_get("last_peer_pubkey")?,
            cursor.try_get("last_peer_endpoint_id")?,
        );
        for (after_pubkey, after_id) in [next_cursor.clone(), (String::new(), String::new())] {
            if selected.len() >= 4 {
                break;
            }
            let page = sqlx::query(&format!(
                "SELECT p.subscriber_pubkey, p.endpoint_id, p.addr_hint {eligible}
                 AND (p.subscriber_pubkey, p.endpoint_id) > ($1, $2)
                 ORDER BY p.subscriber_pubkey, p.endpoint_id LIMIT 8"
            ))
            .bind(after_pubkey)
            .bind(after_id)
            .fetch_all(pool)
            .await?;
            for row in page {
                let pubkey: String = row.try_get("subscriber_pubkey")?;
                let endpoint_id: String = row.try_get("endpoint_id")?;
                next_cursor = (pubkey, endpoint_id.clone());
                if !seen.insert(endpoint_id.clone()) {
                    continue;
                }
                selected.push(SeedPeer {
                    endpoint_id,
                    addr_hint: row.try_get("addr_hint")?,
                });
                if selected.len() >= 4 {
                    break;
                }
            }
        }
        Ok((selected, next_cursor))
    }

    pub async fn poll_once(&self, now: i64) -> Result<IngestSummary> {
        let (selected, next_cursor) =
            Self::selected_scopes(&self.pool, self.physical_budget).await?;
        let (public_seeds, next_peer_cursor) = Self::selected_public_providers(&self.pool).await?;
        let buckets = TimeBucket::from_unix_seconds(now)?
            .live_window()
            .collect::<Vec<_>>();
        let mut scopes = Vec::with_capacity(selected.len() * buckets.len());
        for scope in selected {
            for bucket in &buckets {
                scopes.push((scope.clone(), *bucket));
            }
        }
        let public_seeds = &public_seeds;
        let mut jobs = stream::iter(scopes.into_iter().map(|(scope, bucket)| async move {
            let replica = BucketReplica::new(
                match &scope.epoch_id {
                    Some(epoch_id) => BucketScope::PrivateChannel {
                        channel_id: scope.id.clone(),
                        epoch_id: epoch_id.clone(),
                    },
                    None => BucketScope::Topic {
                        topic_id: scope.id.clone(),
                    },
                },
                bucket,
            )?;
            self.read_scope(
                &scope,
                &replica,
                if scope.priority { 20 } else { 4 },
                public_seeds,
            )
            .await
        }))
        .buffer_unordered(MAX_SCOPE_READS);
        let mut total = IngestSummary::default();
        while let Some(result) = jobs.next().await {
            match result {
                Ok(summary) => total.merge(summary),
                Err(error) => warn!(%error, "failed to read one bucket scope"),
            }
        }
        sqlx::query(
            "UPDATE cn_index.bucket_reader_cursor
             SET last_kind = $1, last_scope_id = $2,
                 last_peer_pubkey = $3, last_peer_endpoint_id = $4 WHERE id = TRUE",
        )
        .bind(next_cursor.0)
        .bind(next_cursor.1)
        .bind(next_peer_cursor.0)
        .bind(next_peer_cursor.1)
        .execute(&self.pool)
        .await?;
        Ok(total)
    }

    async fn read_scope(
        &self,
        scope: &SelectedScope,
        replica: &BucketReplica,
        limit: usize,
        public_seeds: &[SeedPeer],
    ) -> Result<IngestSummary> {
        if !self
            .entries
            .is_scope_supported(scope.kind, &scope.id)
            .await?
        {
            return Ok(IngestSummary::default());
        }
        let private_secret = if scope.epoch_id.is_some() {
            let Some(secret) = get_channel_secret(&self.pool, &self.cipher, &scope.id).await?
            else {
                return Ok(IngestSummary::default());
            };
            if secret.epoch_id != scope.epoch_id {
                return Ok(IngestSummary::default());
            }
            let mut source = [0u8; 32];
            hex::decode_to_slice(secret.namespace_secret_hex, &mut source)?;
            Some(replica.derive_private_secret(&source)?)
        } else {
            None
        };
        let peers = if private_secret.is_some() {
            let rows = sqlx::query(
                "SELECT p.endpoint_id, p.addr_hint FROM cn_index.indexing_requests r
                 JOIN cn_bootstrap.peer_registrations p
                   ON p.subscriber_pubkey = r.requester_pubkey
                 JOIN cn_user.subscriber_accounts a
                   ON a.subscriber_pubkey = p.subscriber_pubkey
                 WHERE r.kind = 'private_channel' AND r.target_id = $1
                   AND r.status = 'approved' AND a.status = 'active'
                   AND p.expires_at > NOW()
                 ORDER BY p.last_seen_at DESC, p.endpoint_id LIMIT 4",
            )
            .bind(&scope.id)
            .fetch_all(&self.pool)
            .await?;
            let seeds = rows
                .into_iter()
                .map(|row| -> Result<SeedPeer> {
                    Ok(SeedPeer {
                        endpoint_id: row.try_get("endpoint_id")?,
                        addr_hint: row.try_get("addr_hint")?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            self.docs.remote_read_candidates_for_seeds(seeds).await?
        } else {
            self.docs
                .remote_read_candidates_for_seeds(public_seeds.to_vec())
                .await?
        };
        if peers.is_empty() {
            return Ok(IngestSummary::default());
        }
        let _permit = self.scope_permits.acquire().await?;
        let per_peer = limit / peers.len();
        let mut total = IngestSummary::default();
        for peer in peers {
            let replica_id = replica.replica_id();
            let source = match private_secret {
                Some(secret) => self.docs.remote_source_with_private_secret(
                    peer.clone(),
                    replica_id.clone(),
                    secret,
                ),
                None => self.docs.remote_source(peer.clone()),
            };
            let keys = match recent_object_keys(&source, &replica_id, per_peer).await {
                Ok(keys) => keys,
                Err(error) => {
                    warn!(replica = %replica_id.as_str(), peer = %peer.id, %error,
                        "bucket provider did not return a page");
                    continue;
                }
            };
            for key in keys {
                if !self.private_epoch_is_current(scope).await? {
                    return Ok(total);
                }
                let source: Arc<dyn DocsSync> = Arc::new(match private_secret {
                    Some(secret) => self.docs.remote_source_with_private_secret(
                        peer.clone(),
                        replica_id.clone(),
                        secret,
                    ),
                    None => self.docs.remote_source(peer.clone()),
                });
                let summary = self
                    .pipeline
                    .clone()
                    .with_docs_source(source)
                    .ingest_changed_keys(scope.kind, &scope.id, &replica_id, &[key])
                    .await?;
                total.merge(summary);
            }
        }
        Ok(total)
    }

    async fn private_epoch_is_current(&self, scope: &SelectedScope) -> Result<bool> {
        let Some(epoch_id) = scope.epoch_id.as_deref() else {
            return Ok(true);
        };
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT epoch_id FROM cn_index.channel_secrets WHERE channel_id = $1",
        )
        .bind(&scope.id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        Ok(stored.as_deref() == Some(epoch_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kukuri_cn_core::{
        TestDatabase, add_supported_topic, connect_postgres, initialize_database,
        mark_index_demand, register_channel_secret_with_epoch, remove_channel_secret,
        remove_supported_topic,
    };

    #[tokio::test]
    async fn recent_demand_wins_while_the_fair_cursor_reaches_every_other_topic() -> Result<()> {
        let Some(admin) = kukuri_test_support::gated_env_url(
            "KUKURI_CN_RUN_INTEGRATION_TESTS",
            "COMMUNITY_NODE_DATABASE_URL",
            "postgres://cn:cn_password@127.0.0.1:15432/cn",
        ) else {
            return Ok(());
        };
        for total in [80, 800] {
            let database = TestDatabase::create(&admin, &format!("cn_bucket_fair_{total}")).await?;
            let pool = connect_postgres(&database.database_url).await?;
            initialize_database(&pool).await?;
            sqlx::query(
                "INSERT INTO cn_index.supported_topics(kind, id)
                 SELECT 'public_topic', 'topic-' || lpad(i::text, 4, '0')
                 FROM generate_series(0, $1::INTEGER - 1) AS i",
            )
            .bind(total)
            .execute(&pool)
            .await?;
            let hot = format!("topic-{:04}", total - 1);
            mark_index_demand(&pool, IndexScopeKind::PublicTopic, &hot).await?;
            let (expanded, _) = BucketReader::selected_scopes(&pool, 64).await?;
            assert_eq!(
                expanded.len(),
                32,
                "the post-transition reader uses the same 64-slot selector"
            );
            let mut fair = HashSet::new();
            for _ in 0..(total / 15 + 2) {
                let (selected, cursor) = BucketReader::selected_scopes(&pool, 32).await?;
                assert!(selected.len() <= 16);
                assert_eq!(selected[0].id, hot);
                assert!(selected[0].priority);
                fair.extend(
                    selected
                        .into_iter()
                        .filter_map(|scope| (!scope.priority).then_some(scope.id)),
                );
                sqlx::query(
                    "UPDATE cn_index.bucket_reader_cursor
                     SET last_kind = $1, last_scope_id = $2 WHERE id = TRUE",
                )
                .bind(cursor.0)
                .bind(cursor.1)
                .execute(&pool)
                .await?;
                if fair.len() == (total - 1) as usize {
                    break;
                }
            }
            assert_eq!(fair.len(), (total - 1) as usize);
            pool.close().await;
            database.cleanup().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn private_epoch_requires_both_support_and_registered_secret() -> Result<()> {
        let Some(admin) = kukuri_test_support::gated_env_url(
            "KUKURI_CN_RUN_INTEGRATION_TESTS",
            "COMMUNITY_NODE_DATABASE_URL",
            "postgres://cn:cn_password@127.0.0.1:15432/cn",
        ) else {
            return Ok(());
        };
        let database = TestDatabase::create(&admin, "cn_bucket_private_gate").await?;
        let pool = connect_postgres(&database.database_url).await?;
        initialize_database(&pool).await?;
        let cipher =
            ChannelSecretCipher::from_key_material("cn-bucket-private-test-key-0123456789")?;
        let secret = hex::encode([7; 32]);
        add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "private-room").await?;
        assert!(BucketReader::selected_scopes(&pool, 32).await?.0.is_empty());
        register_channel_secret_with_epoch(&pool, &cipher, "private-room", "epoch-1", &secret)
            .await?;
        let (selected, _) = BucketReader::selected_scopes(&pool, 32).await?;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].epoch_id.as_deref(), Some("epoch-1"));
        remove_channel_secret(&pool, "private-room").await?;
        assert!(BucketReader::selected_scopes(&pool, 32).await?.0.is_empty());
        register_channel_secret_with_epoch(&pool, &cipher, "private-room", "epoch-1", &secret)
            .await?;
        remove_supported_topic(&pool, IndexScopeKind::PrivateChannel, "private-room").await?;
        assert!(BucketReader::selected_scopes(&pool, 32).await?.0.is_empty());
        pool.close().await;
        database.cleanup().await
    }

    #[tokio::test]
    async fn public_provider_window_keeps_recent_peers_and_rotates_the_rest() -> Result<()> {
        let Some(admin) = kukuri_test_support::gated_env_url(
            "KUKURI_CN_RUN_INTEGRATION_TESTS",
            "COMMUNITY_NODE_DATABASE_URL",
            "postgres://cn:cn_password@127.0.0.1:15432/cn",
        ) else {
            return Ok(());
        };
        let database = TestDatabase::create(&admin, "cn_bucket_peer_fair").await?;
        let pool = connect_postgres(&database.database_url).await?;
        initialize_database(&pool).await?;
        sqlx::query(
            "INSERT INTO cn_user.subscriber_accounts(subscriber_pubkey)
             SELECT 'subscriber-' || i FROM generate_series(0, 11) AS i",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO cn_bootstrap.peer_registrations
               (subscriber_pubkey, endpoint_id, last_seen_at, expires_at)
             SELECT 'subscriber-' || i, 'peer-' || i,
                    NOW() - (12 - i) * INTERVAL '1 second', NOW() + INTERVAL '90 seconds'
             FROM generate_series(0, 11) AS i",
        )
        .execute(&pool)
        .await?;
        let mut fair = HashSet::new();
        for _ in 0..5 {
            let (selected, cursor) = BucketReader::selected_public_providers(&pool).await?;
            assert_eq!(selected.len(), 4);
            assert_eq!(selected[0].endpoint_id, "peer-11");
            assert_eq!(selected[1].endpoint_id, "peer-10");
            fair.extend(selected.into_iter().skip(2).map(|peer| peer.endpoint_id));
            sqlx::query(
                "UPDATE cn_index.bucket_reader_cursor
                 SET last_peer_pubkey = $1, last_peer_endpoint_id = $2 WHERE id = TRUE",
            )
            .bind(cursor.0)
            .bind(cursor.1)
            .execute(&pool)
            .await?;
        }
        assert_eq!(fair.len(), 10);
        pool.close().await;
        database.cleanup().await
    }
}

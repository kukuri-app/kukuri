//! One bounded public/private bucket reader alongside the legacy CN sync path.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{StreamExt, stream};
use sqlx::{
    Row,
    postgres::{PgPool, PgRow},
};
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
    fn selected_scope_row(row: PgRow, priority: bool) -> Result<Option<SelectedScope>> {
        let kind = IndexScopeKind::parse(&row.try_get::<String, _>("kind")?)?;
        let epoch_id: Option<String> = row.try_get("epoch_id")?;
        if kind == IndexScopeKind::PrivateChannel && epoch_id.is_none() {
            return Ok(None);
        }
        Ok(Some(SelectedScope {
            kind,
            id: row.try_get("id")?,
            epoch_id,
            priority,
        }))
    }

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
        let priority_rows = sqlx::query(
            "WITH candidates AS MATERIALIZED (
                SELECT kind, id, last_index_demand_at FROM cn_index.supported_topics
                WHERE last_index_demand_at >= NOW() - INTERVAL '10 minutes'
                ORDER BY last_index_demand_at DESC, kind, id LIMIT $1
             )
             SELECT s.kind, s.id, c.epoch_id FROM candidates s
             LEFT JOIN LATERAL (
               SELECT epoch_id FROM cn_index.channel_secrets
               WHERE s.kind='private_channel' AND channel_id=s.id LIMIT 1
             ) c ON TRUE
             ORDER BY s.last_index_demand_at DESC, s.kind, s.id",
        )
        .bind((PRIORITY_SCOPES * 2) as i64)
        .fetch_all(pool)
        .await?;
        let mut priority = Vec::new();
        for row in priority_rows {
            if let Some(scope) = Self::selected_scope_row(row, true)? {
                priority.push(scope);
                if priority.len() == PRIORITY_SCOPES {
                    break;
                }
            }
        }
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
        let page_sql = "WITH candidates AS MATERIALIZED (
                SELECT kind, id FROM cn_index.supported_topics
                WHERE (kind, id) > ($1, $2)
                ORDER BY kind, id LIMIT 64
             )
             SELECT s.kind, s.id, c.epoch_id FROM candidates s
             LEFT JOIN LATERAL (
               SELECT epoch_id FROM cn_index.channel_secrets
               WHERE s.kind='private_channel' AND channel_id=s.id LIMIT 1
             ) c ON TRUE
             ORDER BY s.kind, s.id";
        let mut page = sqlx::query(page_sql)
            .bind(&next_cursor.0)
            .bind(&next_cursor.1)
            .fetch_all(pool)
            .await?;
        if page.is_empty() {
            page = sqlx::query(page_sql)
                .bind("")
                .bind("")
                .fetch_all(pool)
                .await?;
        }
        for row in page {
            let kind: String = row.try_get("kind")?;
            let id: String = row.try_get("id")?;
            next_cursor = (kind, id);
            if let Some(scope) = Self::selected_scope_row(row, false)?
                && seen.insert((scope.kind, scope.id.clone()))
            {
                selected.push(scope);
                if selected.len() >= physical_budget / 2 {
                    break;
                }
            }
        }
        Ok((selected, next_cursor))
    }

    async fn selected_public_providers(pool: &PgPool) -> Result<(Vec<SeedPeer>, (String, String))> {
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for row in sqlx::query(
            "WITH candidates AS MATERIALIZED (
                SELECT subscriber_pubkey, endpoint_id, addr_hint, expires_at, last_seen_at
                FROM cn_bootstrap.peer_registrations ORDER BY last_seen_at DESC LIMIT 8
             )
             SELECT p.endpoint_id, p.addr_hint,
                    (a.status='active' AND p.expires_at>NOW()) AS eligible
             FROM candidates p LEFT JOIN LATERAL (
               SELECT status FROM cn_user.subscriber_accounts
               WHERE subscriber_pubkey=p.subscriber_pubkey LIMIT 1
             ) a ON TRUE
             ORDER BY p.last_seen_at DESC",
        )
        .fetch_all(pool)
        .await?
        {
            if !row.try_get::<Option<bool>, _>("eligible")?.unwrap_or(false) {
                continue;
            }
            let endpoint_id: String = row.try_get("endpoint_id")?;
            seen.insert(endpoint_id.clone());
            selected.push(SeedPeer {
                endpoint_id,
                addr_hint: row.try_get("addr_hint")?,
            });
            if selected.len() == 2 {
                break;
            }
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
        let page_sql = "WITH candidates AS MATERIALIZED (
                SELECT subscriber_pubkey, endpoint_id, addr_hint, expires_at
                FROM cn_bootstrap.peer_registrations
                WHERE (subscriber_pubkey, endpoint_id)>($1, $2)
                ORDER BY subscriber_pubkey, endpoint_id LIMIT 8
             )
             SELECT p.subscriber_pubkey, p.endpoint_id, p.addr_hint,
                    (a.status='active' AND p.expires_at>NOW()) AS eligible
             FROM candidates p LEFT JOIN LATERAL (
               SELECT status FROM cn_user.subscriber_accounts
               WHERE subscriber_pubkey=p.subscriber_pubkey LIMIT 1
             ) a ON TRUE
             ORDER BY p.subscriber_pubkey, p.endpoint_id";
        let mut page = sqlx::query(page_sql)
            .bind(&next_cursor.0)
            .bind(&next_cursor.1)
            .fetch_all(pool)
            .await?;
        if page.is_empty() {
            page = sqlx::query(page_sql)
                .bind("")
                .bind("")
                .fetch_all(pool)
                .await?;
        }
        for row in page {
            let pubkey: String = row.try_get("subscriber_pubkey")?;
            let endpoint_id: String = row.try_get("endpoint_id")?;
            next_cursor = (pubkey, endpoint_id.clone());
            if !row.try_get::<Option<bool>, _>("eligible")?.unwrap_or(false)
                || !seen.insert(endpoint_id.clone())
            {
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
        Ok((selected, next_cursor))
    }

    async fn selected_private_providers(pool: &PgPool, channel_id: &str) -> Result<Vec<SeedPeer>> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO cn_index.private_bucket_provider_cursor(channel_id)
             VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;
        let cursor = sqlx::query(
            "SELECT last_pubkey, last_endpoint_id
             FROM cn_index.private_bucket_provider_cursor WHERE channel_id=$1 FOR UPDATE",
        )
        .bind(channel_id)
        .fetch_one(&mut *tx)
        .await?;
        let old: (String, String) = (
            cursor.try_get("last_pubkey")?,
            cursor.try_get("last_endpoint_id")?,
        );
        let mut next = old.clone();
        let mut seeds = Vec::new();
        let mut seen = HashSet::new();
        // Continue the current requester's endpoint page before moving to the
        // next approved requester. A requester with many devices cannot hide
        // its later endpoints behind the first four.
        let endpoint_page = sqlx::query(
            "WITH candidates AS MATERIALIZED (
                SELECT endpoint_id, addr_hint, expires_at
                FROM cn_bootstrap.peer_registrations
                WHERE subscriber_pubkey=$2 AND endpoint_id>$3
                ORDER BY endpoint_id LIMIT 8
             )
             SELECT p.endpoint_id, p.addr_hint,
                    (a.status='active' AND r.id IS NOT NULL AND p.expires_at>NOW()) AS eligible
             FROM candidates p
             LEFT JOIN LATERAL (
               SELECT status FROM cn_user.subscriber_accounts
               WHERE subscriber_pubkey=$2 LIMIT 1
             ) a ON TRUE
             LEFT JOIN LATERAL (
               SELECT id FROM cn_index.indexing_requests
               WHERE kind='private_channel' AND target_id=$1
                 AND requester_pubkey=$2 AND status='approved' LIMIT 1
             ) r ON TRUE ORDER BY p.endpoint_id",
        )
        .bind(channel_id)
        .bind(&old.0)
        .bind(&old.1)
        .fetch_all(&mut *tx)
        .await?;
        let endpoint_page_full = endpoint_page.len() == 8;
        for row in endpoint_page {
            next = (old.0.clone(), row.try_get("endpoint_id")?);
            if row.try_get::<Option<bool>, _>("eligible")?.unwrap_or(false)
                && seen.insert(next.1.clone())
            {
                seeds.push(SeedPeer {
                    endpoint_id: next.1.clone(),
                    addr_hint: row.try_get("addr_hint")?,
                });
                if seeds.len() == 4 {
                    break;
                }
            }
        }
        if !endpoint_page_full && seeds.len() < 4 {
            // The request index is the mother set, not every CN peer. Each
            // selected requester gets one indexed endpoint lookup; absent or
            // expired endpoints still advance the requester cursor.
            let page_sql = "WITH requesters AS MATERIALIZED (
                    SELECT requester_pubkey FROM cn_index.indexing_requests
                    WHERE kind='private_channel' AND target_id=$1
                      AND status='approved' AND requester_pubkey>$2
                    ORDER BY requester_pubkey LIMIT 64
                 )
                 SELECT r.requester_pubkey, p.endpoint_id, p.addr_hint,
                        (a.status='active' AND p.expires_at>NOW()) AS eligible
                 FROM requesters r
                 LEFT JOIN LATERAL (
                   SELECT endpoint_id, addr_hint, expires_at
                   FROM cn_bootstrap.peer_registrations
                   WHERE subscriber_pubkey=r.requester_pubkey
                   ORDER BY endpoint_id LIMIT 1
                 ) p ON TRUE
                 LEFT JOIN LATERAL (
                   SELECT status FROM cn_user.subscriber_accounts
                   WHERE subscriber_pubkey=r.requester_pubkey LIMIT 1
                 ) a ON TRUE
                 ORDER BY r.requester_pubkey";
            let mut page = sqlx::query(page_sql)
                .bind(channel_id)
                .bind(&old.0)
                .fetch_all(&mut *tx)
                .await?;
            if page.is_empty() {
                page = sqlx::query(page_sql)
                    .bind(channel_id)
                    .bind("")
                    .fetch_all(&mut *tx)
                    .await?;
            }
            for row in page {
                let pubkey: String = row.try_get("requester_pubkey")?;
                let endpoint_id: Option<String> = row.try_get("endpoint_id")?;
                next = (pubkey, endpoint_id.clone().unwrap_or_default());
                if let Some(endpoint_id) = endpoint_id
                    && row.try_get::<Option<bool>, _>("eligible")?.unwrap_or(false)
                    && seen.insert(endpoint_id.clone())
                {
                    seeds.push(SeedPeer {
                        endpoint_id,
                        addr_hint: row.try_get("addr_hint")?,
                    });
                    if seeds.len() == 4 {
                        break;
                    }
                }
            }
        }
        sqlx::query(
            "UPDATE cn_index.private_bucket_provider_cursor
             SET last_pubkey=$2, last_endpoint_id=$3 WHERE channel_id=$1",
        )
        .bind(channel_id)
        .bind(&next.0)
        .bind(&next.1)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(seeds)
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
            let seeds = Self::selected_private_providers(&self.pool, &scope.id).await?;
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

    fn integration_admin_url() -> Option<String> {
        if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_INTEGRATION_TESTS") {
            return None;
        }
        // Config unit tests temporarily replace COMMUNITY_NODE_DATABASE_URL in
        // this same test binary. The compose port is stable across that test.
        let port = std::env::var("CN_POSTGRES_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port > 0)
            .unwrap_or(15432);
        Some(format!("postgres://cn:cn_password@127.0.0.1:{port}/cn"))
    }

    #[tokio::test]
    async fn recent_demand_wins_while_the_fair_cursor_reaches_every_other_topic() -> Result<()> {
        let Some(admin) = integration_admin_url() else {
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
    async fn unopened_private_scopes_do_not_block_a_later_public_scope() -> Result<()> {
        let Some(admin) = integration_admin_url() else {
            return Ok(());
        };
        let database = TestDatabase::create(&admin, "cn_bucket_unopened_private").await?;
        let pool = connect_postgres(&database.database_url).await?;
        initialize_database(&pool).await?;
        sqlx::query(
            "INSERT INTO cn_index.supported_topics(kind, id)
             SELECT 'private_channel', 'room-' || lpad(i::text, 4, '0')
             FROM generate_series(0, 799) AS i",
        )
        .execute(&pool)
        .await?;
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, "later-public").await?;
        let mut reached = false;
        for _ in 0..14 {
            let (scopes, cursor) = BucketReader::selected_scopes(&pool, 32).await?;
            reached |= scopes.iter().any(|scope| scope.id == "later-public");
            sqlx::query(
                "UPDATE cn_index.bucket_reader_cursor
                 SET last_kind=$1, last_scope_id=$2 WHERE id=TRUE",
            )
            .bind(cursor.0)
            .bind(cursor.1)
            .execute(&pool)
            .await?;
            if reached {
                break;
            }
        }
        assert!(reached, "the fair cursor must pass unopened private scopes");
        pool.close().await;
        database.cleanup().await
    }

    #[tokio::test]
    async fn private_epoch_requires_both_support_and_registered_secret() -> Result<()> {
        let Some(admin) = integration_admin_url() else {
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
    async fn private_provider_window_reaches_the_fifth_approved_device() -> Result<()> {
        let Some(admin) = integration_admin_url() else {
            return Ok(());
        };
        let database = TestDatabase::create(&admin, "cn_bucket_private_peers").await?;
        let pool = connect_postgres(&database.database_url).await?;
        initialize_database(&pool).await?;
        let cipher =
            ChannelSecretCipher::from_key_material("cn-bucket-private-test-key-0123456789")?;
        register_channel_secret_with_epoch(&pool, &cipher, "room", "e1", &hex::encode([7; 32]))
            .await?;
        sqlx::query("INSERT INTO cn_user.subscriber_accounts(subscriber_pubkey) VALUES ('subscriber-owner')")
            .execute(&pool).await?;
        sqlx::query(
            "INSERT INTO cn_index.indexing_requests(id, requester_pubkey, kind, target_id, status)
             VALUES ('request-owner', 'subscriber-owner', 'private_channel', 'room', 'approved')",
        )
        .execute(&pool)
        .await?;
        for i in 0..6 {
            sqlx::query(
                "INSERT INTO cn_bootstrap.peer_registrations(subscriber_pubkey, endpoint_id, expires_at)
                 VALUES ('subscriber-owner', $1, NOW()+INTERVAL '90 seconds')",
            )
            .bind(format!("endpoint-{i}"))
            .execute(&pool).await?;
        }
        for i in 0..20 {
            let pubkey = format!("subscriber-u{i:02}");
            sqlx::query("INSERT INTO cn_user.subscriber_accounts(subscriber_pubkey) VALUES ($1)")
                .bind(&pubkey)
                .execute(&pool)
                .await?;
            sqlx::query(
                "INSERT INTO cn_bootstrap.peer_registrations(subscriber_pubkey, endpoint_id, expires_at)
                 VALUES ($1, $2, NOW()+INTERVAL '90 seconds')",
            )
            .bind(&pubkey)
            .bind(format!("endpoint-u{i:02}"))
            .execute(&pool)
            .await?;
        }
        let first = BucketReader::selected_private_providers(&pool, "room").await?;
        let second = BucketReader::selected_private_providers(&pool, "room").await?;
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 4);
        assert_eq!(first[0].endpoint_id, "endpoint-0");
        assert!(second.iter().any(|peer| peer.endpoint_id == "endpoint-4"));
        let third = BucketReader::selected_private_providers(&pool, "room").await?;
        assert!(third.iter().any(|peer| peer.endpoint_id == "endpoint-5"));
        pool.close().await;
        database.cleanup().await
    }

    #[tokio::test]
    async fn public_provider_window_keeps_recent_peers_and_rotates_the_rest() -> Result<()> {
        let Some(admin) = integration_admin_url() else {
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

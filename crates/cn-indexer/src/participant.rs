//! docs replica sync participant（#413 / T4 / ADR 0025 §6.2 / §6.3）。
//!
//! CN は現状 docs 非参加のため、cn-indexer が iroh-docs を駆動する常駐 participant を新設する。
//! 本モジュールは scope 管理 state（cn-core / Postgres）を真実源に、supported topic / 許可 channel の
//! 共有 replica を open して sync し、ingest pipeline を回し、supported 除外 / channel secret 失効時に
//! sync 停止 + de-index する制御を担う。
//!
//! ここでは `DocsSync` trait 越しに replica を扱うため、本番（iroh-docs）でも in-memory（テスト）でも
//! 同じ制御ロジックを駆動できる。実際の docs node 生成（`IrohDocsNode` / relay 設定）は起動側
//! （`runtime` / `main`）が行い、本モジュールへ `DocsSync` として注入する。

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use sqlx::{Row, postgres::PgPool};
use tracing::{info, warn};

use kukuri_blob_service::BlobService;
use kukuri_cn_core::{
    ChannelSecretCipher, IndexEntryStore, IndexScopeKind, NewTransmissionPrevention,
    TransmissionPreventionMutation, advance_retention_floor, apply_transmission_prevention,
    get_channel_secret, is_topic_supported, load_bootstrap_seed_peers, mark_index_demand,
    reclaim_expired, release_transmission_prevention,
};
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{DocsSync, private_channel_replica_id, topic_replica_id};
use kukuri_transport::SeedPeer;

use crate::ingest::{IngestPipeline, IngestSummary};
use crate::projection::IndexProjection;
use crate::replica_plan::PublicReplicaReadMode;

const LEGACY_SCOPE_LIMIT: usize = 32;
const PRIORITY_SCOPES: i64 = 8;
const INDEXED_SCOPE_PAGE: usize = 32;

fn admit_scopes(
    mode: PublicReplicaReadMode,
    now: i64,
    kind: IndexScopeKind,
    id: String,
    selected: &mut Vec<ScopeReplica>,
    seen: &mut HashSet<(IndexScopeKind, String)>,
) -> Result<()> {
    if selected.len() >= LEGACY_SCOPE_LIMIT || seen.contains(&(kind, id.clone())) {
        return Ok(());
    }
    let scopes = mode.scopes(kind, &id, now)?;
    if selected.len() + scopes.len() <= LEGACY_SCOPE_LIMIT {
        seen.insert((kind, id));
        selected.extend(scopes);
    }
    Ok(())
}

async fn apply_seed_peers(
    docs_sync: &dyn DocsSync,
    blob_service: Option<&dyn BlobService>,
    peers: Vec<SeedPeer>,
) -> Result<()> {
    docs_sync.set_seed_peers(peers.clone()).await?;
    if let Some(blob_service) = blob_service {
        blob_service.set_seed_peers(peers).await?;
    }
    Ok(())
}

/// scope（種別 + id）と、それが指す共有 replica id。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeReplica {
    pub kind: IndexScopeKind,
    pub id: String,
    pub replica_id: ReplicaId,
}

impl ScopeReplica {
    /// scope 種別 + id から共有 replica id を導出する（ADR 0025 §6.2 / §6.3）。
    ///
    /// - public topic: `topic::<id>`（`public_replica_secret` で open 可能）。
    /// - private channel: `channel::<id>`（登録 capability が必要）。
    pub fn from_scope(kind: IndexScopeKind, id: &str) -> Self {
        let replica_id = match kind {
            IndexScopeKind::PublicTopic => topic_replica_id(id),
            IndexScopeKind::PrivateChannel => private_channel_replica_id(id),
        };
        Self {
            kind,
            id: id.to_string(),
            replica_id,
        }
    }
}

/// docs replica sync participant の制御面。
pub struct IndexerParticipant {
    pool: PgPool,
    docs_sync: Arc<dyn DocsSync>,
    entries: Arc<dyn IndexEntryStore>,
    projection: Arc<dyn IndexProjection>,
    pipeline: IngestPipeline,
    channel_secret_cipher: ChannelSecretCipher,
    configured_seed_peers: Option<Vec<SeedPeer>>,
    blob_service: Option<Arc<dyn BlobService>>,
    public_replica_mode: PublicReplicaReadMode,
}

impl IndexerParticipant {
    pub fn new(
        pool: PgPool,
        docs_sync: Arc<dyn DocsSync>,
        entries: Arc<dyn IndexEntryStore>,
        projection: Arc<dyn IndexProjection>,
        pipeline: IngestPipeline,
        channel_secret_cipher: ChannelSecretCipher,
    ) -> Self {
        Self {
            pool,
            docs_sync,
            entries,
            projection,
            pipeline,
            channel_secret_cipher,
            configured_seed_peers: None,
            blob_service: None,
            public_replica_mode: PublicReplicaReadMode::Legacy,
        }
    }

    pub fn with_configured_seed_peers(mut self, peers: Vec<SeedPeer>) -> Self {
        self.configured_seed_peers = Some(peers);
        self
    }

    /// CNの読取りを先行検証するための選択。runtimeは切替contractの完了まではLegacyを使う。
    pub fn with_public_replica_mode(mut self, mode: PublicReplicaReadMode) -> Self {
        self.public_replica_mode = mode;
        self
    }

    pub fn with_blob_service(mut self, blob_service: Arc<dyn BlobService>) -> Self {
        self.pipeline = self.pipeline.with_blob_service(Arc::clone(&blob_service));
        self.blob_service = Some(blob_service);
        self
    }

    /// desktop heartbeat が Postgres に保持する active peer を docs sync と media fetch へ反映する。
    /// operator 指定 seed は残し、同じ endpoint の fresh addr_hint は heartbeat 側で更新する。
    pub(crate) async fn refresh_seed_peers(&self) -> Result<()> {
        let Some(configured) = self.configured_seed_peers.as_deref() else {
            return Ok(());
        };
        let active = load_bootstrap_seed_peers(&self.pool, None, None)
            .await?
            .into_iter()
            .map(|peer| SeedPeer {
                endpoint_id: peer.endpoint_id,
                addr_hint: peer.addr_hint,
            })
            .collect::<Vec<_>>();
        let peers = merge_seed_peers(configured, &active);
        apply_seed_peers(
            self.docs_sync.as_ref(),
            self.blob_service.as_deref(),
            peers.clone(),
        )
        .await?;
        info!(
            configured = configured.len(),
            active = active.len(),
            applied = peers.len(),
            media_fetch = self.blob_service.is_some(),
            "refreshed docs sync and media fetch seed peers from active bootstrap registrations"
        );
        Ok(())
    }

    /// Select at most 32 physical legacy replicas. Recent authorized demand leads; a durable
    /// cursor gives every other eligible scope a finite turn without reading the supported set.
    pub async fn selected_scopes_at(&self, now: i64) -> Result<Vec<ScopeReplica>> {
        let eligible = "FROM cn_index.supported_topics s
            LEFT JOIN cn_index.channel_secrets c
              ON s.kind = 'private_channel' AND c.channel_id = s.id
            WHERE (s.kind = 'public_topic' OR c.channel_id IS NOT NULL)";
        let priority = sqlx::query(&format!(
            "SELECT s.kind, s.id {eligible}
             AND s.last_index_demand_at >= NOW() - INTERVAL '10 minutes'
             ORDER BY s.last_index_demand_at DESC, s.kind, s.id LIMIT {PRIORITY_SCOPES}"
        ))
        .fetch_all(&self.pool)
        .await?;
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for row in priority {
            admit_scopes(
                self.public_replica_mode,
                now,
                IndexScopeKind::parse(&row.try_get::<String, _>("kind")?)?,
                row.try_get("id")?,
                &mut selected,
                &mut seen,
            )?;
        }
        let cursor = sqlx::query(
            "SELECT last_kind, last_scope_id FROM cn_index.legacy_scope_cursor WHERE id = TRUE",
        )
        .fetch_one(&self.pool)
        .await?;
        let mut last_kind: String = cursor.try_get("last_kind")?;
        let mut last_id: String = cursor.try_get("last_scope_id")?;
        for (after_kind, after_id) in [
            (last_kind.clone(), last_id.clone()),
            (String::new(), String::new()),
        ] {
            if selected.len() >= LEGACY_SCOPE_LIMIT {
                break;
            }
            let page = sqlx::query(&format!(
                "SELECT s.kind, s.id {eligible} AND (s.kind, s.id) > ($1, $2)
                 ORDER BY s.kind, s.id LIMIT 64"
            ))
            .bind(after_kind)
            .bind(after_id)
            .fetch_all(&self.pool)
            .await?;
            for row in page {
                let kind: String = row.try_get("kind")?;
                let id: String = row.try_get("id")?;
                last_kind.clone_from(&kind);
                last_id.clone_from(&id);
                admit_scopes(
                    self.public_replica_mode,
                    now,
                    IndexScopeKind::parse(&kind)?,
                    id,
                    &mut selected,
                    &mut seen,
                )?;
                if selected.len() >= LEGACY_SCOPE_LIMIT {
                    break;
                }
            }
        }
        sqlx::query(
            "UPDATE cn_index.legacy_scope_cursor
             SET last_kind = $1, last_scope_id = $2 WHERE id = TRUE",
        )
        .bind(last_kind)
        .bind(last_id)
        .execute(&self.pool)
        .await?;
        Ok(selected)
    }

    /// Seek at most 32 distinct truth-store scopes and retain the next position across restarts.
    pub async fn indexed_scope_page(&self) -> Result<Vec<(IndexScopeKind, String)>> {
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

    pub async fn is_scope_authorized(&self, kind: IndexScopeKind, id: &str) -> Result<bool> {
        if !is_topic_supported(&self.pool, kind, id).await? {
            return Ok(false);
        }
        if kind == IndexScopeKind::PrivateChannel {
            return Ok(
                get_channel_secret(&self.pool, &self.channel_secret_cipher, id)
                    .await?
                    .is_some(),
            );
        }
        Ok(true)
    }

    pub async fn mark_scope_demand(&self, scope: &ScopeReplica) -> Result<()> {
        mark_index_demand(&self.pool, scope.kind, &scope.id).await
    }

    pub async fn restore_selected_scopes(
        &self,
        scopes: &[ScopeReplica],
    ) -> Result<Vec<ScopeReplica>> {
        self.refresh_seed_peers().await?;
        let mut opened = Vec::new();
        for scope in scopes {
            if scope.kind == IndexScopeKind::PrivateChannel {
                let Some(secret) =
                    get_channel_secret(&self.pool, &self.channel_secret_cipher, scope.id.as_str())
                        .await?
                else {
                    warn!(
                        channel_id = %scope.id,
                        "private channel is supported but has no registered capability; skipping (no secret, no index)"
                    );
                    continue;
                };
                self.docs_sync
                    .register_private_replica_secret(
                        &scope.replica_id,
                        secret.namespace_secret_hex.as_str(),
                    )
                    .await?;
            }
            match self.docs_sync.open_replica(&scope.replica_id).await {
                Ok(()) => {
                    info!(
                        kind = scope.kind.as_str(),
                        scope_id = %scope.id,
                        replica_id = %scope.replica_id.as_str(),
                        "opened supported replica for sync"
                    );
                    opened.push(scope.clone());
                }
                Err(error) => {
                    warn!(
                        kind = scope.kind.as_str(),
                        scope_id = %scope.id,
                        error = %error,
                        "failed to open supported replica; skipping"
                    );
                }
            }
        }
        Ok(opened)
    }

    /// Periodic public refresh reads the bounded current window, never all `objects/` entries.
    pub async fn ingest_recent_scope(&self, scope: &ScopeReplica) -> Result<IngestSummary> {
        self.pipeline
            .ingest_recent_scope(scope.kind, scope.id.as_str(), &scope.replica_id)
            .await
    }

    /// 変更通知で届いた key に対応する object だけを ingest する（#1050）。
    pub async fn ingest_changed_keys(
        &self,
        scope: &ScopeReplica,
        keys: &[String],
    ) -> Result<IngestSummary> {
        self.pipeline
            .ingest_changed_keys(scope.kind, scope.id.as_str(), &scope.replica_id, keys)
            .await
    }

    /// Apply durable legal state, remove authoritative entries transactionally, then evict every
    /// derived projection hit. Query reconciliation is already fail-closed after the first step.
    pub async fn apply_transmission_prevention(
        &self,
        actor: &str,
        input: &NewTransmissionPrevention,
    ) -> Result<TransmissionPreventionMutation> {
        let mutation = apply_transmission_prevention(&self.pool, actor, input).await?;
        for (scope_kind, scope_id) in &mutation.removed_index_scopes {
            self.projection
                .remove_object(*scope_kind, scope_id, input.subject_id.as_str())
                .await?;
        }
        Ok(mutation)
    }

    /// Release only changes durable policy. Reappearance requires a later fresh docs ingest and
    /// successful current safety verdict; stale projections are never restored directly.
    pub async fn release_transmission_prevention(
        &self,
        actor: &str,
        subject_kind: &str,
        subject_id: &str,
        reason: &str,
    ) -> Result<TransmissionPreventionMutation> {
        release_transmission_prevention(&self.pool, actor, subject_kind, subject_id, reason).await
    }

    /// 解除した scope の同期を止め、索引を最大 `budget` 件回収して消した件数を返す（#1221 R5-F）。
    ///
    /// 投影 → 真実源の順で消す（真実源が先に空になると、残った投影を見つける入口が無くなる）。解除した scope は
    /// 検索の gate が表示から外すため、回収が済むまでの間も表示されない。撤回 marker は受入下限まで残す。
    pub async fn retire_scope(
        &self,
        kind: IndexScopeKind,
        id: &str,
        budget: usize,
    ) -> Result<usize> {
        for scope in self
            .public_replica_mode
            .scopes(kind, id, chrono::Utc::now().timestamp())?
        {
            self.stop_replica(&scope).await?;
        }
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

    /// worker の 1 回の見直しで行う回収。上限まで消せた間だけ最大 `steps` 回繰り返し、消した件数を返す。
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

    /// bucketが窓から出ただけなら同期だけを止め、論理scopeの索引は残す。
    pub async fn stop_replica(&self, scope: &ScopeReplica) -> Result<()> {
        if scope.kind == IndexScopeKind::PrivateChannel {
            self.docs_sync
                .remove_private_replica_secret(&scope.replica_id)
                .await
        } else {
            self.docs_sync.close_replica(&scope.replica_id).await
        }
    }
}

fn merge_seed_peers(configured: &[SeedPeer], active: &[SeedPeer]) -> Vec<SeedPeer> {
    let mut peers = std::collections::BTreeMap::new();
    for peer in configured.iter().chain(active.iter()) {
        peers
            .entry(peer.endpoint_id.clone())
            .and_modify(|existing: &mut SeedPeer| {
                if peer.addr_hint.is_some() {
                    existing.addr_hint.clone_from(&peer.addr_hint);
                }
            })
            .or_insert_with(|| peer.clone());
    }
    peers.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kukuri_blob_service::{BlobStatus, StoredBlob};
    use kukuri_core::BlobHash;
    use kukuri_docs_sync::MemoryDocsSync;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingBlobService {
        seed_peers: Mutex<Vec<SeedPeer>>,
    }

    #[async_trait]
    impl BlobService for RecordingBlobService {
        async fn put_blob(&self, _data: Vec<u8>, _mime: &str) -> Result<StoredBlob> {
            unreachable!("not used by this contract test")
        }

        async fn fetch_blob(&self, _hash: &BlobHash) -> Result<Option<Vec<u8>>> {
            unreachable!("not used by this contract test")
        }

        async fn pin_blob(&self, _hash: &BlobHash) -> Result<()> {
            unreachable!("not used by this contract test")
        }

        async fn blob_status(&self, _hash: &BlobHash) -> Result<BlobStatus> {
            unreachable!("not used by this contract test")
        }

        async fn local_blob_status(&self, _hash: &BlobHash) -> Result<BlobStatus> {
            unreachable!("not used by this contract test")
        }

        async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
            unreachable!("not used by this contract test")
        }

        async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
            *self.seed_peers.lock().expect("seed peer mutex poisoned") = peers;
            Ok(())
        }
    }

    #[tokio::test]
    async fn seed_refresh_applies_active_peers_to_media_fetcher() {
        let endpoint_id = "1".repeat(64);
        let peer = SeedPeer {
            endpoint_id: endpoint_id.clone(),
            addr_hint: Some("192.0.2.10:4433".to_string()),
        };
        let docs_sync = MemoryDocsSync::default();
        let blob_service = RecordingBlobService::default();

        apply_seed_peers(&docs_sync, Some(&blob_service), vec![peer.clone()])
            .await
            .expect("seed refresh succeeds");

        assert_eq!(
            *blob_service
                .seed_peers
                .lock()
                .expect("seed peer mutex poisoned"),
            vec![peer],
            "media fetcher must receive the same active peers as docs sync"
        );
    }

    #[test]
    fn public_topic_scope_maps_to_topic_replica() {
        let scope = ScopeReplica::from_scope(IndexScopeKind::PublicTopic, "rust");
        assert_eq!(scope.replica_id.as_str(), "topic::rust");
    }

    #[test]
    fn private_channel_scope_maps_to_channel_replica() {
        let scope = ScopeReplica::from_scope(IndexScopeKind::PrivateChannel, "secret-room");
        assert_eq!(scope.replica_id.as_str(), "channel::secret-room");
    }

    #[test]
    fn transition_admission_counts_physical_replicas() {
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for index in 0..80 {
            admit_scopes(
                PublicReplicaReadMode::Transition,
                10 * 86_400,
                IndexScopeKind::PublicTopic,
                format!("topic-{index:02}"),
                &mut selected,
                &mut seen,
            )
            .unwrap();
        }
        assert_eq!(selected.len(), 30);
        assert_eq!(seen.len(), 10);
    }

    #[test]
    fn active_bootstrap_peer_refreshes_configured_addr_hint() {
        let endpoint_id = "1".repeat(64);
        let peers = merge_seed_peers(
            &[SeedPeer {
                endpoint_id: endpoint_id.clone(),
                addr_hint: None,
            }],
            &[SeedPeer {
                endpoint_id: endpoint_id.clone(),
                addr_hint: Some("192.0.2.10:4433".to_string()),
            }],
        );
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].endpoint_id, endpoint_id);
        assert_eq!(peers[0].addr_hint.as_deref(), Some("192.0.2.10:4433"));
    }
}

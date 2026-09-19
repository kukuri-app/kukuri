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

use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPool;
use tracing::{info, warn};

use kukuri_blob_service::BlobService;
use kukuri_cn_core::{
    ChannelSecretCipher, IndexEntryStore, IndexScopeKind, NewTransmissionPrevention,
    TransmissionPreventionMutation, apply_transmission_prevention, list_channel_secrets,
    list_supported_topics, load_bootstrap_seed_peers, release_transmission_prevention,
};
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{DocsSync, private_channel_replica_id, topic_replica_id};
use kukuri_transport::SeedPeer;

use crate::ingest::{IngestPipeline, IngestSummary};
use crate::projection::IndexProjection;

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
        }
    }

    pub fn with_configured_seed_peers(mut self, peers: Vec<SeedPeer>) -> Self {
        self.configured_seed_peers = Some(peers);
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

    /// いま index 対象であるべき scope を返す（#613 T2。読み取りのみ、副作用なし）。
    ///
    /// サポート対象一覧のうち、private channel は秘密鍵（capability）が登録済みのものだけを
    /// 含める（鍵が無ければ索引しない）。常駐ワーカーはこの結果と索引の実在スコープ
    /// （[`Self::indexed_scopes`]）の差分から索引解除を決める。open の成否に依存しないため、
    /// 一時的な open 失敗で誤って索引解除することがない。
    pub async fn desired_scopes(&self) -> Result<Vec<ScopeReplica>> {
        let secrets = list_channel_secrets(&self.pool, &self.channel_secret_cipher).await?;
        let mut scopes = Vec::new();
        for supported in list_supported_topics(&self.pool).await? {
            let scope = ScopeReplica::from_scope(supported.kind, supported.id.as_str());
            if scope.kind == IndexScopeKind::PrivateChannel
                && !secrets.iter().any(|secret| secret.channel_id == scope.id)
            {
                continue;
            }
            scopes.push(scope);
        }
        Ok(scopes)
    }

    /// 索引の真実源にいま entry が存在する scope の一覧（#613 T2）。
    pub async fn indexed_scopes(&self) -> Result<Vec<(IndexScopeKind, String)>> {
        self.entries.list_scopes().await
    }

    /// 起動時 / 再起動時に scope 管理 state から replica を open して sync 復元する（E13）。
    ///
    /// supported topic（public / private channel）の replica を open し、private channel は登録済み
    /// capability（channel secret）を docs へ登録してから open する。secret 未登録の private channel は
    /// open せず warn する（secret 無しでは index しない）。
    pub async fn restore_scopes(&self) -> Result<Vec<ScopeReplica>> {
        self.refresh_seed_peers().await?;

        // private channel の capability を先に docs へ登録する。
        let secrets = list_channel_secrets(&self.pool, &self.channel_secret_cipher).await?;
        for secret in &secrets {
            let replica_id = private_channel_replica_id(secret.channel_id.as_str());
            self.docs_sync
                .register_private_replica_secret(&replica_id, secret.namespace_secret_hex.as_str())
                .await?;
        }

        let mut opened = Vec::new();
        for supported in list_supported_topics(&self.pool).await? {
            let scope = ScopeReplica::from_scope(supported.kind, supported.id.as_str());
            // private channel は capability が登録されていなければ open しない。
            if scope.kind == IndexScopeKind::PrivateChannel
                && !secrets.iter().any(|secret| secret.channel_id == scope.id)
            {
                warn!(
                    channel_id = %scope.id,
                    "private channel is supported but has no registered capability; skipping (no secret, no index)"
                );
                continue;
            }
            match self.docs_sync.open_replica(&scope.replica_id).await {
                Ok(()) => {
                    info!(
                        kind = scope.kind.as_str(),
                        scope_id = %scope.id,
                        replica_id = %scope.replica_id.as_str(),
                        "opened supported replica for sync"
                    );
                    opened.push(scope);
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

    /// 単一 scope を ingest する（scan→allow→投影）。
    pub async fn ingest_scope(&self, scope: &ScopeReplica) -> Result<IngestSummary> {
        self.pipeline
            .ingest_scope(scope.kind, scope.id.as_str(), &scope.replica_id)
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

    /// supported set 全体を 1 巡 ingest する。
    pub async fn ingest_all_supported(&self) -> Result<IngestSummary> {
        let scopes = self.restore_scopes().await?;
        let mut total = IngestSummary::default();
        for scope in scopes {
            match self.ingest_scope(&scope).await {
                Ok(summary) => {
                    total.scanned += summary.scanned;
                    total.indexed += summary.indexed;
                    total.skipped_non_allow += summary.skipped_non_allow;
                    total.deindexed += summary.deindexed;
                    total.scans_fresh += summary.scans_fresh;
                    total.scans_reused += summary.scans_reused;
                }
                Err(error) => warn!(
                    kind = scope.kind.as_str(),
                    scope_id = %scope.id,
                    error = %error,
                    "failed to ingest scope; continuing"
                ),
            }
        }
        Ok(total)
    }

    /// supported topic 除外時の sync 停止 + de-index（E2 / E5）。
    ///
    /// public topic の replica はここでは docs から secret を外せない（導出 secret のため）が、
    /// 真実源と投影を de-index することで検索面から消える。private channel は capability も外す。
    pub async fn stop_and_deindex_scope(&self, kind: IndexScopeKind, id: &str) -> Result<()> {
        let scope = ScopeReplica::from_scope(kind, id);
        if kind == IndexScopeKind::PrivateChannel {
            // capability を外して sync 停止する（remove_private_replica_secret が replica も閉じる）。
            self.docs_sync
                .remove_private_replica_secret(&scope.replica_id)
                .await?;
        }
        // 真実源 → 投影の順で消す（投影削除が失敗しても query 境界の突合が即座に効く）。
        self.entries.remove_scope(kind, id).await?;
        self.projection.remove_scope(kind, id).await?;
        info!(
            kind = kind.as_str(),
            scope_id = %id,
            "stopped sync and de-indexed scope"
        );
        Ok(())
    }

    /// channel secret 失効時の capability 除去 + sync 停止 + de-index（E4）。
    pub async fn revoke_channel_and_deindex(&self, channel_id: &str) -> Result<()> {
        self.stop_and_deindex_scope(IndexScopeKind::PrivateChannel, channel_id)
            .await
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

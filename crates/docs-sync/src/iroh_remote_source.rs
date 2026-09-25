use super::*;
use crate::buckets::{BucketReplica, BucketScope};
use crate::remote_source::RemoteDocsSource;
use crate::replicas::{PostReplicaKind, post_replica_kind};
use kukuri_iroh_node::{DocReadQuery, DocReadResponse};

impl IrohDocsSync {
    pub fn with_account_store(node: Arc<IrohDocsNode>, store: Arc<SqliteStore>) -> Self {
        let mut docs = Self::new(node.clone());
        docs.peers = Arc::new(PeerAddrBook::with_account_store(
            node.endpoint().clone(),
            node.discovery(),
            Arc::new(BlobPeerHealth::default()),
            store.clone(),
            "docs",
        ));
        docs.remote_cache = Some(store);
        docs
    }

    pub(crate) fn remote_cache(&self) -> Option<&SqliteStore> {
        self.remote_cache.as_deref()
    }

    /// A moving, bounded candidate window; callers keep each object read on one provider.
    pub async fn remote_read_candidates(&self) -> Vec<EndpointAddr> {
        self.peers.ranked_peers().await
    }

    /// Explicit provider IDs avoid sampling the global peer window for a private scope.
    pub async fn remote_read_candidates_for_seeds(
        &self,
        seeds: Vec<SeedPeer>,
    ) -> Result<Vec<EndpointAddr>> {
        let relay_urls = self
            .node
            .relay_urls()
            .await
            .into_iter()
            .map(|url| url.to_string())
            .collect::<Vec<_>>();
        seeds
            .into_iter()
            .map(|seed| seed.to_endpoint_addr_with_relay_url_strings(&relay_urls))
            .collect()
    }

    pub fn remote_source(&self, peer: EndpointAddr) -> RemoteDocsSource {
        RemoteDocsSource::new(self.clone(), peer)
    }

    pub fn remote_source_with_private_secret(
        &self,
        peer: EndpointAddr,
        replica: ReplicaId,
        secret: [u8; 32],
    ) -> RemoteDocsSource {
        RemoteDocsSource::with_private_secret(self.clone(), peer, replica, secret)
    }

    pub(crate) async fn remote_readers_owned(
        &self,
        replica: &ReplicaId,
        private_secret: Option<[u8; 32]>,
        scope_peers: Vec<SeedPeer>,
    ) -> Result<Vec<Arc<dyn DocsSync>>> {
        let private = match post_replica_kind(replica) {
            Some(kind) => matches!(kind, PostReplicaKind::PrivateChannel { .. }),
            None if author_replica(replica) => false,
            None => anyhow::bail!("remote reader requires a post or author replica"),
        };
        anyhow::ensure!(
            private == private_secret.is_some(),
            "remote reader capability does not match scope"
        );
        let peers = if private || !scope_peers.is_empty() {
            self.remote_read_candidates_for_seeds(scope_peers.into_iter().take(4).collect())
                .await?
        } else {
            self.remote_read_candidates().await
        };
        Ok(peers
            .into_iter()
            .map(|peer| match private_secret {
                Some(secret) => {
                    Arc::new(self.remote_source_with_private_secret(peer, replica.clone(), secret))
                        as Arc<dyn DocsSync>
                }
                None => Arc::new(self.remote_source(peer)) as Arc<dyn DocsSync>,
            })
            .collect())
    }
}

/// The author control replica or one of its history buckets; both are public.
fn author_replica(replica: &ReplicaId) -> bool {
    match replica.as_str().strip_prefix("author::") {
        Some(pubkey) => !pubkey.is_empty() && !pubkey.contains("::"),
        None => BucketReplica::parse(replica)
            .is_ok_and(|bucket| matches!(bucket.scope(), BucketScope::Author { .. })),
    }
}

impl IrohDocsSync {
    pub(crate) async fn query_remote_docs_with_secret(
        &self,
        replica: &ReplicaId,
        peer: EndpointAddr,
        query: DocReadQuery,
        private_secret: Option<&iroh_docs::NamespaceSecret>,
    ) -> Result<DocReadResponse> {
        let secret = match private_secret {
            Some(secret) => secret.clone(),
            None => self.replica_secret(replica).await?,
        };
        self.node
            .query_remote_docs(peer, replica, &secret, query)
            .await
    }
}

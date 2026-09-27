use super::*;

impl IrohGossipTransport {
    pub(crate) async fn insert_imported_peer_addr(
        &self,
        endpoint_addr: EndpointAddr,
    ) -> Result<()> {
        if let Some(store) = &self.account_store {
            store
                .put_peer_candidate(
                    "gossip",
                    "imported",
                    &endpoint_addr.id.to_string(),
                    &serde_json::to_vec(&endpoint_addr)?,
                    Utc::now().timestamp_millis(),
                )
                .await?;
        } else {
            self.imported_peers
                .lock()
                .await
                .insert(endpoint_addr.id.to_string(), endpoint_addr.clone());
        }
        self.remember_hot_endpoint(endpoint_addr.clone()).await;
        self.extend_topic_peers(None, vec![endpoint_addr], "imported-peer")
            .await;
        Ok(())
    }

    pub(crate) async fn transport_import_ticket_impl(&self, ticket: &str) -> Result<()> {
        let endpoint_addr = match parse_endpoint_ticket(ticket) {
            Ok(endpoint_addr) => endpoint_addr,
            Err(error) => {
                let message = format!("failed to import peer ticket: {error}");
                *self.last_error.lock().await = Some(message.clone());
                return Err(anyhow!(message));
            }
        };
        self.insert_imported_peer_addr(endpoint_addr).await?;
        *self.last_error.lock().await = None;
        Ok(())
    }

    pub(crate) async fn transport_configure_discovery_impl(
        &self,
        mode: DiscoveryMode,
        env_locked: bool,
        configured_seed_peers: Vec<SeedPeer>,
        bootstrap_seed_peers: Vec<SeedPeer>,
    ) -> Result<()> {
        let relay_urls = self
            .relay_urls
            .read()
            .expect("transport relay urls poisoned")
            .clone();
        if !relay_urls.is_empty() {
            let endpoint = self.endpoint.clone();
            tokio::spawn(async move {
                endpoint.online().await;
            });
        }
        let mut configured = BTreeMap::new();
        for seed in configured_seed_peers {
            let endpoint_addr = seed.to_endpoint_addr_with_relays(&relay_urls)?;
            if self.account_store.is_none() {
                self.discovery.add_endpoint_info(endpoint_addr.clone());
            }
            configured.insert(endpoint_addr.id.to_string(), endpoint_addr);
        }
        let mut bootstrap = BTreeMap::new();
        for seed in bootstrap_seed_peers {
            let endpoint_addr = seed.to_endpoint_addr_with_relays(&relay_urls)?;
            if self.account_store.is_none() {
                self.discovery.add_endpoint_info(endpoint_addr.clone());
            }
            bootstrap.insert(endpoint_addr.id.to_string(), endpoint_addr);
        }
        *self.discovery_mode.lock().await = mode;
        *self.env_locked.lock().await = env_locked;
        // 既存の topic へは join しない。seed が増えたときだけ、neighbor の無い topic の再 join へ知らせる
        // (#1221 R2-B。neighbor のある topic は触らない)。
        let mut added = false;
        for (current, next) in [
            (&self.configured_seed_peers, configured),
            (&self.bootstrap_seed_peers, bootstrap),
        ] {
            let mut current = current.lock().await;
            added |= next.keys().any(|id| !current.contains_key(id));
            *current = next;
        }
        if added {
            self.candidates_added.send_modify(|version| *version += 1);
        }
        *self.last_error.lock().await = None;
        self.status_changes.mark(StatusKey::Summary);
        Ok(())
    }

    /// 件数だけを返す。seed・取り込んだ ticket の一覧は詳細のページで読む(#1221 R2-D)。
    pub(crate) async fn transport_discovery_impl(&self) -> Result<DiscoverySnapshot> {
        let bootstrap_seed_peer_count = self.bootstrap_seed_peers.lock().await.len();
        Ok(DiscoverySnapshot {
            mode: self.discovery_mode.lock().await.clone(),
            connect_mode: self.connect_mode.lock().await.clone(),
            active_path: if bootstrap_seed_peer_count == 0 {
                ConnectionPath::DirectP2p
            } else {
                ConnectionPath::RelaySupportedP2p
            },
            env_locked: *self.env_locked.lock().await,
            configured_seed_peer_count: self.configured_seed_peers.lock().await.len(),
            bootstrap_seed_peer_count,
            connected_peer_count: self.connected_peer_count().await,
            local_endpoint_id: self.endpoint.id().to_string(),
            last_discovery_error: self.last_error.lock().await.clone(),
        })
    }
}

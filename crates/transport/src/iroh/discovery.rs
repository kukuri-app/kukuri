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
        self.extend_active_topic_peers(vec![endpoint_addr], "imported-peer")
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
        *self.configured_seed_peers.lock().await = configured;
        *self.bootstrap_seed_peers.lock().await = bootstrap;
        *self.last_error.lock().await = None;
        self.extend_active_topic_peers(self.bootstrap_peers().await?, "seed-update")
            .await;
        Ok(())
    }

    pub(crate) async fn transport_discovery_impl(&self) -> Result<DiscoverySnapshot> {
        let configured_seed_peer_ids = self.configured_seed_peer_ids().await;
        let bootstrap_seed_peer_ids = self.bootstrap_seed_peer_ids().await;
        let manual_ticket_peer_ids = if let Some(store) = &self.account_store {
            store
                .peer_candidate_window("gossip", "imported", None, 4, Utc::now().timestamp_millis())
                .await?
                .into_iter()
                .map(|(id, _, _)| id)
                .collect()
        } else {
            self.imported_peers.lock().await.keys().cloned().collect()
        };
        Ok(DiscoverySnapshot {
            mode: self.discovery_mode.lock().await.clone(),
            connect_mode: self.connect_mode.lock().await.clone(),
            active_path: if bootstrap_seed_peer_ids.is_empty() {
                ConnectionPath::DirectP2p
            } else {
                ConnectionPath::RelaySupportedP2p
            },
            fallback_peer_ids: Vec::new(),
            env_locked: *self.env_locked.lock().await,
            configured_seed_peer_ids,
            bootstrap_seed_peer_ids,
            manual_ticket_peer_ids,
            connected_peer_ids: self.connected_peer_ids().await,
            local_endpoint_id: self.endpoint.id().to_string(),
            last_discovery_error: self.last_error.lock().await.clone(),
        })
    }
}

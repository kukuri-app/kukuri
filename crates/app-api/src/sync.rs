use crate::service::*;

impl AppService {
    pub async fn get_sync_status(&self) -> Result<SyncStatus> {
        let PeerSnapshot {
            connected,
            peer_count,
            connected_peers: _,
            configured_peers,
            subscribed_topics,
            active_path,
            fallback_peer_ids,
            pending_events,
            status_detail,
            last_error,
            topic_diagnostics,
        } = self.services.transport.peers().await?;
        let subscribed_topics = normalize_topics(subscribed_topics);
        let topic_diagnostics = normalize_topic_diagnostics(topic_diagnostics);
        let docs_assist_peer_ids = self.docs_assisted_peer_ids().await?;
        let discovery = self.get_discovery_status().await?;
        let mut effective_delivery_state = if connected {
            DeliveryState::Live
        } else {
            DeliveryState::Offline
        };
        let mut effective_last_docs_activity_at = None;
        let mut effective_topic_diagnostics = Vec::with_capacity(topic_diagnostics.len());

        for diagnostic in topic_diagnostics {
            let delivery = self
                .public_topic_delivery_status(diagnostic.topic.as_str())
                .await;
            let last_docs_activity_at = delivery.and_then(|status| status.last_docs_activity_at);
            let delivery_state = delivery_state_for_topic(
                diagnostic.connected_peers.len(),
                docs_assist_peer_ids.len(),
                last_docs_activity_at,
            );
            effective_delivery_state =
                combine_delivery_states(effective_delivery_state, delivery_state);
            effective_last_docs_activity_at =
                merge_optional_timestamp(effective_last_docs_activity_at, last_docs_activity_at);
            effective_topic_diagnostics.push(TopicSyncStatus {
                topic: diagnostic.topic,
                joined: diagnostic.joined,
                delivery_state,
                peer_count: diagnostic.peer_count,
                connected_peers: diagnostic.connected_peers,
                docs_assist_peer_ids: docs_assist_peer_ids.clone(),
                configured_peer_ids: diagnostic.configured_peer_ids,
                missing_peer_ids: diagnostic.missing_peer_ids,
                active_path: diagnostic.active_path,
                rendezvous_peer_ids: diagnostic.rendezvous_peer_ids,
                fallback_peer_ids: diagnostic.fallback_peer_ids,
                last_received_at: diagnostic.last_received_at,
                last_docs_activity_at,
                status_detail: effective_topic_status_detail(
                    diagnostic.status_detail.as_str(),
                    delivery_state,
                    docs_assist_peer_ids.len(),
                ),
                last_error: diagnostic.last_error,
            });
        }

        if effective_delivery_state == DeliveryState::Offline
            && !docs_assist_peer_ids.is_empty()
            && !subscribed_topics.is_empty()
        {
            effective_delivery_state = if effective_last_docs_activity_at.is_some() {
                DeliveryState::DurableReady
            } else {
                DeliveryState::DurableRecovering
            };
        }

        Ok(SyncStatus {
            connected,
            delivery_state: effective_delivery_state,
            last_sync_ts: *self.last_sync_ts.lock().await,
            peer_count,
            pending_events,
            status_detail: effective_sync_status_detail(
                status_detail.as_str(),
                effective_delivery_state,
                docs_assist_peer_ids.len(),
                subscribed_topics.len(),
            ),
            last_error,
            configured_peers,
            subscribed_topics,
            active_path,
            fallback_peer_ids,
            topic_diagnostics: effective_topic_diagnostics,
            local_author_pubkey: self.current_author_pubkey(),
            discovery,
            gossip_disabled_topics: self.list_gossip_disabled_topics().await,
            gossip_disabled_channels: self.list_gossip_disabled_channels().await,
        })
    }

    pub async fn get_discovery_status(&self) -> Result<DiscoveryStatus> {
        let DiscoverySnapshot {
            mode,
            connect_mode,
            active_path,
            fallback_peer_ids,
            env_locked,
            configured_seed_peer_ids,
            bootstrap_seed_peer_ids,
            manual_ticket_peer_ids,
            connected_peer_ids,
            local_endpoint_id,
            last_discovery_error,
        } = self.services.transport.discovery().await?;
        let docs_assist_peer_ids = self.docs_assisted_peer_ids().await?;
        let blob_assist_peer_ids = self.blob_assisted_peer_ids().await?;
        Ok(DiscoveryStatus {
            mode,
            connect_mode,
            active_path,
            fallback_peer_ids,
            env_locked,
            configured_seed_peer_ids,
            bootstrap_seed_peer_ids,
            manual_ticket_peer_ids,
            connected_peer_ids,
            docs_assist_peer_ids,
            blob_assist_peer_ids,
            local_endpoint_id,
            last_discovery_error,
        })
    }

    pub async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.services.transport.import_ticket(ticket).await?;
        self.services.docs_sync.import_peer_ticket(ticket).await?;
        self.services
            .blob_service
            .import_peer_ticket(ticket)
            .await?;
        Ok(())
    }

    pub async fn set_discovery_seeds(
        &self,
        mode: DiscoveryMode,
        env_locked: bool,
        configured_seed_peers: Vec<SeedPeer>,
        bootstrap_seed_peers: Vec<SeedPeer>,
    ) -> Result<()> {
        let effective_seed_peers =
            merge_seed_peers(configured_seed_peers.clone(), bootstrap_seed_peers.clone());
        self.services
            .transport
            .configure_discovery(
                mode,
                env_locked,
                configured_seed_peers,
                bootstrap_seed_peers,
            )
            .await?;
        self.services
            .docs_sync
            .set_seed_peers(effective_seed_peers.clone())
            .await?;
        self.services
            .blob_service
            .set_seed_peers(effective_seed_peers)
            .await?;
        Ok(())
    }

    /// 表示中の列と CLI の desired のうち、この topic を持つ holder を外す。live・Dome・private channel の
    /// 参加は止めない(#1221 R2-C)。
    pub async fn unsubscribe_topic(&self, topic_id: &str) -> Result<()> {
        let holders = self
            .subscription_registry
            .scope_leases
            .lock()
            .await
            .holders_with(&["display:", "desired:"], |key| match key {
                ScopeKey::Topic(topic) | ScopeKey::Channel(topic, _) => topic == topic_id,
                ScopeKey::Author(_) => false,
            });
        for holder in holders {
            self.release_scope_holder(&holder).await;
        }
        self.clear_public_topic_delivery(topic_id).await;
        Ok(())
    }

    /// 開いている列の需要(#1221 R2-C)。observer は列。`visible: false` でその列の holder を外す。
    pub async fn set_scope_display(&self, request: ScopeDisplayRequest) -> Result<()> {
        let holder = display_holder(&request.observer);
        if !request.visible {
            self.release_scope_holder(&holder).await;
            return Ok(());
        }
        let keys = match &request.target {
            ScopeDisplayTarget::Timeline { topic, scope } => {
                self.timeline_scope_keys(topic, scope).await?
            }
            ScopeDisplayTarget::Author { pubkey } => {
                vec![ScopeKey::Author(normalize_author_pubkey(pubkey)?)]
            }
        };
        self.set_scope_holder(&holder, keys).await
    }

    /// CLI daemon の desired(#1221 R2-C)。上限は表示・参加の lease と共通。
    pub async fn set_desired_scope(
        &self,
        topic: &str,
        scope: &TimelineScope,
        desired: bool,
    ) -> Result<()> {
        let holder = desired_holder(topic, scope);
        if desired {
            let keys = self.timeline_scope_keys(topic, scope).await?;
            self.set_scope_holder(&holder, keys).await
        } else {
            self.release_scope_holder(&holder).await;
            Ok(())
        }
    }

    pub async fn peer_ticket(&self) -> Result<Option<String>> {
        self.services.transport.export_ticket().await
    }

    /// gossip の停止・再開。lease は残し、task だけを止める・起こす。
    pub async fn set_topic_gossip_enabled(&self, topic_id: &str, enabled: bool) -> Result<()> {
        if enabled {
            self.gossip_disabled_topics.lock().await.remove(topic_id);
        } else {
            self.gossip_disabled_topics
                .lock()
                .await
                .insert(topic_id.to_string());
        }
        self.restart_topic_scope_subscriptions(topic_id).await;
        Ok(())
    }

    pub async fn set_channel_gossip_enabled(
        &self,
        topic_id: &str,
        channel_id: &str,
        enabled: bool,
    ) -> Result<()> {
        let key = gossip_disabled_channel_key(topic_id, channel_id);
        if enabled {
            self.gossip_disabled_channels.lock().await.remove(&key);
        } else {
            self.gossip_disabled_channels.lock().await.insert(key);
        }
        self.restart_scope_subscription(&ScopeKey::Channel(
            topic_id.to_string(),
            channel_id.to_string(),
        ))
        .await;
        Ok(())
    }
}

/// 開いている列の需要。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ScopeDisplayRequest {
    pub observer: String,
    pub target: ScopeDisplayTarget,
    pub visible: bool,
}

/// 列が購読する対象。timeline の channel scope は、topic と channel の 2 key を取る。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeDisplayTarget {
    Timeline { topic: String, scope: TimelineScope },
    Author { pubkey: String },
}

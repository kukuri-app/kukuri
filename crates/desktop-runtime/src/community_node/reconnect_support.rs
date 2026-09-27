use super::*;

impl DesktopRuntime {
    pub(crate) async fn maybe_self_heal_community_node_connectivity(
        &self,
        status: &kukuri_app_api::SyncStatus,
    ) -> bool {
        if !self
            .community_node_reconnect_is_applicable(status)
            .await
            .unwrap_or(false)
            || !community_node_status_needs_reconnect(status)
        {
            self.reset_community_node_reconnect_state().await;
            return false;
        }

        let now = Utc::now().timestamp();
        {
            let mut state = self.community_node_reconnect_state.lock().await;
            let unhealthy_since = *state.unhealthy_since.get_or_insert(now);
            if now.saturating_sub(unhealthy_since) < COMMUNITY_NODE_RECONNECT_UNHEALTHY_SECONDS {
                return false;
            }
            if state.next_retry_at > now {
                return false;
            }
        }

        let Ok(_guard) = self.community_node_reconnect_guard.try_lock() else {
            return false;
        };

        match self.reconnect_community_node_connectivity().await {
            Ok(()) => {
                info!(target: "kukuri_connectivity", "community-node connectivity self-heal completed");
                self.reset_community_node_reconnect_state().await;
                true
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "community-node connectivity self-heal failed; retrying with backoff"
                );
                self.schedule_community_node_reconnect_retry().await;
                false
            }
        }
    }

    async fn community_node_reconnect_is_applicable(
        &self,
        status: &kukuri_app_api::SyncStatus,
    ) -> Result<bool> {
        if status.subscribed_topics.is_empty() {
            return Ok(false);
        }

        let config = self.community_node_config.lock().await.clone();
        for node in config.nodes {
            let node_status = self.community_node_status(node, None, None).await?;
            let Some(resolved_urls) = node_status.resolved_urls.as_ref() else {
                continue;
            };
            let has_connectivity_inputs =
                !resolved_urls.connectivity_urls.is_empty() || !resolved_urls.seed_peers.is_empty();
            let consent_accepted = node_status
                .consent_state
                .as_ref()
                .is_some_and(|consent| consent.all_required_accepted);
            // 参加拒否(AwaitingAdmission)のノードは利用者の操作待ちであり、自己修復の対象にしない(#708)。
            if has_connectivity_inputs
                && node_status.auth_state.authenticated
                && node_status.admission_rejection.is_none()
                && consent_accepted
                && node_status.last_error.is_none()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn reconnect_community_node_connectivity(&self) -> Result<()> {
        let base_urls = self.ready_community_node_base_urls().await?;
        if base_urls.is_empty() {
            return Ok(());
        }

        for base_url in base_urls {
            self.refresh_community_node_metadata(CommunityNodeTargetRequest { base_url })
                .await?;
        }

        self.repair_community_node_connectivity().await
    }

    pub(crate) async fn repair_community_node_connectivity(&self) -> Result<()> {
        // A missing remote peer is not a failed local actor. Reapply discovery
        // and subscriptions first; only a failed local probe warrants rebuilding.
        self.apply_community_node_connectivity(None).await?;
        Ok(())
    }

    pub(crate) async fn ready_community_node_base_urls(&self) -> Result<Vec<String>> {
        let config = self.community_node_config.lock().await.clone();
        let mut base_urls = Vec::new();
        for node in config.nodes {
            let node_status = self.community_node_status(node, None, None).await?;
            let Some(resolved_urls) = node_status.resolved_urls.as_ref() else {
                continue;
            };
            let has_connectivity_inputs =
                !resolved_urls.connectivity_urls.is_empty() || !resolved_urls.seed_peers.is_empty();
            let consent_accepted = node_status
                .consent_state
                .as_ref()
                .is_some_and(|consent| consent.all_required_accepted);
            // 参加拒否(AwaitingAdmission)のノードは利用者の操作待ちであり、自己修復の対象にしない(#708)。
            if has_connectivity_inputs
                && node_status.auth_state.authenticated
                && node_status.admission_rejection.is_none()
                && consent_accepted
                && node_status.last_error.is_none()
            {
                base_urls.push(node_status.base_url);
            }
        }
        Ok(base_urls)
    }

    async fn reset_community_node_reconnect_state(&self) {
        *self.community_node_reconnect_state.lock().await = CommunityNodeReconnectState::default();
    }

    async fn schedule_community_node_reconnect_retry(&self) {
        let now = Utc::now().timestamp();
        let mut state = self.community_node_reconnect_state.lock().await;
        let delay = COMMUNITY_NODE_RECONNECT_BACKOFF_SECONDS[state.backoff_step.min(
            COMMUNITY_NODE_RECONNECT_BACKOFF_SECONDS
                .len()
                .saturating_sub(1),
        )];
        state.next_retry_at = now.saturating_add(delay);
        if state.backoff_step + 1 < COMMUNITY_NODE_RECONNECT_BACKOFF_SECONDS.len() {
            state.backoff_step += 1;
        }
    }
}

fn community_node_status_needs_reconnect(status: &kukuri_app_api::SyncStatus) -> bool {
    status.topic_diagnostics.iter().any(|topic| {
        !topic.configured_peer_ids.is_empty()
            && (topic.peer_count == 0 || topic.last_error.is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kukuri_app_api::{DeliveryState, DiscoveryStatus, SyncStatus, TopicSyncStatus};
    use kukuri_transport::{ConnectMode, DiscoveryMode};

    fn sync_status_with_topic(
        configured_peer_ids: Vec<String>,
        connected_peers: Vec<String>,
        last_error: Option<String>,
    ) -> SyncStatus {
        let connected = !connected_peers.is_empty();
        SyncStatus {
            connected,
            delivery_state: if connected {
                DeliveryState::Live
            } else {
                DeliveryState::Offline
            },
            last_sync_ts: None,
            peer_count: connected_peers.len(),
            pending_events: 0,
            status_detail: String::new(),
            last_error: None,
            configured_peers: configured_peer_ids.clone(),
            subscribed_topics: vec!["hint/kukuri:topic:test".to_string()],
            active_path: Default::default(),
            fallback_peer_ids: Vec::new(),
            topic_diagnostics: vec![TopicSyncStatus {
                topic: "hint/kukuri:topic:test".to_string(),
                joined: connected,
                delivery_state: if connected {
                    DeliveryState::Live
                } else {
                    DeliveryState::Offline
                },
                peer_count: connected_peers.len(),
                connected_peers,
                docs_assist_peer_ids: Vec::new(),
                configured_peer_ids,
                missing_peer_ids: Vec::new(),
                active_path: Default::default(),
                rendezvous_peer_ids: Vec::new(),
                fallback_peer_ids: Vec::new(),
                last_received_at: None,
                last_docs_activity_at: None,
                status_detail: String::new(),
                last_error,
            }],
            local_author_pubkey: String::new(),
            discovery: DiscoveryStatus {
                mode: DiscoveryMode::StaticPeer,
                connect_mode: ConnectMode::DirectOrRelay,
                active_path: Default::default(),
                fallback_peer_ids: Vec::new(),
                env_locked: false,
                configured_seed_peer_ids: Vec::new(),
                bootstrap_seed_peer_ids: Vec::new(),
                manual_ticket_peer_ids: Vec::new(),
                connected_peer_ids: Vec::new(),
                docs_assist_peer_ids: Vec::new(),
                blob_assist_peer_ids: Vec::new(),
                local_endpoint_id: String::new(),
                last_discovery_error: None,
            },
            gossip_disabled_topics: Vec::new(),
            gossip_disabled_channels: Vec::new(),
        }
    }

    #[test]
    fn community_node_reconnect_detection_requires_configured_peer_without_direct_peer() {
        assert!(community_node_status_needs_reconnect(
            &sync_status_with_topic(vec!["peer-a".to_string()], Vec::new(), None,)
        ));
        assert!(!community_node_status_needs_reconnect(
            &sync_status_with_topic(vec!["peer-a".to_string()], vec!["peer-a".to_string()], None,)
        ));
        assert!(!community_node_status_needs_reconnect(
            &sync_status_with_topic(Vec::new(), Vec::new(), None,)
        ));
    }

    #[test]
    fn community_node_reconnect_detection_treats_topic_error_as_unhealthy() {
        assert!(community_node_status_needs_reconnect(
            &sync_status_with_topic(
                vec!["peer-a".to_string()],
                vec!["peer-a".to_string()],
                Some("gossip receiver closed".to_string()),
            )
        ));
    }
}

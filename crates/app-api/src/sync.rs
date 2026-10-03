use crate::service::*;

/// 詳細のページの 1 回の上限(#1221 R2-D。稼働中の scope の上限と同じ)。
pub const CONNECTIVITY_PEER_PAGE_LIMIT: usize = MAX_ACTIVE_SCOPES;

impl AppService {
    pub async fn get_sync_status(&self) -> Result<SyncStatus> {
        Ok(self.sync_status_delta(None).await?.0)
    }

    /// 通常の通信状態を、transport と app-api が保持している状態から作る。全台帳の走査・SQLite の読取り・
    /// 取得の候補の cursor の前進を起こさない(#1221 R2-D)。
    ///
    /// `changed` を渡すと、`topic_diagnostics` はその印の topic だけにし、印の付いた topic のうち
    /// 稼働していないもの(抜けた topic)を 2 つ目に返す。
    pub async fn sync_status_delta(
        &self,
        changed: Option<&BTreeSet<StatusKey>>,
    ) -> Result<(SyncStatus, Vec<String>)> {
        let PeerSnapshot {
            connected,
            peer_count,
            configured_peer_count,
            subscribed_topics,
            active_path,
            fallback_peer_count,
            pending_events,
            status_detail,
            last_error,
            topic_diagnostics,
        } = self.services.transport.peers().await?;
        let subscribed_topics = normalize_topics(subscribed_topics);
        let discovery = self.get_discovery_status().await?;
        let docs_assist = discovery.docs_assist_peer_count;
        let mut effective_delivery_state = if connected {
            DeliveryState::Live
        } else {
            DeliveryState::Offline
        };
        let mut effective_last_docs_activity_at = None;
        let mut effective_topic_diagnostics = Vec::with_capacity(topic_diagnostics.len());
        for diagnostic in normalize_topic_diagnostics(topic_diagnostics) {
            let last_docs_activity_at = self
                .public_topic_delivery_status(diagnostic.topic.as_str())
                .await
                .and_then(|status| status.last_docs_activity_at);
            let delivery_state =
                delivery_state_for_topic(diagnostic.peer_count, docs_assist, last_docs_activity_at);
            effective_delivery_state =
                combine_delivery_states(effective_delivery_state, delivery_state);
            effective_last_docs_activity_at =
                merge_optional_timestamp(effective_last_docs_activity_at, last_docs_activity_at);
            effective_topic_diagnostics.push(TopicSyncStatus {
                joined: diagnostic.joined,
                delivery_state,
                peer_count: diagnostic.peer_count,
                configured_peer_count: diagnostic.configured_peer_count,
                missing_peer_count: diagnostic.missing_peer_count,
                active_path: diagnostic.active_path,
                rendezvous_peer_count: diagnostic.rendezvous_peer_count,
                fallback_peer_count: diagnostic.fallback_peer_count,
                last_received_at: diagnostic.last_received_at,
                last_docs_activity_at,
                status_detail: effective_topic_status_detail(
                    diagnostic.status_detail.as_str(),
                    delivery_state,
                    docs_assist,
                ),
                last_error: diagnostic.last_error,
                topic: diagnostic.topic,
            });
        }

        if effective_delivery_state == DeliveryState::Offline
            && docs_assist > 0
            && !subscribed_topics.is_empty()
        {
            effective_delivery_state = if effective_last_docs_activity_at.is_some() {
                DeliveryState::DurableReady
            } else {
                DeliveryState::DurableRecovering
            };
        }
        let removed_topics = match changed {
            Some(keys) if !keys.contains(&StatusKey::All) => {
                let dirty = keys
                    .iter()
                    .filter_map(|key| match key {
                        StatusKey::Topic(topic) => normalize_topic_name(topic.clone()),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                effective_topic_diagnostics.retain(|diagnostic| dirty.contains(&diagnostic.topic));
                dirty
                    .into_iter()
                    .filter(|topic| {
                        !effective_topic_diagnostics
                            .iter()
                            .any(|diagnostic| &diagnostic.topic == topic)
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let status = SyncStatus {
            connected,
            delivery_state: effective_delivery_state,
            last_sync_ts: self.last_sync_ts.get().await,
            peer_count,
            pending_events,
            status_detail: effective_sync_status_detail(
                status_detail.as_str(),
                effective_delivery_state,
                docs_assist,
                subscribed_topics.len(),
            ),
            last_error,
            configured_peer_count,
            subscribed_topics,
            active_path,
            fallback_peer_count,
            topic_diagnostics: effective_topic_diagnostics,
            local_author_pubkey: self.current_author_pubkey(),
            discovery,
            // 保存済みの停止設定(手元の集合)。topic をやめると、その topic の設定も消える。
            gossip_disabled_topics: self.list_gossip_disabled_topics().await,
            gossip_disabled_channels: self.list_gossip_disabled_channels().await,
            account_sync: self.last_account_sync_status().await,
        };
        Ok((status, removed_topics))
    }

    /// 件数だけを返す。補助の peer は、取得の成功を観測した peer の数(#1221 R2-D)。
    pub async fn get_discovery_status(&self) -> Result<DiscoveryStatus> {
        let DiscoverySnapshot {
            mode,
            connect_mode,
            active_path,
            env_locked,
            configured_seed_peer_count,
            bootstrap_seed_peer_count,
            connected_peer_count,
            local_endpoint_id,
            last_discovery_error,
        } = self.services.transport.discovery().await?;
        Ok(DiscoveryStatus {
            mode,
            connect_mode,
            active_path,
            env_locked,
            configured_seed_peer_count,
            bootstrap_seed_peer_count,
            connected_peer_count,
            docs_assist_peer_count: self.docs_assisted_peer_ids().await?.len(),
            blob_assist_peer_count: self.blob_assisted_peer_ids().await?.len(),
            local_endpoint_id,
            last_discovery_error,
        })
    }

    /// 設定画面の詳細が開いたときに読む peer の一覧の 1 ページ(id の順、最大 64 件。#1221 R2-D)。
    pub async fn list_connectivity_peers(
        &self,
        request: ConnectivityPeersRequest,
    ) -> Result<PeerPage> {
        let limit = request
            .limit
            .unwrap_or(CONNECTIVITY_PEER_PAGE_LIMIT)
            .clamp(1, CONNECTIVITY_PEER_PAGE_LIMIT);
        let cursor = request.cursor.as_deref();
        match request.kind {
            ConnectivityPeerKind::DocsAssist | ConnectivityPeerKind::BlobAssist => {
                let mut ids = if request.kind == ConnectivityPeerKind::DocsAssist {
                    self.docs_assisted_peer_ids().await?
                } else {
                    self.blob_assisted_peer_ids().await?
                };
                ids.sort();
                Ok(PeerPage::from_sorted(&ids, cursor, limit))
            }
            kind => {
                let topic = request
                    .topic
                    .map(|topic| kukuri_core::wire::hint_topic_id(&TopicId::new(topic)).0);
                self.services
                    .transport
                    .peer_page(kind, topic.as_deref(), cursor, limit)
                    .await
            }
        }
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

    /// CLI の desired のうち、この topic を持つ holder を外す。開いている列の holder は列だけが外し、
    /// live・Dome・private channel の参加も止めない(#1221 R2-C)。
    pub async fn unsubscribe_topic(&self, topic_id: &str) -> Result<()> {
        let holders = self
            .subscription_registry
            .scope_leases
            .lock()
            .await
            .holders_with(&["desired:"], |key| match key {
                ScopeKey::Topic(topic) | ScopeKey::Channel(topic, _) => topic == topic_id,
                ScopeKey::Author(_) | ScopeKey::AccountSync(_) => false,
            });
        for holder in holders {
            self.release_scope_holder(&holder).await;
        }
        self.clear_public_topic_delivery(topic_id).await;
        // やめた topic の gossip の停止設定を消す(停止設定が履歴とともに増えない。#1221 R2-D)。
        self.gossip_disabled_topics.lock().await.remove(topic_id);
        let channel_prefix = gossip_disabled_channel_key(topic_id, "");
        self.gossip_disabled_channels
            .lock()
            .await
            .retain(|key| !key.starts_with(&channel_prefix));
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

/// 詳細のページの要求。`topic` は `Connected`・`Configured`・`Missing` をその topic の分に絞る。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ConnectivityPeersRequest {
    pub kind: ConnectivityPeerKind,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
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

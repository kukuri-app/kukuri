use super::*;

impl DesktopRuntime {
    pub async fn list_session_candidates(
        &self,
        request: ListLiveSessionsRequest,
    ) -> Result<Vec<kukuri_app_api::SessionCandidateView>> {
        self.app_service
            .list_session_candidates(&request.topic, request.scope)
            .await
    }

    pub async fn set_session_display(
        &self,
        request: kukuri_app_api::SessionDisplayRequest,
    ) -> Result<()> {
        self.app_service.set_session_display(request).await
    }

    /// 開いている列の購読の需要(#1221 R2-C)。
    pub async fn set_scope_display(
        &self,
        request: kukuri_app_api::ScopeDisplayRequest,
    ) -> Result<()> {
        self.app_service.set_scope_display(request).await
    }
    /// 読み取り専用。CN セッションの establish/refresh はスケジューラ
    /// (`run_community_node_session_maintenance_once`)が担い、getter は副作用を持たない。
    pub async fn get_sync_status(&self) -> Result<SyncStatus> {
        self.app_service.get_sync_status().await
    }

    /// 設定画面の詳細が開いたときに読む peer の一覧の 1 ページ(#1221 R2-D)。
    pub async fn list_connectivity_peers(
        &self,
        request: crate::ConnectivityPeersRequest,
    ) -> Result<crate::PeerPage> {
        self.app_service.list_connectivity_peers(request).await
    }

    #[cfg(test)]
    pub(crate) async fn connectivity_peer_ids(
        &self,
        kind: kukuri_transport::ConnectivityPeerKind,
    ) -> Vec<String> {
        self.list_connectivity_peers(kukuri_app_api::ConnectivityPeersRequest {
            kind,
            topic: None,
            cursor: None,
            limit: None,
        })
        .await
        .expect("peer page")
        .peer_ids
    }

    pub async fn has_topic_timeline_doc_index_entry(
        &self,
        topic: &str,
        object_id: &str,
    ) -> Result<bool> {
        let replica = kukuri_docs_sync::topic_replica_id(topic);
        let current = self.iroh_stack.current.lock().await;
        let docs_sync = current
            .as_ref()
            .context("desktop runtime stack is not initialized")?
            .docs_sync
            .clone();
        drop(current);
        // #1239: 索引を全件読まない。投稿の header から sort key を作り、索引の key を 1 つだけ確認する。
        let Some(state) = docs_sync
            .query_replica(
                &replica,
                DocQuery::Exact(kukuri_docs_sync::stable_key(
                    "objects",
                    &format!("{object_id}/state"),
                )),
            )
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let header: kukuri_core::CanonicalPostHeader = serde_json::from_slice(&state.value)?;
        let sort_key = kukuri_core::timeline_sort_key(header.created_at, &header.object_id);
        let rows = docs_sync
            .query_replica(
                &replica,
                DocQuery::Exact(kukuri_docs_sync::stable_key(
                    "indexes/timeline",
                    &format!("{sort_key}/{object_id}"),
                )),
            )
            .await?;
        Ok(!rows.is_empty())
    }

    pub async fn get_discovery_config(&self) -> Result<DiscoveryConfig> {
        Ok(self.discovery_config.lock().await.clone())
    }

    pub async fn set_discovery_seeds(
        &self,
        request: SetDiscoverySeedsRequest,
    ) -> Result<DiscoveryConfig> {
        self.update_discovery_config(|config| {
            config.seed_peers = parse_seed_entries(&request.seed_entries)?;
            Ok(())
        })
        .await
    }

    /// 公開コンテンツの発見の切替（#1632 D2）。関連する要求を止めてから、DHT・補助 index・endpoint を組み直す。
    pub async fn set_public_blob_discovery(
        &self,
        request: SetPublicBlobDiscoveryRequest,
    ) -> Result<DiscoveryConfig> {
        self.update_discovery_config(|config| {
            config.public_blob_discovery = request.enabled;
            Ok(())
        })
        .await
    }

    async fn update_discovery_config(
        &self,
        update: impl FnOnce(&mut DiscoveryConfig) -> Result<()>,
    ) -> Result<DiscoveryConfig> {
        let mut next_config = self.discovery_config.lock().await.clone();
        if next_config.env_locked {
            bail!("discovery configuration is locked by environment variables");
        }
        update(&mut next_config)?;
        save_discovery_config(&self.db_path, &next_config.stored()).await?;
        *self.discovery_config.lock().await = next_config.clone();
        self.apply_community_node_connectivity(None).await?;
        Ok(next_config)
    }

    pub async fn list_live_sessions(
        &self,
        request: ListLiveSessionsRequest,
    ) -> Result<Vec<LiveSessionView>> {
        self.app_service
            .list_live_sessions_scoped(request.topic.as_str(), request.scope)
            .await
    }

    pub async fn create_live_session(&self, request: CreateLiveSessionRequest) -> Result<String> {
        self.app_service
            .create_live_session_in_channel(
                request.topic.as_str(),
                request.channel_ref,
                CreateLiveSessionInput {
                    title: request.title,
                    description: request.description,
                },
            )
            .await
    }

    pub async fn end_live_session(&self, request: LiveSessionCommandRequest) -> Result<()> {
        self.app_service
            .end_live_session(request.topic.as_str(), request.session_id.as_str())
            .await
    }

    pub async fn join_live_session(&self, request: LiveSessionCommandRequest) -> Result<()> {
        self.app_service
            .join_live_session(request.topic.as_str(), request.session_id.as_str())
            .await
    }

    pub async fn leave_live_session(&self, request: LiveSessionCommandRequest) -> Result<()> {
        self.app_service
            .leave_live_session(request.topic.as_str(), request.session_id.as_str())
            .await
    }

    pub async fn list_game_rooms(
        &self,
        request: ListGameRoomsRequest,
    ) -> Result<Vec<GameRoomView>> {
        self.app_service
            .list_game_rooms_scoped(request.topic.as_str(), request.scope)
            .await
    }

    pub async fn create_game_room(&self, request: CreateGameRoomRequest) -> Result<String> {
        self.app_service
            .create_game_room_in_channel(
                request.topic.as_str(),
                request.channel_ref,
                CreateGameRoomInput {
                    title: request.title,
                    description: request.description,
                    participants: request.participants,
                },
            )
            .await
    }
}

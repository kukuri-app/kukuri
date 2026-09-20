use super::*;
impl AppService {
    pub(crate) async fn maybe_redeem_epoch_handoff_grants_for_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<bool> {
        let mut redeemed_any = false;
        loop {
            let Some(state) = self
                .joined_private_channel_state(topic_id, channel_id)
                .await
            else {
                return Ok(redeemed_any);
            };
            let local_author = self.current_author_pubkey();
            let replica = current_private_channel_replica_id(&state);
            let grant_doc = fetch_private_channel_epoch_handoff_grant_from_replica(
                self.docs_sync(),
                &replica,
                local_author.as_str(),
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            let grant_doc = if let Some(grant_doc) = grant_doc {
                Some(grant_doc)
            } else {
                let has_peers = self
                    .services
                    .transport
                    .peers()
                    .await
                    .is_ok_and(|peers| peers.peer_count > 0);
                if !has_peers {
                    return Ok(redeemed_any);
                }
                if let Err(error) = self.docs_sync().restart_replica_sync(&replica).await {
                    warn!(
                        topic = %topic_id,
                        channel_id = %channel_id,
                        epoch_id = %state.current_epoch_id,
                        error = %error,
                        "failed to restart private channel replica sync while polling epoch handoff"
                    );
                }
                fetch_private_channel_epoch_handoff_grant_from_replica(
                    self.docs_sync(),
                    &replica,
                    local_author.as_str(),
                    DocFetchPolicy::LocalThenRemote,
                )
                .await?
            };
            let Some(grant_doc) = grant_doc else {
                return Ok(redeemed_any);
            };
            let payload = match decrypt_private_channel_epoch_handoff_grant(self.keys(), &grant_doc)
            {
                Ok(payload) => payload,
                Err(error) => {
                    warn!(
                        topic = %topic_id,
                        channel_id = %channel_id,
                        epoch_id = %state.current_epoch_id,
                        error = %error,
                        "failed to decrypt private channel epoch handoff grant"
                    );
                    return Ok(redeemed_any);
                }
            };
            if payload.old_epoch_id != state.current_epoch_id
                || private_channel_epoch_capabilities(&state)
                    .iter()
                    .any(|known_epoch| known_epoch.epoch_id == payload.new_epoch_id)
            {
                return Ok(redeemed_any);
            }
            let next_replica =
                private_channel_epoch_replica_id(channel_id, payload.new_epoch_id.as_str());
            self.docs_sync()
                .register_private_replica_secret(
                    &next_replica,
                    payload.new_namespace_secret_hex.as_str(),
                )
                .await?;
            if let Err(error) = self
                .services
                .docs_sync
                .restart_replica_sync(&next_replica)
                .await
            {
                warn!(
                    topic = %topic_id,
                    channel_id = %channel_id,
                    epoch_id = %payload.new_epoch_id,
                    error = %error,
                    "failed to restart rotated private channel replica sync"
                );
            }
            let (metadata, policy, participants) = match wait_for_private_channel_epoch_snapshot(
                self.docs_sync(),
                &next_replica,
                PrivateChannelSnapshotWaitContext::EpochHandoff,
            )
            .await
            {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    warn!(
                        topic = %topic_id,
                        channel_id = %channel_id,
                        epoch_id = %payload.new_epoch_id,
                        error = %error,
                        "failed to load rotated private channel replica"
                    );
                    return Ok(redeemed_any);
                }
            };
            if policy.audience_kind != state.audience_kind
                || policy.epoch_id != payload.new_epoch_id
                || policy.previous_epoch_id.as_deref() != Some(payload.old_epoch_id.as_str())
            {
                warn!(
                    topic = %topic_id,
                    channel_id = %channel_id,
                    epoch_id = %payload.new_epoch_id,
                    audience_kind = ?policy.audience_kind,
                    "private channel epoch handoff payload does not match rotated policy"
                );
                return Ok(redeemed_any);
            }
            let local_pubkey = Pubkey::from(local_author.clone());
            if !participants.iter().any(|participant| {
                participant.participant_pubkey == local_pubkey
                    && participant.epoch_id == policy.epoch_id
                    && participant.left_at.is_none()
            }) {
                persist_private_channel_participant(
                    self.docs_sync(),
                    self.keys(),
                    &PrivateChannelParticipantDocV1 {
                        channel_id: metadata.channel_id.clone(),
                        topic_id: metadata.topic_id.clone(),
                        epoch_id: policy.epoch_id.clone(),
                        participant_pubkey: local_pubkey,
                        joined_at: Utc::now().timestamp_millis(),
                        is_owner: false,
                        join_mode: Some(PrivateChannelJoinMode::RotationRedeem),
                        sponsor_pubkey: Some(policy.owner_pubkey.clone()),
                        share_token_id: None,
                        left_at: None,
                    },
                    &next_replica,
                )
                .await?;
            }
            let next_state = merged_private_channel_state_from_epoch_join(
                Some(state.clone()),
                metadata.topic_id.as_str(),
                metadata.channel_id.clone(),
                metadata.label.as_str(),
                metadata.creator_pubkey.as_str(),
                policy.owner_pubkey.as_str(),
                state.joined_via_pubkey.as_deref(),
                policy.audience_kind.clone(),
                payload.new_epoch_id.as_str(),
                payload.new_namespace_secret_hex.as_str(),
            );
            self.register_joined_private_channel(next_state).await?;
            redeemed_any = true;
        }
    }
    pub(crate) async fn private_channel_diagnostics(
        &self,
        state: &JoinedPrivateChannelState,
    ) -> Result<PrivateChannelDiagnostics> {
        let replica = current_private_channel_replica_id(state);
        let policy = fetch_private_channel_policy_from_replica(
            self.docs_sync(),
            &replica,
            DocFetchPolicy::LocalOnly,
        )
        .await?;
        let sharing_state = policy
            .as_ref()
            .map(|policy| policy.sharing_state.clone())
            .unwrap_or(ChannelSharingState::Open);
        let participants = fetch_private_channel_participants_from_replica(
            self.docs_sync(),
            &replica,
            DocFetchPolicy::LocalOnly,
        )
        .await?;
        let participants =
            active_private_channel_participants(&participants, state.current_epoch_id.as_str());
        let participant_count = participants.len();
        let mut stale_participant_count = 0usize;
        if state.audience_kind == ChannelAudienceKind::FriendOnly
            && state.owner_pubkey == self.current_author_pubkey()
        {
            for participant in &participants {
                if participant.is_owner {
                    continue;
                }
                self.ensure_author_subscription(participant.participant_pubkey.as_str())
                    .await?;
                let relationship = self
                    .services
                    .projection_store
                    .get_author_relationship(
                        self.current_author_pubkey().as_str(),
                        participant.participant_pubkey.as_str(),
                    )
                    .await?;
                if relationship.as_ref().is_some_and(|value| !value.mutual) {
                    stale_participant_count += 1;
                }
            }
        }
        Ok(PrivateChannelDiagnostics {
            sharing_state,
            participant_count,
            stale_participant_count,
            rotation_required: state.audience_kind == ChannelAudienceKind::FriendOnly
                && stale_participant_count > 0,
            entry_dome_instance_id: policy.and_then(|policy| policy.entry_dome_instance_id),
        })
    }
    pub(crate) async fn joined_private_channel_view_for_state(
        &self,
        state: &JoinedPrivateChannelState,
    ) -> Result<JoinedPrivateChannelView> {
        let diagnostics = self.private_channel_diagnostics(state).await?;
        Ok(JoinedPrivateChannelView {
            topic_id: state.topic_id.clone(),
            channel_id: state.channel_id.as_str().to_string(),
            label: state.label.clone(),
            creator_pubkey: state.creator_pubkey.clone(),
            owner_pubkey: state.owner_pubkey.clone(),
            joined_via_pubkey: state.joined_via_pubkey.clone(),
            audience_kind: state.audience_kind.clone(),
            is_owner: state.owner_pubkey == self.current_author_pubkey(),
            current_epoch_id: state.current_epoch_id.clone(),
            archived_epoch_ids: state
                .archived_epochs
                .iter()
                .map(|epoch| epoch.epoch_id.clone())
                .collect(),
            sharing_state: diagnostics.sharing_state,
            rotation_required: diagnostics.rotation_required,
            participant_count: diagnostics.participant_count,
            stale_participant_count: diagnostics.stale_participant_count,
            entry_dome_instance_id: diagnostics.entry_dome_instance_id,
        })
    }
    /// テスト専用: 唯一の呼び出し元が cfg(test) の get_private_channel_capability。
    #[cfg(test)]
    pub(crate) async fn private_channel_capability_from_state(
        &self,
        state: &JoinedPrivateChannelState,
    ) -> Result<PrivateChannelCapability> {
        let diagnostics = self.private_channel_diagnostics(state).await?;
        Ok(PrivateChannelCapability {
            topic_id: state.topic_id.clone(),
            channel_id: state.channel_id.as_str().to_string(),
            label: state.label.clone(),
            creator_pubkey: state.creator_pubkey.clone(),
            owner_pubkey: state.owner_pubkey.clone(),
            joined_via_pubkey: state.joined_via_pubkey.clone(),
            audience_kind: state.audience_kind.clone(),
            current_epoch_id: state.current_epoch_id.clone(),
            current_epoch_secret_hex: state.current_epoch_secret_hex.clone(),
            archived_epochs: state.archived_epochs.clone(),
            rotation_required: diagnostics.rotation_required,
            participant_count: diagnostics.participant_count,
            stale_participant_count: diagnostics.stale_participant_count,
            namespace_secret_hex: state.current_epoch_secret_hex.clone(),
        })
    }
    pub(crate) async fn audience_label_for_storage(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> String {
        if channel_id == PUBLIC_CHANNEL_ID {
            return "Public".to_string();
        }
        self.joined_private_channels
            .lock()
            .await
            .get(joined_private_channel_key(topic_id, channel_id).as_str())
            .map(|channel| channel.label.clone())
            .unwrap_or_else(|| "Private channel".to_string())
    }
    pub(crate) async fn joined_private_channel_states_for_topic(
        &self,
        topic_id: &str,
    ) -> Vec<JoinedPrivateChannelState> {
        self.joined_private_channels
            .lock()
            .await
            .values()
            .filter(|state| state.topic_id == topic_id)
            .cloned()
            .collect()
    }
    pub(crate) async fn joined_private_channel_state(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Option<JoinedPrivateChannelState> {
        self.joined_private_channels
            .lock()
            .await
            .get(joined_private_channel_key(topic_id, channel_id).as_str())
            .cloned()
    }
    pub(crate) async fn ensure_private_channel_access(
        &self,
        topic_id: &str,
        channel_id: &ChannelId,
    ) -> Result<()> {
        if self
            .joined_private_channel_state(topic_id, channel_id.as_str())
            .await
            .is_none()
        {
            anyhow::bail!("private channel is not joined");
        }
        Ok(())
    }
    pub(crate) async fn maybe_auto_rotate_private_channel_for_owner(
        &self,
        topic_id: &str,
        channel_id: &ChannelId,
        action: PrivateChannelOwnerAction,
    ) -> Result<()> {
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id.as_str())
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        if state.owner_pubkey != self.current_author_pubkey() {
            return Ok(());
        }
        match state.audience_kind {
            ChannelAudienceKind::InviteOnly | ChannelAudienceKind::FriendPlus => {
                if matches!(action, PrivateChannelOwnerAction::Share) {
                    let _ = self
                        .rotate_private_channel(topic_id, channel_id.as_str())
                        .await?;
                }
            }
            ChannelAudienceKind::FriendOnly => {
                let diagnostics = self.private_channel_diagnostics(&state).await?;
                if diagnostics.rotation_required {
                    let _ = self
                        .rotate_private_channel(topic_id, channel_id.as_str())
                        .await?;
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn private_channel_state_for_owner_action(
        &self,
        topic_id: &str,
        channel_id: &ChannelId,
        action: PrivateChannelOwnerAction,
    ) -> Result<JoinedPrivateChannelState> {
        self.maybe_redeem_epoch_handoff_grants_for_channel(topic_id, channel_id.as_str())
            .await?;
        self.ensure_private_channel_access(topic_id, channel_id)
            .await?;
        self.ensure_private_channel_subscription(topic_id, channel_id.as_str())
            .await?;
        self.maybe_auto_rotate_private_channel_for_owner(topic_id, channel_id, action)
            .await?;
        self.maybe_redeem_epoch_handoff_grants_for_channel(topic_id, channel_id.as_str())
            .await?;
        self.ensure_private_channel_access(topic_id, channel_id)
            .await?;
        self.ensure_private_channel_subscription(topic_id, channel_id.as_str())
            .await?;
        let state = self
            .joined_private_channel_state(topic_id, channel_id.as_str())
            .await
            .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
        if private_channel_rotation_is_pending(self.docs_sync(), self.keys(), &state).await? {
            anyhow::bail!(
                "private channel epoch handoff is pending; wait for automatic redemption or use a fresh access token"
            );
        }
        Ok(state)
    }

    pub(crate) async fn private_channel_write_state(
        &self,
        topic_id: &str,
        channel_id: &ChannelId,
    ) -> Result<JoinedPrivateChannelState> {
        self.private_channel_state_for_owner_action(
            topic_id,
            channel_id,
            PrivateChannelOwnerAction::Write,
        )
        .await
    }

    /// registry 変異時の write-through 永続化。callback 未接続(テスト・harness・復元前)
    /// なら no-op。persist 同士を専用 guard で直列化し、スナップショットを guard 内で
    /// 取ることで「古いスナップショットが新しい書き込みを上書きする」lost-update を防ぐ
    /// (全量スナップショットのため、最後に走った persist が必ず最新の registry を反映する)。
    pub(crate) async fn persist_private_channel_capabilities_if_configured(&self) -> Result<()> {
        let Some(persist) = self.private_channel_capability_persist.get() else {
            return Ok(());
        };
        let _guard = self.private_channel_capability_persist_guard.lock().await;
        let snapshot = {
            let map = self.joined_private_channels.lock().await;
            map.values()
                .map(private_channel_capability_snapshot)
                .collect::<Vec<_>>()
        };
        persist(&snapshot)
    }

    pub(crate) async fn register_joined_private_channel(
        &self,
        state: JoinedPrivateChannelState,
    ) -> Result<()> {
        register_private_channel_replica_secrets(self.docs_sync(), &state).await?;
        self.joined_private_channels.lock().await.insert(
            joined_private_channel_key(state.topic_id.as_str(), state.channel_id.as_str()),
            state.clone(),
        );
        self.persist_private_channel_capabilities_if_configured()
            .await?;
        self.ensure_private_channel_subscription(
            state.topic_id.as_str(),
            state.channel_id.as_str(),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn remove_joined_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<JoinedPrivateChannelState>> {
        let removed = self
            .joined_private_channels
            .lock()
            .await
            .remove(joined_private_channel_key(topic_id, channel_id).as_str());
        if removed.is_some() {
            self.persist_private_channel_capabilities_if_configured()
                .await?;
        }
        let prefix = joined_private_channel_subscription_prefix(topic_id, channel_id);
        let keys = self
            .subscription_registry
            .private_channel_subscriptions
            .lock()
            .await
            .keys()
            .filter(|key| key.starts_with(prefix.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(handle) = self
                .subscription_registry
                .private_channel_subscriptions
                .lock()
                .await
                .remove(key.as_str())
            {
                handle.abort();
            }
        }
        let has_peers = self
            .services
            .transport
            .peers()
            .await
            .is_ok_and(|peers| peers.peer_count > 0);
        if has_peers {
            let hint_topic = private_channel_hint_topic(channel_id);
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                self.hint_transport().unsubscribe_hints(&hint_topic),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(
                        topic = %topic_id,
                        channel_id = %channel_id,
                        error = %error,
                        "failed to unsubscribe private channel hints after leave"
                    );
                }
                Err(_) => {
                    warn!(
                        topic = %topic_id,
                        channel_id = %channel_id,
                        "timed out unsubscribing private channel hints after leave"
                    );
                }
            }
        }
        Ok(removed)
    }

    pub(crate) async fn ensure_private_channel_subscription(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        if self.is_channel_gossip_disabled(topic_id, channel_id).await {
            return Ok(());
        }
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        self.spawn_private_channel_subscription(state).await
    }

    pub(crate) async fn ensure_joined_private_channel_subscriptions(
        &self,
        topic_id: &str,
    ) -> Result<()> {
        for state in self.joined_private_channel_states_for_topic(topic_id).await {
            self.ensure_private_channel_subscription(topic_id, state.channel_id.as_str())
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn restart_private_channel_subscription(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        let prefix = joined_private_channel_subscription_prefix(topic_id, channel_id);
        let keys = self
            .subscription_registry
            .private_channel_subscriptions
            .lock()
            .await
            .keys()
            .filter(|key| key.starts_with(prefix.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(handle) = self
                .subscription_registry
                .private_channel_subscriptions
                .lock()
                .await
                .remove(key.as_str())
            {
                handle.abort();
            }
        }
        self.hint_transport()
            .unsubscribe_hints(&private_channel_hint_topic(channel_id))
            .await?;
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            return Ok(());
        };
        self.spawn_private_channel_subscription(state).await
    }

    pub(crate) async fn spawn_private_channel_subscription(
        &self,
        state: JoinedPrivateChannelState,
    ) -> Result<()> {
        let docs_sync = Arc::clone(&self.services.docs_sync);
        for epoch in private_channel_epoch_capabilities(&state) {
            let replica = private_channel_replica_for_epoch(
                state.channel_id.as_str(),
                epoch.epoch_id.as_str(),
            );
            let key = joined_private_channel_subscription_key(
                state.topic_id.as_str(),
                state.channel_id.as_str(),
                &replica,
            );
            if self
                .subscription_registry
                .private_channel_subscriptions
                .lock()
                .await
                .contains_key(key.as_str())
            {
                continue;
            }
            docs_sync
                .register_private_replica_secret(&replica, epoch.namespace_secret_hex.as_str())
                .await?;
            self.spawn_subscription_task(
                state.topic_id.as_str(),
                Some(state.channel_id.clone()),
                replica,
                private_channel_hint_topic(state.channel_id.as_str()),
                Some(key),
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn spawn_subscription_task(
        &self,
        topic_id: &str,
        channel_id: Option<ChannelId>,
        replica: ReplicaId,
        hint_topic: TopicId,
        private_key: Option<String>,
    ) -> Result<()> {
        let services = self.services.clone();
        let metaverse_room_events = Arc::clone(&self.metaverse_room_events);
        let dome_host_heartbeats = Arc::clone(&self.dome_host_heartbeats);
        let last_sync = Arc::clone(&self.last_sync_ts);
        let notification_inserted = Arc::clone(&self.notification_inserted_notify);
        let public_topic_delivery = Arc::clone(&self.public_topic_delivery);
        let topic = topic_id.to_string();
        let storage_channel_id = channel_storage_id(channel_id.as_ref());
        let local_author_pubkey = self.current_author_pubkey();
        let subscription_key = private_key.clone().unwrap_or_else(|| topic_id.to_string());
        let generation = self
            .next_subscription_generation(subscription_key.as_str())
            .await;
        let is_public_topic = channel_id.is_none() && private_key.is_none();
        if is_public_topic {
            self.reset_public_topic_delivery_generation(topic_id, generation)
                .await;
        }
        services.docs_sync.open_replica(&replica).await?;
        let mut doc_stream = services.docs_sync.subscribe_replica(&replica).await?;
        let mut hint_stream = services.hint_transport.subscribe_hints(&hint_topic).await?;
        let replica_for_task = replica.clone();
        let hint_topic_for_task = hint_topic.clone();
        let handle = tokio::spawn(async move {
            let projection_store = &services.projection_store;
            let docs_sync = &services.docs_sync;
            let blob_service = &services.blob_service;
            let hint_transport = &services.hint_transport;
            let transport = &services.transport;
            let notification_baseline = match snapshot_object_notification_baseline(
                docs_sync.as_ref(),
                &replica_for_task,
                DocFetchPolicy::LocalOnly,
            )
            .await
            {
                Ok(baseline) => baseline,
                Err(error) => {
                    warn!(
                        topic = %topic,
                        replica = %replica_for_task.as_str(),
                        error = %error,
                        "failed to snapshot local notification baseline for subscription bootstrap"
                    );
                    NotificationDocEventBaseline::default()
                }
            };
            let mut recovery_tick = tokio::time::interval(std::time::Duration::from_secs(1));
            recovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            if let Err(error) = hydrate_subscription_state(
                &services,
                topic.as_str(),
                &replica_for_task,
                DocFetchPolicy::LocalOnly,
            )
            .await
            {
                warn!(
                    topic = %topic,
                    replica = %replica_for_task.as_str(),
                    error = %error,
                    "failed to hydrate local subscription cache during background bootstrap"
                );
            }
            let mut recovery_backoff = SubscriptionRecoveryBackoff::default();
            let mut recovery_probe_due_at = Utc::now()
                .timestamp_millis()
                .saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
            let mut hint_recovery_gate = HintRecoveryGate::default();
            loop {
                tokio::select! {
                    Some(event) = doc_stream.next() => {
                        if let Ok(event) = event {
                            let now = Utc::now().timestamp_millis();
                            let had_source_peer = event.source_peer.is_some();
                            if let Some(source_peer) = event.source_peer.as_deref() {
                                if let Err(error) = docs_sync.learn_peer(source_peer).await {
                                    warn!(
                                        topic = %topic,
                                        source_peer = %source_peer,
                                        error = %error,
                                        "failed to learn docs peer from docs sync event"
                                    );
                                }
                                if let Err(error) = blob_service.learn_peer(source_peer).await {
                                    warn!(
                                        topic = %topic,
                                        source_peer = %source_peer,
                                        error = %error,
                                        "failed to learn blob peer from docs sync event"
                                    );
                                }
                            }
                            match AppService::maybe_create_notification_for_remote_object_event(
                                projection_store.as_ref(),
                                docs_sync.as_ref(),
                                blob_service.as_ref(),
                                local_author_pubkey.as_str(),
                                topic.as_str(),
                                &notification_baseline,
                                &event,
                            ).await {
                                Ok(true) => {
                                    *last_sync.lock().await = Some(now);
                                    notification_inserted.notify_waiters();
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    warn!(
                                        topic = %topic,
                                        key = %event.key,
                                        error = %error,
                                        "failed to create notification from remote object event"
                                    );
                                }
                            }
                            let mut hydrated = match hydrate_subscription_doc_event(
                                &services,
                                topic.as_str(),
                                &replica_for_task,
                                event.key.as_str(),
                                event.docs_author.as_deref(),
                            ).await {
                                Ok(count) => count,
                                Err(error) => {
                                    warn!(
                                        topic = %topic,
                                        key = %event.key,
                                        error = %error,
                                        "failed to hydrate subscription from docs event"
                                    );
                                    0
                                }
                            };
                            if hydrated == 0 && !is_public_topic {
                                hydrated = match hydrate_subscription_state(
                                    &services,
                                    topic.as_str(),
                                    &replica_for_task,
                                    DocFetchPolicy::LocalThenRemote,
                                )
                                .await {
                                    Ok(count) => count,
                                    Err(error) => {
                                        warn!(
                                            topic = %topic,
                                            replica = %replica_for_task.as_str(),
                                            error = %error,
                                            "failed to hydrate subscription from docs-event recovery"
                                        );
                                        0
                                    }
                                };
                            }
                            if hydrated > 0 {
                                recovery_backoff.reset();
                                if is_public_topic && event.source_peer.is_some() {
                                    record_public_topic_docs_activity_if_current(
                                        &public_topic_delivery,
                                        topic.as_str(),
                                        generation,
                                        now,
                                    )
                                    .await;
                                    recovery_backoff.reset();
                                    recovery_probe_due_at =
                                        now.saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
                                }
                                *last_sync.lock().await = Some(now);
                            } else {
                                restart_replica_sync_with_backoff(
                                    docs_sync.as_ref(),
                                    topic.as_str(),
                                    &replica_for_task,
                                    &mut recovery_backoff,
                                )
                                .await;
                                if is_public_topic && had_source_peer {
                                    recovery_probe_due_at =
                                        now.saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
                                }
                            }
                        }
                    }
                    Some(event) = hint_stream.next() => {
                        if hint_targets_topic(&event.hint, topic.as_str()) {
                            if !event.source_peer.is_empty() {
                                let source_peer = event.source_peer.as_str();
                                if let Err(error) = docs_sync.learn_peer(source_peer).await {
                                    warn!(
                                        topic = %topic,
                                        source_peer = %source_peer,
                                        error = %error,
                                        "failed to learn docs peer from hint event"
                                    );
                                }
                                if let Err(error) = blob_service.learn_peer(source_peer).await {
                                    warn!(
                                        topic = %topic,
                                        source_peer = %source_peer,
                                        error = %error,
                                        "failed to learn blob peer from hint event"
                                    );
                                }
                            }
                            match &event.hint {
                                GossipHint::MetaverseRoomEvent { event: envelope, .. } => {
                                    let now = Utc::now().timestamp_millis();
                                    match parse_metaverse_room_event_envelope(
                                        envelope.as_ref().clone(),
                                        event.received_at,
                                        event.source_peer.clone(),
                                    ) {
                                        Ok(Some(view)) => {
                                            push_metaverse_room_event_buffer(
                                                &metaverse_room_events,
                                                view,
                                            )
                                            .await;
                                            *last_sync.lock().await = Some(now);
                                        }
                                        Ok(None) => {}
                                        Err(error) => {
                                            warn!(
                                                topic = %topic,
                                                error = %error,
                                                "failed to parse metaverse room event hint"
                                            );
                                        }
                                    }
                                }
                                GossipHint::DomeHostHeartbeat { instance_id, heartbeat, .. } => {
                                    let mut heartbeats = dome_host_heartbeats.lock().await;
                                    let replace = heartbeats
                                        .get(instance_id)
                                        .is_none_or(|current| {
                                            heartbeat.heartbeat.sequence > current.heartbeat.sequence
                                                || (heartbeat.heartbeat.sequence == current.heartbeat.sequence
                                                    && heartbeat.heartbeat.sent_at > current.heartbeat.sent_at)
                                        });
                                    if replace {
                                        heartbeats.insert(instance_id.clone(), heartbeat.as_ref().clone());
                                        *last_sync.lock().await = Some(Utc::now().timestamp_millis());
                                    }
                                }
                                GossipHint::LivePresence { session_id, author, ttl_ms, .. } => {
                                    let now = Utc::now().timestamp_millis();
                                    let _ = projection_store
                                        .upsert_live_presence(
                                            topic.as_str(),
                                            storage_channel_id.as_str(),
                                            session_id.as_str(),
                                            author.as_str(),
                                            now + i64::from(*ttl_ms),
                                            now,
                                        )
                                        .await;
                                    let _ = projection_store.clear_expired_live_presence(now).await;
                                    *last_sync.lock().await = Some(now);
                                }
                                _ => {
                                    let mut hydrated = match hydrate_subscription_hint(
                                        &services,
                                        topic.as_str(),
                                        &replica_for_task,
                                        &event.hint,
                                    )
                                    .await {
                                        Ok(count) => count,
                                        Err(error) => {
                                            warn!(
                                                topic = %topic,
                                                error = %error,
                                                "failed to hydrate subscription from hint"
                                            );
                                            0
                                        }
                                    };
                                    let now = Utc::now().timestamp_millis();
                                    // #1225: 個別反映が 0 件でも、hint ごとに全件走査しない。
                                    if hydrated == 0 && !hint_recovery_gate.allow(&event.hint, now) {
                                        continue;
                                    }
                                    if hydrated == 0 {
                                        hydrated = match hydrate_subscription_state(
                                            &services,
                                            topic.as_str(),
                                            &replica_for_task,
                                            DocFetchPolicy::LocalThenRemote,
                                        )
                                        .await {
                                            Ok(count) => count,
                                            Err(error) => {
                                                warn!(
                                                    topic = %topic,
                                                    error = %error,
                                                    "failed to hydrate subscription from docs-first recovery probe"
                                                );
                                                0
                                            }
                                        };
                                    }
                                    if hydrated > 0 {
                                        recovery_backoff.reset();
                                        if is_public_topic && !event.source_peer.is_empty() {
                                            record_public_topic_docs_activity_if_current(
                                                &public_topic_delivery,
                                                topic.as_str(),
                                                generation,
                                                now,
                                            )
                                            .await;
                                            recovery_backoff.reset();
                                            recovery_probe_due_at =
                                                now.saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
                                        }
                                        *last_sync.lock().await = Some(now);
                                    } else {
                                        restart_replica_sync_with_backoff(
                                            docs_sync.as_ref(),
                                            topic.as_str(),
                                            &replica_for_task,
                                            &mut recovery_backoff,
                                        )
                                        .await;
                                        recovery_probe_due_at =
                                            now.saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
                                    }
                                }
                            }
                        }
                    }
                    _ = recovery_tick.tick(), if is_public_topic => {
                        let now = Utc::now().timestamp_millis();
                        if recovery_probe_due_at > now {
                            continue;
                        }
                        let (has_live_topic_peer, has_configured_topic_peer, docs_assist_peer_count) =
                            recovery_probe_peer_state(
                                transport.as_ref(),
                                docs_sync.as_ref(),
                                topic.as_str(),
                            )
                            .await;
                        // #1225: 走査しない場合も次の期限を置く。置かないと毎秒 peer を照会し続ける。
                        if docs_assist_peer_count == 0
                            && (has_live_topic_peer || !has_configured_topic_peer)
                        {
                            recovery_probe_due_at =
                                now.saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
                            continue;
                        }
                        let hydrated = match hydrate_subscription_state(
                            &services,
                            topic.as_str(),
                            &replica_for_task,
                            DocFetchPolicy::LocalThenRemote,
                        )
                        .await {
                            Ok(count) => count,
                            Err(error) => {
                                warn!(
                                    topic = %topic,
                                    error = %error,
                                    "failed to hydrate subscription during periodic docs-first recovery"
                                );
                                0
                            }
                        };
                        if hydrated > 0 {
                            if docs_assist_peer_count > 0 {
                                record_public_topic_docs_activity_if_current(
                                    &public_topic_delivery,
                                    topic.as_str(),
                                    generation,
                                    now,
                                )
                                .await;
                            }
                            recovery_backoff.reset();
                            recovery_probe_due_at =
                                now.saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
                            *last_sync.lock().await = Some(now);
                        } else {
                            restart_replica_sync_with_backoff(
                                docs_sync.as_ref(),
                                topic.as_str(),
                                &replica_for_task,
                                &mut recovery_backoff,
                            )
                            .await;
                            // #1225: 変化が無い間は、全件走査の間隔も再 sync の backoff に合わせて伸ばす。
                            recovery_probe_due_at = recovery_backoff.next_probe_at(now);
                        }
                    }
                    else => {
                        let _ = hint_transport.unsubscribe_hints(&hint_topic_for_task).await;
                        break;
                    },
                }
            }
        });

        if let Some(private_key) = private_key {
            self.subscription_registry
                .private_channel_subscriptions
                .lock()
                .await
                .insert(private_key, handle);
        } else {
            self.subscription_registry
                .subscriptions
                .lock()
                .await
                .insert(topic_id.to_string(), handle);
        }
        Ok(())
    }
}

/// write-through 永続化用の state-only スナップショット。
/// `private_channel_capability_from_state` と異なり diagnostics(docs 読みを伴う
/// async/fallible 処理)に依存しない純関数。diagnostics 派生 3 フィールドは復元時に
/// 一切読まれない(capability_registry_snapshot テストで固定)ため既定値で永続化する。
/// JSON のフィールド名・形状(凍結境界)は不変。
pub(crate) fn private_channel_capability_snapshot(
    state: &JoinedPrivateChannelState,
) -> PrivateChannelCapability {
    PrivateChannelCapability {
        topic_id: state.topic_id.clone(),
        channel_id: state.channel_id.as_str().to_string(),
        label: state.label.clone(),
        creator_pubkey: state.creator_pubkey.clone(),
        owner_pubkey: state.owner_pubkey.clone(),
        joined_via_pubkey: state.joined_via_pubkey.clone(),
        audience_kind: state.audience_kind.clone(),
        current_epoch_id: state.current_epoch_id.clone(),
        current_epoch_secret_hex: state.current_epoch_secret_hex.clone(),
        archived_epochs: state.archived_epochs.clone(),
        rotation_required: false,
        participant_count: 0,
        stale_participant_count: 0,
        namespace_secret_hex: state.current_epoch_secret_hex.clone(),
    }
}

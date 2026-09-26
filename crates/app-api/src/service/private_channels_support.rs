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
            let Some(grant_doc) = self.own_epoch_handoff_grant(&state).await? else {
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
            let snapshot = match self
                .load_private_epoch_snapshot(
                    topic_id,
                    channel_id,
                    &next_replica,
                    payload.new_namespace_secret_hex.as_str(),
                    &[state.owner_pubkey.as_str()],
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
            let PrivateEpochSnapshot {
                metadata,
                policy,
                local_participant,
            } = snapshot;
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
            if local_participant.is_none_or(|participant| participant.left_at.is_some()) {
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
        if !self
            .joined_private_channels
            .lock()
            .await
            .contains_key(&joined_private_channel_key(topic_id, channel_id.as_str()))
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
        self.maybe_auto_rotate_private_channel_for_owner(topic_id, channel_id, action)
            .await?;
        self.maybe_redeem_epoch_handoff_grants_for_channel(topic_id, channel_id.as_str())
            .await?;
        self.ensure_private_channel_access(topic_id, channel_id)
            .await?;
        let state = self
            .joined_private_channel_state(topic_id, channel_id.as_str())
            .await
            .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
        if self.private_channel_rotation_is_pending(&state).await? {
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

    /// 参加を登録する。新しい参加は参加の holder で channel key を取る。上限なら何も保存しない(#1221 R2-C)。
    pub(crate) async fn register_joined_private_channel(
        &self,
        state: JoinedPrivateChannelState,
    ) -> Result<()> {
        self.hold_joined_private_channel(state.topic_id.as_str(), state.channel_id.as_str())
            .await?;
        self.register_joined_private_channel_state(state).await
    }

    /// 参加の holder で channel key を取る。既に持っていれば何もしない。戻り値は今回取ったか。
    pub(crate) async fn hold_joined_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<bool> {
        let holder = private_channel_holder(topic_id, channel_id);
        let key = ScopeKey::Channel(topic_id.to_string(), channel_id.to_string());
        if self
            .subscription_registry
            .scope_leases
            .lock()
            .await
            .holds(&holder, &key)
        {
            return Ok(false);
        }
        self.set_scope_holder(&holder, [key]).await?;
        Ok(true)
    }

    /// 起動時の復元。上限を超えた channel は購読せずに参加状態だけを戻す(起動を失敗させない)。
    pub(crate) async fn restore_joined_private_channel(
        &self,
        state: JoinedPrivateChannelState,
    ) -> Result<()> {
        if let Err(error) = self
            .hold_joined_private_channel(state.topic_id.as_str(), state.channel_id.as_str())
            .await
        {
            if error.downcast_ref::<ScopeLimitReached>().is_none() {
                return Err(error);
            }
            warn!(
                topic = %state.topic_id,
                channel_id = %state.channel_id.as_str(),
                "private channel is not subscribed because the active scopes are full"
            );
        }
        self.register_joined_private_channel_state(state).await
    }

    pub(crate) async fn register_joined_private_channel_state(
        &self,
        mut state: JoinedPrivateChannelState,
    ) -> Result<()> {
        register_private_channel_replica_secrets(self.docs_sync(), &state).await?;
        let _save_access = self.services.content_save_access.lock().await;
        let key = joined_private_channel_key(state.topic_id.as_str(), state.channel_id.as_str());
        let mut joined = self.joined_private_channels.lock().await;
        if let Some(previous) = joined.get(&key) {
            state.generation = previous.generation;
        }
        let changed = joined.get(&key).is_none_or(|previous| *previous != state);
        if changed {
            state.generation = self
                .services
                .content_scope_generation
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .saturating_add(1);
        }
        joined.insert(key, state.clone());
        drop(joined);
        if changed {
            self.services
                .content_scope_changes
                .send_replace(state.generation);
        }
        drop(_save_access);
        self.persist_private_channel_capabilities_if_configured()
            .await?;
        // 現 epoch の replica だけを購読する。epoch が変わったら task を作り直す。
        if changed {
            self.restart_scope_subscription(&ScopeKey::Channel(
                state.topic_id.clone(),
                state.channel_id.as_str().to_string(),
            ))
            .await;
        }
        Ok(())
    }

    pub(crate) async fn remove_joined_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<JoinedPrivateChannelState>> {
        let _display_access = self.services.content_save_access.lock().await;
        let removed = self
            .joined_private_channels
            .lock()
            .await
            .remove(joined_private_channel_key(topic_id, channel_id).as_str());
        if removed.is_some() {
            let generation = self
                .services
                .content_scope_generation
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .saturating_add(1);
            self.services.content_scope_changes.send_replace(generation);
        }
        if let Some(state) = &removed {
            let replicas = private_channel_epoch_capabilities(state)
                .iter()
                .map(|epoch| private_channel_replica_for_epoch(channel_id, epoch.epoch_id.as_str()))
                .collect::<Vec<_>>();
            self.services
                .session_projections
                .remove_replicas(&replicas)
                .await;
        }
        if removed.is_some() {
            self.persist_private_channel_capabilities_if_configured()
                .await?;
        }
        self.release_scope_holder(&private_channel_holder(topic_id, channel_id))
            .await;
        // 列が channel を開いたままでも、参加していない channel は購読しない。
        self.restart_scope_subscription(&ScopeKey::Channel(
            topic_id.to_string(),
            channel_id.to_string(),
        ))
        .await;
        Ok(removed)
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

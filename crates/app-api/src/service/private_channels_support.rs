use super::*;
use kukuri_core::{AccountSyncItem, AccountSyncItemKey, ChannelRotationRequestV1};

/// 担当でない端末が、担当へ依頼した鍵更新の新しい世代を待つ上限（#1219 AC-3。2026-10-03 ユーザー判断）。
const PRIVATE_CHANNEL_ROTATION_REQUEST_WAIT: std::time::Duration =
    std::time::Duration::from_secs(15);

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
            // 止めるのは、grant の旧世代が手元の現在の世代でないときだけ。新しい世代の鍵の行が本人の別の端末から
            // 先に届いていても、同じ redeem で現在の世代を進める(ADR 0061 §9)。
            if payload.old_epoch_id != state.current_epoch_id {
                return Ok(redeemed_any);
            }
            let next_replica =
                private_channel_epoch_replica_id(channel_id, payload.new_epoch_id.as_str());
            // 検証の間は、新しい世代の秘密を一時の登録で引く。成否によらず外す(成功のときは行の commit の後)。
            self.docs_sync()
                .register_private_replica_secret(
                    &next_replica,
                    payload.new_namespace_secret_hex.as_str(),
                )
                .await?;
            let redeemed = self
                .redeem_epoch_handoff(&state, &payload, &next_replica)
                .await;
            self.docs_sync()
                .remove_private_replica_secret(&next_replica)
                .await?;
            if !redeemed? {
                return Ok(redeemed_any);
            }
            redeemed_any = true;
        }
    }
    /// 検証を通った handoff を、鍵の行と現在の世代へ 1 transaction で書き、その後に新しい世代の参加の記録を書く。
    /// 書いたら true。
    async fn redeem_epoch_handoff(
        &self,
        state: &JoinedPrivateChannelState,
        payload: &PrivateChannelEpochHandoffGrantPayloadV1,
        next_replica: &ReplicaId,
    ) -> Result<bool> {
        let topic_id = state.topic_id.as_str();
        let channel_id = state.channel_id.as_str();
        let snapshot = match self
            .load_private_epoch_snapshot(
                topic_id,
                channel_id,
                next_replica,
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
                return Ok(false);
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
            return Ok(false);
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
        let Some(membership_at) = self
            .commit_joined_private_channel_state(
                next_state,
                ChannelCommit::Advance {
                    from_epoch: payload.old_epoch_id.as_str(),
                },
            )
            .await?
        else {
            return Ok(false);
        };
        if local_participant.is_none_or(|participant| participant.left_at.is_some()) {
            self.record_private_channel_participant(
                &PrivateChannelParticipantDocV1 {
                    channel_id: metadata.channel_id.clone(),
                    topic_id: metadata.topic_id.clone(),
                    epoch_id: policy.epoch_id.clone(),
                    participant_pubkey: Pubkey::from(self.current_author_pubkey()),
                    joined_at: membership_at,
                    is_owner: false,
                    join_mode: Some(PrivateChannelJoinMode::RotationRedeem),
                    sponsor_pubkey: Some(policy.owner_pubkey.clone()),
                    share_token_id: None,
                    left_at: None,
                },
                policy.owner_pubkey.as_str(),
                payload.new_namespace_secret_hex.as_str(),
                next_replica,
            )
            .await?;
        }
        Ok(true)
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
        // #1221 R5-H: 参加者数は参加者の表(owner は account 経路で届いた参加・退出)の現 epoch の行で数える。
        // 参加・退出 record は owner にだけ届くため、人数は owner の端末だけが返す(2026-09-27 ユーザー決定)。
        // #1219 AC-2: 数と資格喪失(owner と mutual でない)の数は、store が行と follow の edge の書込みで保つ 1 行を
        // 読む(参加者の数に比例しない)。
        let is_owner = state.owner_pubkey == self.current_author_pubkey();
        let (participant_count, stale) = if is_owner {
            self.services
                .projection_store
                .private_channel_participant_counts(
                    state.channel_id.as_str(),
                    &state.current_epoch_id,
                )
                .await?
        } else {
            (0, 0)
        };
        let participant_count = is_owner.then_some(participant_count);
        let stale_participant_count = if state.audience_kind == ChannelAudienceKind::FriendOnly {
            stale
        } else {
            0
        };
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
            archived_epoch_ids: self
                .archived_private_channel_epochs(state, PRIVATE_CHANNEL_EPOCH_WINDOW)
                .await?
                .into_iter()
                .map(|(epoch_id, _)| epoch_id)
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
            archived_epochs: self
                .archived_private_channel_epochs(state, PRIVATE_CHANNEL_EPOCH_WINDOW)
                .await?
                .into_iter()
                .rev()
                .map(
                    |(epoch_id, namespace_secret_hex)| PrivateChannelEpochCapability {
                        epoch_id,
                        namespace_secret_hex,
                    },
                )
                .collect(),
            rotation_required: diagnostics.rotation_required,
            participant_count: diagnostics.participant_count.unwrap_or_default(),
            stale_participant_count: diagnostics.stale_participant_count,
            namespace_secret_hex: state.current_epoch_secret_hex.clone(),
            controller: Some(state.controller.clone()),
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
        self.joined_private_channel_state(topic_id, channel_id)
            .await
            .map(|channel| channel.label)
            .unwrap_or_else(|| "Private channel".to_string())
    }
    /// 現在の世代より前に始まった世代を、新しい順に `limit` 件まで(本人の別の端末から届いて、まだ現在の世代に
    /// なっていない新しい世代は含めない)。
    pub(crate) async fn archived_private_channel_epochs(
        &self,
        state: &JoinedPrivateChannelState,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        // `legacy`(開始時刻が最小)の世代より前の世代は無い。
        let current_at = epoch_started_at(&state.current_epoch_id);
        if current_at == i64::MIN {
            return Ok(Vec::new());
        }
        self.services
            .private_channel_epochs(
                state.channel_id.as_str(),
                kukuri_store::PrivateChannelEpochRange::AtOrBefore(current_at - 1),
                limit,
            )
            .await
    }
    /// この端末の ID(iroh endpoint ID)。鍵更新の担当端末の判定に使う(#1219 W6)。
    pub(crate) async fn local_device_id(&self) -> Result<String> {
        Ok(self.services.transport.discovery().await?.local_endpoint_id)
    }
    /// この端末を最初の担当(世代 1)にする記録。
    pub(crate) async fn first_controller(&self) -> Result<PrivateChannelController> {
        Ok(PrivateChannelController {
            device_id: self.local_device_id().await?,
            generation: 1,
            transfer_to: None,
        })
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
        let required = match state.audience_kind {
            ChannelAudienceKind::InviteOnly | ChannelAudienceKind::FriendPlus => {
                matches!(action, PrivateChannelOwnerAction::Share)
            }
            ChannelAudienceKind::FriendOnly => {
                self.private_channel_diagnostics(&state)
                    .await?
                    .rotation_required
            }
        };
        if required {
            self.rotate_or_request_private_channel(topic_id, &state)
                .await?;
        }
        Ok(())
    }

    /// 共有・投稿の前の鍵更新。担当でなければ、担当への依頼を account 同期へ書き、担当が本人の端末の候補にいれば
    /// 新しい世代が届くまで待つ。候補にいない・期限までに届かなければ保留を返す（依頼は残り、担当が取得したときに
    /// 処理する。#1219 AC-3、ADR 0018 §8）。
    async fn rotate_or_request_private_channel(
        &self,
        topic_id: &str,
        state: &JoinedPrivateChannelState,
    ) -> Result<()> {
        let channel_id = state.channel_id.as_str();
        let pending = match self.rotate_private_channel(topic_id, channel_id).await {
            Err(error) if error.is::<PrivateChannelControllerPending>() => error,
            result => return result.map(|_| ()),
        };
        self.publish_account_sync_item(AccountSyncItem::edit(
            AccountSyncItemKey::ChannelRotationRequest {
                channel_id: state.channel_id.clone(),
            },
            Utc::now().timestamp_millis(),
            Some(serde_json::to_value(ChannelRotationRequestV1 {
                from_epoch_id: state.current_epoch_id.clone(),
            })?),
        ))
        .await?;
        let candidates = self
            .services
            .hint_transport
            .topic_read_candidates(self.services.keys.derive_account_sync().hint_topic())
            .await
            .unwrap_or_default();
        if !state.controller.as_ref().is_some_and(|controller| {
            controller.transfer_to.is_none()
                && candidates
                    .iter()
                    .any(|peer| peer.endpoint_id == controller.device_id)
        }) {
            return Err(pending);
        }
        let advanced = n0_future::time::timeout(PRIVATE_CHANNEL_ROTATION_REQUEST_WAIT, async {
            while self
                .joined_private_channel_state(topic_id, channel_id)
                .await
                .is_some_and(|current| current.current_epoch_id == state.current_epoch_id)
            {
                n0_future::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await;
        advanced.map_err(|_| pending)
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

    /// 参加を登録する(参加の版を時刻 `joined_at` で新しくする)。新しい参加は参加の holder で channel key を取る。
    /// 上限なら何も保存しない(#1221 R2-C)。
    pub(crate) async fn register_joined_private_channel(
        &self,
        state: JoinedPrivateChannelState,
        joined_at: i64,
    ) -> Result<()> {
        self.hold_joined_private_channel(state.topic_id.as_str(), state.channel_id.as_str())
            .await?;
        self.commit_joined_private_channel_state(state, ChannelCommit::Join(joined_at))
            .await
            .map(|_| ())
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

    /// 旧 registry から移した channel の lease を取る。上限を超えた channel は購読しない(起動を失敗させない)。
    /// 既に lease があれば、移した行をメモリへ読み直す。
    pub(crate) async fn restore_joined_private_channel(&self, topic_id: &str, channel_id: &str) {
        match self.hold_joined_private_channel(topic_id, channel_id).await {
            Ok(true) => {}
            Ok(false) => {
                self.restart_scope_subscription(&ScopeKey::Channel(
                    topic_id.to_string(),
                    channel_id.to_string(),
                ))
                .await;
            }
            Err(error) => warn!(
                topic = %topic_id,
                channel_id = %channel_id,
                %error,
                "private channel is not subscribed because the active scopes are full"
            ),
        }
    }

    /// 参加状態を行へ書き、メモリと購読を合わせる。
    /// - `ChannelCommit::Join(t)`: 参加・再参加・取り込み直し。参加の版を時刻 `t` で新しくする。
    /// - `ChannelCommit::Advance`: 世代を進める。行が参加中でその世代が現在の世代のときだけ書く(照合は書込みと
    ///   同じ排他の中。退会と並行した世代の追加で参加に戻さない。ADR 0061 §9)。
    ///
    /// 書いたら参加の版の時刻(owner への参加の記録の時刻)。メモリは lease の task が行から読む(lease の無い channel
    /// はメモリへ載せない)。
    pub(crate) async fn commit_joined_private_channel_state(
        &self,
        state: JoinedPrivateChannelState,
        commit: ChannelCommit<'_>,
    ) -> Result<Option<i64>> {
        self.install_private_epoch_secrets().await?;
        let key = joined_private_channel_key(state.topic_id.as_str(), state.channel_id.as_str());
        let ((membership_at, writes), changed) = {
            let _save_access = self.services.content_save_access.lock().await;
            let persisted = match commit {
                ChannelCommit::Join(joined_at) => {
                    self.persist_private_channel(&state, joined_at, &[], true)
                        .await?
                }
                ChannelCommit::Advance { from_epoch } => {
                    if !self
                        .services
                        .projection_store
                        .get_private_channel(&key)
                        .await?
                        .is_some_and(|row| row.joined && row.current_epoch_id == from_epoch)
                    {
                        return Ok(None);
                    }
                    self.persist_private_channel(&state, Utc::now().timestamp_millis(), &[], false)
                        .await?
                }
            };
            let changed = self
                .joined_private_channels
                .lock()
                .await
                .get(&key)
                .is_none_or(|current| !same_private_channel_state(current, &state));
            (persisted, changed)
        };
        for item in &writes {
            self.write_account_sync_item(item).await?;
        }
        if changed {
            // 現 epoch の replica だけを購読する。epoch が変わったら task を作り直す。
            self.restart_scope_subscription(&ScopeKey::Channel(
                state.topic_id.clone(),
                state.channel_id.as_str().to_string(),
            ))
            .await;
        }
        Ok(Some(membership_at))
    }

    /// 退会。参加の行を tombstone にしてメモリから外し、鍵の行を消して lease を外す。
    pub(crate) async fn remove_joined_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<JoinedPrivateChannelState>> {
        let removed = self
            .joined_private_channel_state(topic_id, channel_id)
            .await;
        if let Some(state) = &removed {
            self.tombstone_private_channel(state, None).await?;
        }
        self.forget_private_channel_keys(topic_id, channel_id)
            .await?;
        Ok(removed)
    }
}

/// 参加状態の書き込みの種類(ADR 0061 §9)。
pub(crate) enum ChannelCommit<'a> {
    /// 参加・再参加・取り込み直し。参加の版をこの時刻で新しくする。
    Join(i64),
    /// 世代を進める。行が参加中で、現在の世代が `from_epoch` のときだけ書く。
    Advance { from_epoch: &'a str },
}

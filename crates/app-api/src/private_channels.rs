use crate::service::*;
use DocFetchPolicy::LocalThenRemote;
use kukuri_core::{DomeInstanceStatusV1, SpatialContextV1};
impl AppService {
    pub async fn create_private_channel(
        &self,
        input: CreatePrivateChannelInput,
    ) -> Result<JoinedPrivateChannelView> {
        let label = input.label.trim();
        if label.is_empty() {
            anyhow::bail!("private channel label is required");
        }
        let now = Utc::now().timestamp_millis();
        let owner_pubkey = self.current_author_pubkey();
        let channel_id = ChannelId::new(format!(
            "channel-{}-{}",
            now,
            short_id_suffix(owner_pubkey.as_str())
        ));
        let current_epoch_id =
            initial_private_channel_epoch_id(&input.audience_kind, now, owner_pubkey.as_str());
        let current_epoch_secret_hex = generate_keys().export_secret_hex();
        let state = JoinedPrivateChannelState {
            generation: 0,
            topic_id: input.topic_id.as_str().to_string(),
            channel_id: channel_id.clone(),
            label: label.to_string(),
            creator_pubkey: owner_pubkey.clone(),
            owner_pubkey: owner_pubkey.clone(),
            joined_via_pubkey: None,
            audience_kind: input.audience_kind.clone(),
            current_epoch_id: current_epoch_id.clone(),
            current_epoch_secret_hex: current_epoch_secret_hex.clone(),
            archived_epochs: Vec::new(),
        };
        self.register_joined_private_channel(state.clone()).await?;
        let metadata = PrivateChannelMetadataDocV1 {
            channel_id: channel_id.clone(),
            topic_id: input.topic_id.clone(),
            label: label.to_string(),
            creator_pubkey: Pubkey::from(state.creator_pubkey.clone()),
            created_at: now,
            audience_kind: input.audience_kind.clone(),
            owner_pubkey: Pubkey::from(owner_pubkey.clone()),
        };
        persist_private_channel_metadata(
            self.docs_sync(),
            &current_private_channel_replica_id(&state),
            &metadata,
        )
        .await?;
        persist_private_channel_policy(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelPolicyDocV1 {
                channel_id: channel_id.clone(),
                topic_id: input.topic_id.clone(),
                audience_kind: input.audience_kind.clone(),
                owner_pubkey: Pubkey::from(owner_pubkey.clone()),
                epoch_id: current_epoch_id,
                sharing_state: ChannelSharingState::Open,
                rotated_at: None,
                previous_epoch_id: None,
                entry_dome_instance_id: None,
            },
            &current_private_channel_replica_id(&state),
        )
        .await?;
        persist_private_channel_participant(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelParticipantDocV1 {
                channel_id,
                topic_id: input.topic_id,
                epoch_id: state.current_epoch_id.clone(),
                participant_pubkey: Pubkey::from(owner_pubkey),
                joined_at: now,
                is_owner: true,
                join_mode: Some(PrivateChannelJoinMode::OwnerSeed),
                sponsor_pubkey: None,
                share_token_id: None,
                left_at: None,
            },
            &current_private_channel_replica_id(&state),
        )
        .await?;
        self.joined_private_channel_view_for_state(&state).await
    }
    pub async fn export_private_channel_invite(
        &self,
        topic_id: &str,
        channel_id: &str,
        expires_at: Option<i64>,
    ) -> Result<String> {
        let state = self
            .private_channel_state_for_owner_action(
                topic_id,
                &ChannelId::new(channel_id),
                PrivateChannelOwnerAction::Share,
            )
            .await?;
        if state.audience_kind != ChannelAudienceKind::InviteOnly {
            anyhow::bail!(
                "private channel invite export is only available for invite-only channels"
            );
        }
        build_private_channel_invite_token(
            self.keys(),
            PrivateChannelInviteTokenParams {
                topic: &TopicId::new(topic_id),
                channel_id: &state.channel_id,
                channel_label: state.label.as_str(),
                owner_pubkey: &Pubkey::from(state.owner_pubkey.clone()),
                epoch_id: state.current_epoch_id.as_str(),
                namespace_secret_hex: state.current_epoch_secret_hex.as_str(),
                expires_at,
            },
        )
    }
    /// private channel import 3 系統(招待 / friend-only 許可 / friend-plus 共有)の共通仕様。
    ///
    /// フローは同型(トークン検証 → 前提確認 → replica 秘密登録 → snapshot 検証 →
    /// 参加登録 → 状態併合。失敗時は秘密を掃除)で、差分だけをこの仕様に載せる(WP-H5 PR3)。
    /// **エラー文言は統合前と 1 字も変えないこと**(テストと利用者向け表示が固定している)。
    async fn import_private_channel_by_spec(&self, spec: PrivateChannelImportSpec) -> Result<()> {
        if let Some(expires_at) = spec.expires_at
            && expires_at < Utc::now().timestamp_millis()
        {
            return Err(PrivateChannelImportError::Expired { kind: spec.kind }.into());
        }
        if let Some(peer_pubkey) = spec.mutual_with.as_ref() {
            // #1221 R4-D: 関係は手元の edge から読むときに求める。相手の follow は購読の追いつきと follow の offer で届く。
            let relationship = self
                .services
                .projection_store
                .get_author_relationship(
                    self.current_author_pubkey().as_str(),
                    peer_pubkey.as_str(),
                )
                .await?;
            if !relationship.as_ref().is_some_and(|value| value.mutual) {
                return Err(PrivateChannelImportError::MutualRelationshipRequired {
                    kind: spec.kind,
                }
                .into());
            }
        }
        // 新しい参加は、何かを保存する前に参加の holder で channel key を取る。上限なら参加しない(#1221 R2-C)。
        let newly_held = self
            .hold_joined_private_channel(spec.topic_id.as_str(), spec.channel_id.as_str())
            .await?;
        let replica = if spec.use_legacy_aware_replica {
            private_channel_replica_for_epoch(spec.channel_id.as_str(), spec.epoch_id.as_str())
        } else {
            private_channel_epoch_replica_id(spec.channel_id.as_str(), spec.epoch_id.as_str())
        };
        self.docs_sync()
            .register_private_replica_secret(&replica, spec.namespace_secret_hex.as_str())
            .await?;
        let import_result = async {
            // #1221 R5-C: token の発行者と owner の端末から、参加に要る制御 record だけを key 指定で読む。
            let PrivateEpochSnapshot {
                metadata,
                policy,
                local_participant,
            } = self
                .load_private_epoch_snapshot(
                    spec.topic_id.as_str(),
                    spec.channel_id.as_str(),
                    &replica,
                    spec.namespace_secret_hex.as_str(),
                    &[spec.joined_via_pubkey.as_str(), spec.owner_pubkey.as_str()],
                    PrivateChannelSnapshotWaitContext::Import(spec.kind),
                )
                .await?;
            if policy.audience_kind != spec.audience {
                return Err(PrivateChannelImportError::AudienceMismatch { kind: spec.kind }.into());
            }
            if policy.sharing_state != ChannelSharingState::Open {
                return Err(PrivateChannelImportError::SharingClosed { kind: spec.kind }.into());
            }
            if policy.epoch_id != spec.epoch_id {
                return Err(PrivateChannelImportError::EpochMismatch { kind: spec.kind }.into());
            }
            let local_pubkey = Pubkey::from(self.current_author_pubkey());
            let already_participant =
                local_participant.is_some_and(|participant| participant.left_at.is_none());
            if !(spec.skip_persist_if_participant && already_participant) {
                let sponsor_pubkey = match &spec.sponsor {
                    ImportSponsor::Pubkey(value) => value.clone(),
                    ImportSponsor::PolicyOwner => policy.owner_pubkey.clone(),
                };
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
                        join_mode: Some(spec.join_mode),
                        sponsor_pubkey: Some(sponsor_pubkey),
                        share_token_id: spec.share_token_id.clone(),
                        left_at: None,
                    },
                    &replica,
                )
                .await?;
            }
            let next_state = merged_private_channel_state_from_epoch_join(
                self.joined_private_channel_state(spec.topic_id.as_str(), spec.channel_id.as_str())
                    .await,
                spec.topic_id.as_str(),
                spec.channel_id.clone(),
                spec.channel_label.as_str(),
                metadata.creator_pubkey.as_str(),
                spec.owner_pubkey.as_str(),
                Some(spec.joined_via_pubkey.as_str()),
                spec.audience,
                spec.epoch_id.as_str(),
                spec.namespace_secret_hex.as_str(),
            );
            self.register_joined_private_channel(next_state).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if import_result.is_err() {
            let _ = self
                .services
                .docs_sync
                .remove_private_replica_secret(&replica)
                .await;
            if newly_held {
                self.release_scope_holder(&private_channel_holder(
                    spec.topic_id.as_str(),
                    spec.channel_id.as_str(),
                ))
                .await;
            }
        }
        import_result
    }
    pub async fn import_private_channel_invite(
        &self,
        token: &str,
    ) -> Result<PrivateChannelInvitePreview> {
        let preview = parse_private_channel_invite_token(token)?;
        self.import_private_channel_by_spec(PrivateChannelImportSpec {
            topic_id: preview.topic_id.as_str().to_string(),
            channel_id: preview.channel_id.clone(),
            channel_label: preview.channel_label.clone(),
            owner_pubkey: preview.owner_pubkey.as_str().to_string(),
            epoch_id: preview.epoch_id.clone(),
            namespace_secret_hex: preview.namespace_secret_hex.clone(),
            expires_at: preview.expires_at,
            kind: PrivateChannelImportKind::InviteOnly,
            mutual_with: None,
            // 招待だけは epoch なし時代の channel を legacy replica で読む(互換パス)。
            use_legacy_aware_replica: true,
            audience: ChannelAudienceKind::InviteOnly,
            join_mode: PrivateChannelJoinMode::InviteToken,
            sponsor: ImportSponsor::Pubkey(preview.inviter_pubkey.clone()),
            share_token_id: None,
            skip_persist_if_participant: true,
            joined_via_pubkey: preview.inviter_pubkey.as_str().to_string(),
        })
        .await?;
        Ok(preview)
    }
    pub async fn export_channel_access_token(
        &self,
        topic_id: &str,
        channel_id: &str,
        expires_at: Option<i64>,
    ) -> Result<ChannelAccessTokenExport> {
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        let (kind, token) = match state.audience_kind {
            ChannelAudienceKind::InviteOnly => (
                ChannelAccessTokenKind::Invite,
                self.export_private_channel_invite(topic_id, channel_id, expires_at)
                    .await?,
            ),
            ChannelAudienceKind::FriendOnly => (
                ChannelAccessTokenKind::Grant,
                self.export_friend_only_grant(topic_id, channel_id, expires_at)
                    .await?,
            ),
            ChannelAudienceKind::FriendPlus => (
                ChannelAccessTokenKind::Share,
                self.export_friend_plus_share(topic_id, channel_id, expires_at)
                    .await?,
            ),
        };
        Ok(ChannelAccessTokenExport { kind, token })
    }
    pub async fn import_channel_access_token(
        &self,
        token: &str,
    ) -> Result<ChannelAccessTokenPreview> {
        if let Ok(preview) = self.preview_channel_access_token(token).await {
            match preview.kind {
                ChannelAccessTokenKind::Invite => {
                    let preview = self.import_private_channel_invite(token).await?;
                    return Ok(ChannelAccessTokenPreview {
                        kind: ChannelAccessTokenKind::Invite,
                        topic_id: preview.topic_id.as_str().to_string(),
                        channel_id: preview.channel_id.as_str().to_string(),
                        channel_label: preview.channel_label,
                        owner_pubkey: preview.owner_pubkey.as_str().to_string(),
                        inviter_pubkey: Some(preview.inviter_pubkey.as_str().to_string()),
                        sponsor_pubkey: None,
                        epoch_id: preview.epoch_id,
                    });
                }
                ChannelAccessTokenKind::Grant => {
                    let preview = self.import_friend_only_grant(token).await?;
                    return Ok(ChannelAccessTokenPreview {
                        kind: ChannelAccessTokenKind::Grant,
                        topic_id: preview.topic_id.as_str().to_string(),
                        channel_id: preview.channel_id.as_str().to_string(),
                        channel_label: preview.channel_label,
                        owner_pubkey: preview.owner_pubkey.as_str().to_string(),
                        inviter_pubkey: None,
                        sponsor_pubkey: Some(preview.owner_pubkey.as_str().to_string()),
                        epoch_id: preview.epoch_id,
                    });
                }
                ChannelAccessTokenKind::Share => {
                    let preview = self.import_friend_plus_share(token).await?;
                    return Ok(ChannelAccessTokenPreview {
                        kind: ChannelAccessTokenKind::Share,
                        topic_id: preview.topic_id.as_str().to_string(),
                        channel_id: preview.channel_id.as_str().to_string(),
                        channel_label: preview.channel_label,
                        owner_pubkey: preview.owner_pubkey.as_str().to_string(),
                        inviter_pubkey: None,
                        sponsor_pubkey: Some(preview.sponsor_pubkey.as_str().to_string()),
                        epoch_id: preview.epoch_id,
                    });
                }
            }
        }
        anyhow::bail!("unrecognized private channel access token")
    }
    pub async fn preview_channel_access_token(
        &self,
        token: &str,
    ) -> Result<ChannelAccessTokenPreview> {
        if parse_private_channel_invite_token(token).is_ok() {
            let preview = parse_private_channel_invite_token(token)?;
            return Ok(ChannelAccessTokenPreview {
                kind: ChannelAccessTokenKind::Invite,
                topic_id: preview.topic_id.as_str().to_string(),
                channel_id: preview.channel_id.as_str().to_string(),
                channel_label: preview.channel_label,
                owner_pubkey: preview.owner_pubkey.as_str().to_string(),
                inviter_pubkey: Some(preview.inviter_pubkey.as_str().to_string()),
                sponsor_pubkey: None,
                epoch_id: preview.epoch_id,
            });
        }
        if parse_friend_only_grant_token(token).is_ok() {
            let preview = parse_friend_only_grant_token(token)?;
            return Ok(ChannelAccessTokenPreview {
                kind: ChannelAccessTokenKind::Grant,
                topic_id: preview.topic_id.as_str().to_string(),
                channel_id: preview.channel_id.as_str().to_string(),
                channel_label: preview.channel_label,
                owner_pubkey: preview.owner_pubkey.as_str().to_string(),
                inviter_pubkey: None,
                sponsor_pubkey: Some(preview.owner_pubkey.as_str().to_string()),
                epoch_id: preview.epoch_id,
            });
        }
        if parse_friend_plus_share_token(token).is_ok() {
            let preview = parse_friend_plus_share_token(token)?;
            return Ok(ChannelAccessTokenPreview {
                kind: ChannelAccessTokenKind::Share,
                topic_id: preview.topic_id.as_str().to_string(),
                channel_id: preview.channel_id.as_str().to_string(),
                channel_label: preview.channel_label,
                owner_pubkey: preview.owner_pubkey.as_str().to_string(),
                inviter_pubkey: None,
                sponsor_pubkey: Some(preview.sponsor_pubkey.as_str().to_string()),
                epoch_id: preview.epoch_id,
            });
        }
        anyhow::bail!("unrecognized private channel access token")
    }
    pub async fn export_friend_only_grant(
        &self,
        topic_id: &str,
        channel_id: &str,
        expires_at: Option<i64>,
    ) -> Result<String> {
        let state = self
            .private_channel_state_for_owner_action(
                topic_id,
                &ChannelId::new(channel_id),
                PrivateChannelOwnerAction::Share,
            )
            .await?;
        if state.audience_kind != ChannelAudienceKind::FriendOnly {
            anyhow::bail!("friend-only grant export is only available for friends channels");
        }
        if state.owner_pubkey != self.current_author_pubkey() {
            anyhow::bail!("only the channel owner can create friend-only grants");
        }
        let diagnostics = self.private_channel_diagnostics(&state).await?;
        if diagnostics.sharing_state != ChannelSharingState::Open {
            anyhow::bail!("friend-only grant export is disabled while sharing is frozen");
        }
        build_friend_only_grant_token(
            self.keys(),
            &TopicId::new(topic_id),
            &state.channel_id,
            state.label.as_str(),
            state.current_epoch_id.as_str(),
            state.current_epoch_secret_hex.as_str(),
            expires_at,
        )
    }
    pub async fn import_friend_only_grant(&self, token: &str) -> Result<FriendOnlyGrantPreview> {
        let preview = parse_friend_only_grant_token(token)?;
        self.import_private_channel_by_spec(PrivateChannelImportSpec {
            topic_id: preview.topic_id.as_str().to_string(),
            channel_id: preview.channel_id.clone(),
            channel_label: preview.channel_label.clone(),
            owner_pubkey: preview.owner_pubkey.as_str().to_string(),
            epoch_id: preview.epoch_id.clone(),
            namespace_secret_hex: preview.namespace_secret_hex.clone(),
            expires_at: preview.expires_at,
            kind: PrivateChannelImportKind::FriendOnly,
            mutual_with: Some(preview.owner_pubkey.as_str().to_string()),
            use_legacy_aware_replica: false,
            audience: ChannelAudienceKind::FriendOnly,
            join_mode: PrivateChannelJoinMode::FriendOnlyGrant,
            // 参加ドキュメントの sponsor は snapshot 取得後の policy.owner_pubkey(統合前と同じ)。
            sponsor: ImportSponsor::PolicyOwner,
            share_token_id: None,
            // friend-only は既参加でも参加ドキュメントを再発行する(統合前と同じ)。
            skip_persist_if_participant: false,
            joined_via_pubkey: preview.owner_pubkey.as_str().to_string(),
        })
        .await?;
        Ok(preview)
    }
    pub async fn export_friend_plus_share(
        &self,
        topic_id: &str,
        channel_id: &str,
        expires_at: Option<i64>,
    ) -> Result<String> {
        let state = self
            .private_channel_state_for_owner_action(
                topic_id,
                &ChannelId::new(channel_id),
                PrivateChannelOwnerAction::Share,
            )
            .await?;
        if state.audience_kind != ChannelAudienceKind::FriendPlus {
            anyhow::bail!("friend-plus share export is only available for friends+ channels");
        }
        let replica = &current_private_channel_replica_id(&state);
        let Some(policy) = self
            .read_joined_private_control(&state, |docs, policy| async move {
                fetch_private_channel_policy_from_replica(docs.as_ref(), replica, policy).await
            })
            .await?
        else {
            anyhow::bail!("friend-plus channel policy is missing");
        };
        if policy.sharing_state != ChannelSharingState::Open {
            anyhow::bail!("friend-plus share export is disabled while sharing is frozen");
        }
        let local_author = &self.current_author_pubkey();
        if !self
            .read_joined_private_control(&state, |docs, policy| async move {
                fetch_private_channel_participant_from_replica(
                    docs.as_ref(),
                    replica,
                    local_author,
                    policy,
                )
                .await
            })
            .await?
            .is_some_and(|participant| {
                participant.epoch_id == state.current_epoch_id && participant.left_at.is_none()
            })
        {
            anyhow::bail!("only active participants can create friend-plus shares");
        }
        let effective_expires_at =
            expires_at.or_else(|| Some(Utc::now().timestamp_millis() + 24 * 60 * 60 * 1000));
        build_friend_plus_share_token(
            self.keys(),
            &TopicId::new(topic_id),
            &state.channel_id,
            state.label.as_str(),
            &Pubkey::from(state.owner_pubkey.clone()),
            state.current_epoch_id.as_str(),
            state.current_epoch_secret_hex.as_str(),
            effective_expires_at,
        )
    }
    pub async fn import_friend_plus_share(&self, token: &str) -> Result<FriendPlusSharePreview> {
        let preview = parse_friend_plus_share_token(token)?;
        self.import_private_channel_by_spec(PrivateChannelImportSpec {
            topic_id: preview.topic_id.as_str().to_string(),
            channel_id: preview.channel_id.clone(),
            channel_label: preview.channel_label.clone(),
            owner_pubkey: preview.owner_pubkey.as_str().to_string(),
            epoch_id: preview.epoch_id.clone(),
            namespace_secret_hex: preview.namespace_secret_hex.clone(),
            // friend-plus 共有トークンは期限チェックを行わない(統合前と同じ)。
            expires_at: None,
            kind: PrivateChannelImportKind::FriendPlus,
            mutual_with: Some(preview.sponsor_pubkey.as_str().to_string()),
            use_legacy_aware_replica: false,
            audience: ChannelAudienceKind::FriendPlus,
            join_mode: PrivateChannelJoinMode::FriendPlusShare,
            sponsor: ImportSponsor::Pubkey(preview.sponsor_pubkey.clone()),
            share_token_id: Some(preview.share_token_id.clone()),
            skip_persist_if_participant: true,
            joined_via_pubkey: preview.sponsor_pubkey.as_str().to_string(),
        })
        .await?;
        Ok(preview)
    }
    pub async fn freeze_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<JoinedPrivateChannelView> {
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        if state.audience_kind != ChannelAudienceKind::FriendPlus {
            anyhow::bail!("freeze is only available for friend-plus channels");
        }
        if state.owner_pubkey != self.current_author_pubkey() {
            anyhow::bail!("only the channel owner can freeze the channel");
        }
        let current_replica = current_private_channel_replica_id(&state);
        let Some(current_policy) = fetch_private_channel_policy_from_replica(
            self.docs_sync(),
            &current_replica,
            LocalThenRemote,
        )
        .await?
        else {
            anyhow::bail!("friend-plus channel policy is missing");
        };
        persist_private_channel_policy(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelPolicyDocV1 {
                sharing_state: ChannelSharingState::Frozen,
                rotated_at: current_policy.rotated_at,
                ..current_policy
            },
            &current_replica,
        )
        .await?;
        self.joined_private_channel_view_for_state(&state).await
    }

    pub async fn set_private_channel_entry_dome(
        &self,
        topic_id: &str,
        channel_id: &str,
        entry_dome_instance_id: Option<String>,
    ) -> Result<JoinedPrivateChannelView> {
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        if state.owner_pubkey != self.current_author_pubkey() {
            anyhow::bail!("only the channel owner can set the entry Dome");
        }
        let context = SpatialContextV1::Channel {
            topic_id: TopicId::new(topic_id),
            channel_id: state.channel_id.clone(),
        };
        let replica = self.hosting_context_replica(&context).await?;
        let entry_dome_instance_id = entry_dome_instance_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(instance_id) = entry_dome_instance_id.as_deref() {
            let instance = self
                .hosting_instance(&replica, &context, instance_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("entry Dome is not in this Spatial Context"))?;
            if instance.spatial_context != context
                || instance.status != DomeInstanceStatusV1::Active
            {
                anyhow::bail!("entry Dome is not active in this Spatial Context");
            }
        }
        let current_policy =
            fetch_private_channel_policy_from_replica(self.docs_sync(), &replica, LocalThenRemote)
                .await?
                .ok_or_else(|| anyhow::anyhow!("private channel policy is missing"))?;
        persist_private_channel_policy(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelPolicyDocV1 {
                entry_dome_instance_id,
                ..current_policy
            },
            &replica,
        )
        .await?;
        self.joined_private_channel_view_for_state(&state).await
    }
    /// private channel の epoch rotate(所有者のみ)。
    ///
    /// フェーズ分割(WP-H5 PR4)。順序と失敗時挙動は分割前と同一:
    /// 準備・受信者収集 → 旧 epoch 凍結 → 新 epoch 作成 → handoff grant 配布 →
    /// 状態更新・通知。途中で失敗した場合の巻き戻しは行わない(分割前と同じ。
    /// 受信側は maybe_redeem_epoch_handoff_grants_for_channel で追いつく)。
    pub async fn rotate_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<JoinedPrivateChannelView> {
        let prep = self
            .prepare_private_channel_rotation(topic_id, channel_id)
            .await?;
        self.freeze_rotated_epoch_policy(&prep).await?;
        let next = self
            .seed_next_private_channel_epoch(
                topic_id,
                &prep.state,
                prep.current_policy.entry_dome_instance_id.clone(),
            )
            .await?;
        self.distribute_epoch_handoff_grants(topic_id, &prep, &next)
            .await?;
        self.finalize_rotated_channel_state(topic_id, prep.state, next)
            .await
    }
    /// フェーズ 1: 前提検証(参加中・epoch 対応・所有者)と、現行 replica の
    /// policy 取得、handoff grant の受信者収集(現 epoch + 過去 epoch の active
    /// 参加者。owner 除外・pubkey で重複排除)。
    async fn prepare_private_channel_rotation(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<PrivateChannelRotationPrep> {
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        if !private_channel_is_epoch_aware(&state.audience_kind) {
            anyhow::bail!("rotate is only available for epoch-aware private channels");
        }
        if state.owner_pubkey != self.current_author_pubkey() {
            anyhow::bail!("only the channel owner can rotate the channel");
        }
        let current_replica = current_private_channel_replica_id(&state);
        let current_policy = fetch_private_channel_policy_from_replica(
            self.docs_sync(),
            &current_replica,
            LocalThenRemote,
        )
        .await?
        .unwrap_or(PrivateChannelPolicyDocV1 {
            channel_id: state.channel_id.clone(),
            topic_id: TopicId::new(topic_id),
            audience_kind: state.audience_kind.clone(),
            owner_pubkey: Pubkey::from(state.owner_pubkey.clone()),
            epoch_id: state.current_epoch_id.clone(),
            sharing_state: ChannelSharingState::Open,
            rotated_at: None,
            previous_epoch_id: None,
            entry_dome_instance_id: None,
        });
        let current_participants = fetch_private_channel_participants_from_replica(
            self.docs_sync(),
            &current_replica,
            LocalThenRemote,
        )
        .await?;
        let mut rotation_recipients = BTreeMap::new();
        for participant in active_private_channel_participants(
            &current_participants,
            state.current_epoch_id.as_str(),
        ) {
            if participant.is_owner {
                continue;
            }
            rotation_recipients
                .entry(participant.participant_pubkey.as_str().to_string())
                .or_insert(participant);
        }
        for epoch in &state.archived_epochs {
            let archived_replica =
                private_channel_epoch_replica_id(channel_id, epoch.epoch_id.as_str());
            let archived_participants = fetch_private_channel_participants_from_replica(
                self.docs_sync(),
                &archived_replica,
                LocalThenRemote,
            )
            .await?;
            for participant in
                active_private_channel_participants(&archived_participants, epoch.epoch_id.as_str())
            {
                if participant.is_owner {
                    continue;
                }
                rotation_recipients
                    .entry(participant.participant_pubkey.as_str().to_string())
                    .or_insert(participant);
            }
        }
        Ok(PrivateChannelRotationPrep {
            state,
            current_replica,
            current_policy,
            rotation_recipients,
        })
    }
    /// フェーズ 2: 旧 epoch の policy を Frozen + rotated_at で書き込み、
    /// 以後の import(sharing_state Open 前提)を止める。
    async fn freeze_rotated_epoch_policy(&self, prep: &PrivateChannelRotationPrep) -> Result<()> {
        persist_private_channel_policy(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelPolicyDocV1 {
                sharing_state: ChannelSharingState::Frozen,
                rotated_at: Some(Utc::now().timestamp_millis()),
                ..prep.current_policy.clone()
            },
            &prep.current_replica,
        )
        .await
    }
    /// フェーズ 3: 新 epoch(id / secret / replica)を作成し、metadata・
    /// Open policy・owner 参加ドキュメントを書き込む。
    async fn seed_next_private_channel_epoch(
        &self,
        topic_id: &str,
        state: &JoinedPrivateChannelState,
        entry_dome_instance_id: Option<String>,
    ) -> Result<PrivateChannelNextEpoch> {
        let epoch_id = next_private_channel_epoch_id(self.current_author_pubkey().as_str());
        let secret_hex = generate_keys().export_secret_hex();
        let replica =
            private_channel_epoch_replica_id(state.channel_id.as_str(), epoch_id.as_str());
        self.docs_sync()
            .register_private_replica_secret(&replica, secret_hex.as_str())
            .await?;
        let metadata = PrivateChannelMetadataDocV1 {
            channel_id: state.channel_id.clone(),
            topic_id: TopicId::new(topic_id),
            label: state.label.clone(),
            creator_pubkey: Pubkey::from(state.creator_pubkey.clone()),
            created_at: Utc::now().timestamp_millis(),
            audience_kind: state.audience_kind.clone(),
            owner_pubkey: Pubkey::from(state.owner_pubkey.clone()),
        };
        persist_private_channel_metadata(self.docs_sync(), &replica, &metadata).await?;
        persist_private_channel_policy(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelPolicyDocV1 {
                channel_id: state.channel_id.clone(),
                topic_id: TopicId::new(topic_id),
                audience_kind: state.audience_kind.clone(),
                owner_pubkey: Pubkey::from(state.owner_pubkey.clone()),
                epoch_id: epoch_id.clone(),
                sharing_state: ChannelSharingState::Open,
                rotated_at: None,
                previous_epoch_id: Some(state.current_epoch_id.clone()),
                entry_dome_instance_id,
            },
            &replica,
        )
        .await?;
        persist_private_channel_participant(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelParticipantDocV1 {
                channel_id: state.channel_id.clone(),
                topic_id: TopicId::new(topic_id),
                epoch_id: epoch_id.clone(),
                participant_pubkey: Pubkey::from(state.owner_pubkey.clone()),
                joined_at: Utc::now().timestamp_millis(),
                is_owner: true,
                join_mode: Some(PrivateChannelJoinMode::OwnerSeed),
                sponsor_pubkey: None,
                share_token_id: None,
                left_at: None,
            },
            &replica,
        )
        .await?;
        Ok(PrivateChannelNextEpoch {
            epoch_id,
            secret_hex,
        })
    }
    /// フェーズ 4: 受信者へ handoff grant を暗号化して旧 replica に書く。
    /// friend-only は配布時点でも mutual を再確認し、外れていれば黙って配らない
    /// (分割前と同じ。受け取れなかった参加者は新 epoch に入れない)。
    async fn distribute_epoch_handoff_grants(
        &self,
        topic_id: &str,
        prep: &PrivateChannelRotationPrep,
        next: &PrivateChannelNextEpoch,
    ) -> Result<()> {
        let state = &prep.state;
        for participant in prep.rotation_recipients.values() {
            if state.audience_kind == ChannelAudienceKind::FriendOnly {
                let relationship = self
                    .services
                    .projection_store
                    .get_author_relationship(
                        self.current_author_pubkey().as_str(),
                        participant.participant_pubkey.as_str(),
                    )
                    .await?;
                if !relationship.as_ref().is_some_and(|value| value.mutual) {
                    continue;
                }
            }
            let grant_doc = encrypt_private_channel_epoch_handoff_grant(
                self.keys(),
                &PrivateChannelEpochHandoffGrantPayloadV1 {
                    channel_id: state.channel_id.clone(),
                    topic_id: TopicId::new(topic_id),
                    owner_pubkey: Pubkey::from(state.owner_pubkey.clone()),
                    recipient_pubkey: participant.participant_pubkey.clone(),
                    old_epoch_id: state.current_epoch_id.clone(),
                    new_epoch_id: next.epoch_id.clone(),
                    new_namespace_secret_hex: next.secret_hex.clone(),
                },
            )?;
            persist_private_channel_epoch_handoff_grant(
                self.docs_sync(),
                self.keys(),
                &grant_doc,
                &prep.current_replica,
            )
            .await?;
        }
        Ok(())
    }
    /// フェーズ 5: 旧 epoch を archive して現 epoch を差し替え、registry へ登録、
    /// rotation hint を publish(失敗は warn のみ)して view を返す。
    async fn finalize_rotated_channel_state(
        &self,
        topic_id: &str,
        mut state: JoinedPrivateChannelState,
        next: PrivateChannelNextEpoch,
    ) -> Result<JoinedPrivateChannelView> {
        let archived_epoch_id = state.current_epoch_id.clone();
        let archived_secret = state.current_epoch_secret_hex.clone();
        archive_private_channel_epoch(
            &mut state,
            archived_epoch_id.as_str(),
            archived_secret.as_str(),
        );
        state.current_epoch_id = next.epoch_id;
        state.current_epoch_secret_hex = next.secret_hex;
        self.register_joined_private_channel(state.clone()).await?;
        if let Err(error) = self
            .services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, Some(&state.channel_id)),
                GossipHint::TopicObjectsChanged {
                    topic_id: TopicId::new(topic_id),
                    objects: Vec::new(),
                },
            )
            .await
        {
            warn!(
                topic = %topic_id,
                channel_id = %state.channel_id.as_str(),
                epoch_id = %state.current_epoch_id,
                error = %error,
                "failed to publish private channel rotation hint"
            );
        }
        self.joined_private_channel_view_for_state(&state).await
    }
    pub async fn restore_private_channel_capability(
        &self,
        capability: PrivateChannelCapability,
    ) -> Result<()> {
        let state = joined_private_channel_state_from_capability(capability)?;
        self.restore_joined_private_channel(state).await
    }
    pub async fn leave_private_channel(&self, topic_id: &str, channel_id: &str) -> Result<()> {
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        let replica = current_private_channel_replica_id(&state);
        let local_author = self.current_author_pubkey();
        let local_pubkey = Pubkey::from(local_author.clone());
        let now = Utc::now().timestamp_millis();
        let existing_participant = fetch_private_channel_participant_from_replica(
            self.docs_sync(),
            &replica,
            local_author.as_str(),
            DocFetchPolicy::LocalOnly,
        )
        .await?
        .filter(|participant| participant.epoch_id == state.current_epoch_id);
        persist_private_channel_participant(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelParticipantDocV1 {
                channel_id: state.channel_id.clone(),
                topic_id: TopicId::new(topic_id),
                epoch_id: state.current_epoch_id.clone(),
                participant_pubkey: local_pubkey,
                joined_at: existing_participant
                    .as_ref()
                    .map(|participant| participant.joined_at)
                    .unwrap_or(now),
                is_owner: existing_participant
                    .as_ref()
                    .map(|participant| participant.is_owner)
                    .unwrap_or(state.owner_pubkey == local_author),
                join_mode: existing_participant
                    .as_ref()
                    .and_then(|participant| participant.join_mode.clone()),
                sponsor_pubkey: existing_participant
                    .as_ref()
                    .and_then(|participant| participant.sponsor_pubkey.clone()),
                share_token_id: existing_participant
                    .as_ref()
                    .and_then(|participant| participant.share_token_id.clone()),
                left_at: Some(now),
            },
            &replica,
        )
        .await?;
        let has_peers = self
            .services
            .transport
            .peers()
            .await
            .is_ok_and(|peers| peers.peer_count > 0);
        if has_peers {
            let publish_result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                self.hint_transport().publish_hint(
                    &channel_hint_topic_for(topic_id, Some(&state.channel_id)),
                    GossipHint::TopicObjectsChanged {
                        topic_id: TopicId::new(topic_id),
                        objects: Vec::new(),
                    },
                ),
            )
            .await;
            match publish_result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(
                        topic = %topic_id,
                        channel_id = %state.channel_id.as_str(),
                        epoch_id = %state.current_epoch_id,
                        error = %error,
                        "failed to publish private channel leave hint"
                    );
                }
                Err(_) => {
                    warn!(
                        topic = %topic_id,
                        channel_id = %state.channel_id.as_str(),
                        epoch_id = %state.current_epoch_id,
                        "timed out publishing private channel leave hint"
                    );
                }
            }
        }
        self.remove_joined_private_channel_and_evict_dome_participant(topic_id, &state.channel_id)
            .await?;
        Ok(())
    }
    pub async fn list_joined_private_channels(
        &self,
        topic_id: &str,
    ) -> Result<Vec<JoinedPrivateChannelView>> {
        self.maybe_restart_scope_replica_sync(topic_id, &ReplicaScope::AllJoined)
            .await;
        for state in self.joined_private_channel_states_for_topic(topic_id).await {
            self.maybe_redeem_epoch_handoff_grants_for_channel(topic_id, state.channel_id.as_str())
                .await?;
        }
        let mut items = Vec::new();
        for state in self.joined_private_channel_states_for_topic(topic_id).await {
            items.push(self.joined_private_channel_view_for_state(&state).await?);
        }
        Ok(items)
    }
    /// テスト専用: capability の取得→restore 結合テストのユーティリティ
    /// (production の呼び出し元は WP-C2 T5 #479 で消滅)。
    #[cfg(test)]
    pub(crate) async fn get_private_channel_capability(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<PrivateChannelCapability>> {
        self.maybe_redeem_epoch_handoff_grants_for_channel(topic_id, channel_id)
            .await?;
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            return Ok(None);
        };
        Ok(Some(
            self.private_channel_capability_from_state(&state).await?,
        ))
    }
}
/// import 3 系統の差分(注入点)。詳細は import_private_channel_by_spec を参照。
struct PrivateChannelImportSpec {
    topic_id: String,
    channel_id: ChannelId,
    channel_label: String,
    owner_pubkey: String,
    epoch_id: String,
    namespace_secret_hex: String,
    /// トークン期限(None = 期限チェックなし)。
    expires_at: Option<i64>,
    kind: PrivateChannelImportKind,
    /// mutual 関係の前提となる相手 pubkey(None = チェックなし)。
    mutual_with: Option<String>,
    /// epoch なし時代の channel を legacy replica で読むか(招待のみ true。互換パス)。
    use_legacy_aware_replica: bool,
    audience: ChannelAudienceKind,
    join_mode: PrivateChannelJoinMode,
    sponsor: ImportSponsor,
    share_token_id: Option<String>,
    /// 既に参加者なら参加ドキュメントの発行をスキップするか(friend-only のみ false)。
    skip_persist_if_participant: bool,
    joined_via_pubkey: String,
}
/// 参加ドキュメントに載せる sponsor の出どころ。
enum ImportSponsor {
    /// トークン preview から(招待 = inviter、friend-plus = sponsor)。
    Pubkey(Pubkey),
    /// snapshot 取得後の policy.owner_pubkey(friend-only)。
    PolicyOwner,
}

/// rotate フェーズ 1(準備)の成果物。
struct PrivateChannelRotationPrep {
    state: JoinedPrivateChannelState,
    current_replica: ReplicaId,
    current_policy: PrivateChannelPolicyDocV1,
    /// handoff grant の受信者(pubkey で重複排除済み。owner は含まない)。
    rotation_recipients: BTreeMap<String, PrivateChannelParticipantDocV1>,
}

/// rotate フェーズ 3(新 epoch 作成)の成果物。
struct PrivateChannelNextEpoch {
    epoch_id: String,
    secret_hex: String,
}

/// friend-only grant import 失敗の再試行可能性(リトライ契約。WP-B11)。
///
/// gossip 伝播待ちで解消しうる失敗(mutual 未成立 / epoch 不一致 / owner 未着 /
/// snapshot timeout)のみ true。判定は `PrivateChannelImportError` への downcast で
/// 行い、**エラー文言に依存しない**(文言は表示用で、契約はこの関数が正本)。
/// リトライ対象の分類は crate 内の characterization テスト(tests/support/waiters.rs)
/// が固定している。
pub fn is_retryable_friend_only_grant_import_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<PrivateChannelImportError>()
        .is_some_and(PrivateChannelImportError::is_retryable_friend_only)
}

/// friend-plus share import 失敗の再試行可能性(リトライ契約。WP-B11)。
pub fn is_retryable_friend_plus_share_import_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<PrivateChannelImportError>()
        .is_some_and(PrivateChannelImportError::is_retryable_friend_plus)
}

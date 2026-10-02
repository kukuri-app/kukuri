use crate::service::*;
use DocFetchPolicy::LocalThenRemote;
use kukuri_core::{AccountSyncItem, DomeInstanceStatusV1, SpatialContextV1};
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
            // #1219 W6: 作成した端末が最初の担当。
            controller: Some(self.first_controller().await?),
        };
        self.register_joined_private_channel(state.clone(), now)
            .await?;
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
        self.record_private_channel_participant(
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
            &state.owner_pubkey,
            &state.current_epoch_secret_hex,
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
        // 参加の記録の時刻は、参加の版の時刻(ADR 0061 §9)。
        let joined_at = Utc::now().timestamp_millis();
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
                self.record_private_channel_participant(
                    &PrivateChannelParticipantDocV1 {
                        channel_id: metadata.channel_id.clone(),
                        topic_id: metadata.topic_id.clone(),
                        epoch_id: policy.epoch_id.clone(),
                        participant_pubkey: local_pubkey,
                        joined_at,
                        is_owner: false,
                        join_mode: Some(spec.join_mode),
                        sponsor_pubkey: Some(sponsor_pubkey),
                        share_token_id: spec.share_token_id.clone(),
                        left_at: None,
                    },
                    spec.owner_pubkey.as_str(),
                    spec.namespace_secret_hex.as_str(),
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
            self.register_joined_private_channel(next_state, joined_at)
                .await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        // 招待の一時の秘密は、参加の成否によらず外す(参加したら、docs は鍵の行から引く。ADR 0061 §9)。
        let _ = self
            .services
            .docs_sync
            .remove_private_replica_secret(&replica)
            .await;
        if import_result.is_err() && newly_held {
            self.release_scope_holder(&private_channel_holder(
                spec.topic_id.as_str(),
                spec.channel_id.as_str(),
            ))
            .await;
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
                .hosting_instance(&context, instance_id)
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
    /// 操作は新しい世代の鍵の行の予約で始まり、その世代の ID を操作 ID にする(#1219 AC-2、ADR 0018 §8)。
    /// 予約 → 旧 epoch 凍結 → 新 epoch 作成 → 確定(現在の世代と account 同期への記録)→ handoff grant の配布の
    /// 最初の 1 page。確定の前に失敗したら、同じ操作の再送が同じ世代を再開する。確定の後の残り(account 同期への
    /// 記録の前に止まった分と、配布の続き)は DM outbox の再送 owner の tick が 1 page ずつ進める。
    pub async fn rotate_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<JoinedPrivateChannelView> {
        let rotation = self
            .prepare_private_channel_rotation(topic_id, channel_id)
            .await?;
        if let Err(error) = self.advance_private_channel_rotation(&rotation).await {
            let state = self
                .joined_private_channel_state(topic_id, channel_id)
                .await
                .context("private channel is not joined")?;
            if state.current_epoch_id == rotation.from_epoch_id {
                return Err(error);
            }
            warn!(
                topic = %topic_id,
                channel_id = %channel_id,
                epoch_id = %rotation.epoch_id,
                error = %error,
                "private channel rotation continues in the background"
            );
        }
        let state = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
            .context("private channel is not joined")?;
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
    /// 前提検証(参加中・epoch 対応・所有者・担当端末)と、操作の予約。現在の世代から予約した確定前の世代があれば
    /// それを再開し、無ければ新しい世代の ID と秘密を作って鍵の行を 1 行書く(docs へはまだ何も書かない)。
    async fn prepare_private_channel_rotation(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<PrivateChannelRotation> {
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
        // #1219 W6: 明示の rotate も write/share 前の auto rotate もここを通る。担当端末でなければ何も書かずに保留する。
        let device_id = self.local_device_id().await?;
        if !state.controller.as_ref().is_some_and(|controller| {
            controller.device_id == device_id && controller.transfer_to.is_none()
        }) {
            return Err(PrivateChannelControllerPending.into());
        }
        // 予約の照合と書込みは、参加の行の commit と同じ排他の中で行う(同じ端末の同時の鍵更新は同じ操作になる)。
        let _save_access = self.services.content_save_access.lock().await;
        let store = &self.services.projection_store;
        let from_epoch_id = store
            .get_private_channel(&joined_private_channel_key(topic_id, channel_id))
            .await?
            .filter(|row| row.joined)
            .context("private channel is not joined")?
            .current_epoch_id;
        let reserved = store
            .list_private_channel_epochs(
                channel_id,
                kukuri_store::PrivateChannelEpochRange::After(epoch_started_at(&from_epoch_id)),
                1,
            )
            .await?
            .pop()
            .filter(|row| row.rotation_from.as_deref() == Some(from_epoch_id.as_str()));
        let row = match reserved {
            Some(row) => row,
            None => {
                let epoch_id = next_private_channel_epoch_id(
                    self.current_author_pubkey().as_str(),
                    &from_epoch_id,
                );
                let mut row = self.services.private_channel_epoch_row(
                    channel_id,
                    &epoch_id,
                    &generate_keys().export_secret_hex(),
                    Utc::now().timestamp_millis(),
                )?;
                row.rotation_from = Some(from_epoch_id);
                store.put_private_channel_epoch(&row).await?;
                row
            }
        };
        self.services.private_channel_rotation(topic_id, row)
    }
    /// 予約した操作を次の段へ進める(受付と背景の step が共に使う)。確定前なら旧 epoch の凍結 → 新 epoch の作成 →
    /// 確定、確定の後で account 同期への記録が済んでいなければ記録し、配布を 1 page 進める。各段は同じ操作で
    /// やり直しても同じ世代を書く。
    pub(crate) async fn advance_private_channel_rotation(
        &self,
        rotation: &PrivateChannelRotation,
    ) -> Result<()> {
        let store = &self.services.projection_store;
        let Some(state) = self
            .joined_private_channel_state(&rotation.topic_id, &rotation.channel_id)
            .await
        else {
            // 退会した channel の鍵更新は終える(鍵の行は退会で消える)。
            return store
                .set_private_channel_rotation(&rotation.channel_id, &rotation.epoch_id, None)
                .await;
        };
        let after = match rotation.after.clone() {
            Some(after) => after,
            None => {
                if state.current_epoch_id == rotation.from_epoch_id {
                    self.commit_private_channel_rotation(rotation, state.clone())
                        .await?;
                } else {
                    // 確定の後、account 同期への記録と印の更新の前に止まっていた。
                    self.write_account_sync_item(&AccountSyncItem::channel_epoch(
                        &state.channel_id,
                        &rotation.epoch_id,
                        Utc::now().timestamp_millis(),
                        serde_json::to_value(PrivateChannelEpochCapability {
                            epoch_id: rotation.epoch_id.clone(),
                            namespace_secret_hex: rotation.secret_hex.clone(),
                        })?,
                    )?)
                    .await?;
                }
                store
                    .set_private_channel_rotation(
                        &rotation.channel_id,
                        &rotation.epoch_id,
                        Some(""),
                    )
                    .await?;
                String::new()
            }
        };
        // 配布: 受信者(owner の参加者の表で、いずれかの epoch に参加中の pubkey。owner を除く)へ handoff grant を
        // 暗号化して旧 replica に書き、account 経路で届ける(ACK まで再送。#1221 R5-H)。宛先は表を pubkey の順に
        // 128 件の page で読み、cursor を鍵の行に置く。friend-only は配布時点でも mutual を再確認する。
        let page = store
            .list_private_channel_participants(
                &rotation.channel_id,
                None,
                &after,
                PRIVATE_CHANNEL_PARTICIPANT_PAGE,
            )
            .await?;
        let mut old = state;
        old.current_epoch_secret_hex = self
            .services
            .private_channel_epoch_secret(&rotation.channel_id, &rotation.from_epoch_id)
            .await?
            .context("private channel rotation source epoch is missing")?;
        old.current_epoch_id = rotation.from_epoch_id.clone();
        for recipient in &page {
            self.distribute_epoch_handoff_grant(
                &old,
                &rotation.epoch_id,
                &rotation.secret_hex,
                recipient,
            )
            .await?;
        }
        let next = page
            .last()
            .filter(|_| page.len() == PRIVATE_CHANNEL_PARTICIPANT_PAGE);
        store
            .set_private_channel_rotation(
                &rotation.channel_id,
                &rotation.epoch_id,
                next.map(String::as_str),
            )
            .await
    }
    /// 確定前の段: 旧 epoch の policy を Frozen にし(以後の import を止める)、新 epoch の metadata・Open policy・
    /// owner の参加を書き、参加の行の現在の世代を進める(account 同期への記録を含む)。新しい世代の秘密は予約した
    /// 鍵の行から docs が引く。
    async fn commit_private_channel_rotation(
        &self,
        rotation: &PrivateChannelRotation,
        mut state: JoinedPrivateChannelState,
    ) -> Result<()> {
        self.install_private_epoch_secrets().await?;
        let topic_id = TopicId::new(rotation.topic_id.as_str());
        let current_replica = current_private_channel_replica_id(&state);
        let current_policy = fetch_private_channel_policy_from_replica(
            self.docs_sync(),
            &current_replica,
            LocalThenRemote,
        )
        .await?
        .unwrap_or(PrivateChannelPolicyDocV1 {
            channel_id: state.channel_id.clone(),
            topic_id: topic_id.clone(),
            audience_kind: state.audience_kind.clone(),
            owner_pubkey: Pubkey::from(state.owner_pubkey.clone()),
            epoch_id: state.current_epoch_id.clone(),
            sharing_state: ChannelSharingState::Open,
            rotated_at: None,
            previous_epoch_id: None,
            entry_dome_instance_id: None,
        });
        persist_private_channel_policy(
            self.docs_sync(),
            self.keys(),
            &PrivateChannelPolicyDocV1 {
                sharing_state: ChannelSharingState::Frozen,
                rotated_at: Some(Utc::now().timestamp_millis()),
                ..current_policy.clone()
            },
            &current_replica,
        )
        .await?;
        // owner の参加の記録の時刻は、参加の版の時刻(ADR 0061 §9)。
        let joined_at = self
            .services
            .projection_store
            .get_private_channel(&joined_private_channel_key(
                &rotation.topic_id,
                &rotation.channel_id,
            ))
            .await?
            .map_or_else(|| Utc::now().timestamp_millis(), |row| row.updated_at);
        let replica = private_channel_epoch_replica_id(&rotation.channel_id, &rotation.epoch_id);
        let metadata = PrivateChannelMetadataDocV1 {
            channel_id: state.channel_id.clone(),
            topic_id: topic_id.clone(),
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
                topic_id: topic_id.clone(),
                audience_kind: state.audience_kind.clone(),
                owner_pubkey: Pubkey::from(state.owner_pubkey.clone()),
                epoch_id: rotation.epoch_id.clone(),
                sharing_state: ChannelSharingState::Open,
                rotated_at: None,
                previous_epoch_id: Some(state.current_epoch_id.clone()),
                entry_dome_instance_id: current_policy.entry_dome_instance_id,
            },
            &replica,
        )
        .await?;
        self.record_private_channel_participant(
            &PrivateChannelParticipantDocV1 {
                channel_id: state.channel_id.clone(),
                topic_id,
                epoch_id: rotation.epoch_id.clone(),
                participant_pubkey: Pubkey::from(state.owner_pubkey.clone()),
                joined_at,
                is_owner: true,
                join_mode: Some(PrivateChannelJoinMode::OwnerSeed),
                sponsor_pubkey: None,
                share_token_id: None,
                left_at: None,
            },
            &state.owner_pubkey,
            &rotation.secret_hex,
            &replica,
        )
        .await?;
        let from_epoch = std::mem::replace(&mut state.current_epoch_id, rotation.epoch_id.clone());
        state.current_epoch_secret_hex = rotation.secret_hex.clone();
        self.commit_joined_private_channel_state(
            state,
            ChannelCommit::Advance {
                from_epoch: &from_epoch,
            },
        )
        .await?
        .context("private channel is not joined")?;
        Ok(())
    }

    /// 鍵更新が終わっていない操作の 1 つを 1 段進める(DM outbox の再送 owner の tick ごと)。操作は
    /// (channel id, epoch id) の順に巡回し、`cursor` はこの tick までに進めた操作。確定前の操作は、担当の判定を
    /// 通る同じ操作の再送が再開するので、ここでは進めない。
    pub(crate) async fn step_private_channel_rotations(
        services: &ServiceHandles,
        cursor: &mut (String, String),
    ) -> Result<()> {
        let store = &services.projection_store;
        let mut rows = store
            .list_private_channel_rotations((&cursor.0, &cursor.1), 1)
            .await?;
        if rows.is_empty() && !cursor.0.is_empty() {
            rows = store.list_private_channel_rotations(("", ""), 1).await?;
        }
        let Some(row) = rows.pop() else {
            *cursor = Default::default();
            return Ok(());
        };
        *cursor = (row.channel_id.clone(), row.epoch_id.clone());
        let Some(channel) = store.get_private_channel_by_id(&row.channel_id).await? else {
            return store
                .set_private_channel_rotation(&row.channel_id, &row.epoch_id, None)
                .await;
        };
        if row.rotation_from.as_deref() == Some(channel.current_epoch_id.as_str()) {
            return Ok(());
        }
        let rotation = services.private_channel_rotation(&channel.topic_id, row)?;
        AppService::from_handles(services.clone())
            .advance_private_channel_rotation(&rotation)
            .await
    }

    pub(crate) async fn distribute_epoch_handoff_grant(
        &self,
        state: &JoinedPrivateChannelState,
        new_epoch_id: &str,
        new_secret_hex: &str,
        recipient: &str,
    ) -> Result<()> {
        if recipient == state.owner_pubkey {
            return Ok(());
        }
        if state.audience_kind == ChannelAudienceKind::FriendOnly {
            let relationship = self
                .services
                .projection_store
                .get_author_relationship(self.current_author_pubkey().as_str(), recipient)
                .await?;
            if !relationship.as_ref().is_some_and(|value| value.mutual) {
                return Ok(());
            }
        }
        let grant_doc = encrypt_private_channel_epoch_handoff_grant(
            self.keys(),
            &PrivateChannelEpochHandoffGrantPayloadV1 {
                channel_id: state.channel_id.clone(),
                topic_id: TopicId::new(state.topic_id.as_str()),
                owner_pubkey: Pubkey::from(state.owner_pubkey.clone()),
                recipient_pubkey: Pubkey::from(recipient),
                old_epoch_id: state.current_epoch_id.clone(),
                new_epoch_id: new_epoch_id.to_string(),
                new_namespace_secret_hex: new_secret_hex.to_string(),
            },
        )?;
        let envelope = persist_private_channel_epoch_handoff_grant(
            self.docs_sync(),
            self.keys(),
            &grant_doc,
            &current_private_channel_replica_id(state),
        )
        .await?;
        self.queue_epoch_control(
            recipient,
            state.channel_id.as_str(),
            &state.current_epoch_id,
            &state.current_epoch_secret_hex,
            &envelope,
        )
        .await
    }
    pub async fn leave_private_channel(&self, topic_id: &str, channel_id: &str) -> Result<()> {
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            anyhow::bail!("private channel is not joined");
        };
        // 退会の印を先に付ける(並行する世代の追加で参加に戻さない)。鍵の行は記録を書いた後に消す(ADR 0061 §9)。
        // 退出の記録の時刻は退会の版の時刻。
        let left_at = self.tombstone_private_channel(&state, None).await?;
        self.record_private_channel_leave(&state, left_at).await?;
        let has_peers = self
            .services
            .transport
            .discovery()
            .await
            .is_ok_and(|discovery| discovery.connected_peer_count > 0);
        if has_peers {
            let publish_result = n0_future::time::timeout(
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

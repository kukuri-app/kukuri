//! private channel の鍵更新の操作(#1219 W6 AC-2、ADR 0018 §8)。新しい世代の鍵の行の予約を操作 ID にし、凍結 →
//! 新しい世代 → 確定 → handoff grant の配布を、失敗・再起動・再送で同じ世代へ再開する。配布は参加者の表の 128 件の
//! page と、鍵の行に置いた cursor で背景に進める。

use crate::service::*;
use DocFetchPolicy::LocalThenRemote;
use kukuri_core::AccountSyncItem;

impl AppService {
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
                .rotation_channel_state(topic_id, channel_id)
                .await?
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
            .rotation_channel_state(topic_id, channel_id)
            .await?
            .context("private channel is not joined")?;
        // 相手が居ないと gossip の送信は待ち続けうるので、期限で打ち切る（担当が account 同期の取得の中で依頼を処理する
        // とき、その task を止めない。#1219 AC-3）。
        if let Err(error) = n0_future::time::timeout(
            std::time::Duration::from_secs(2),
            self.services.hint_transport.publish_hint(
                &channel_hint_topic_for(topic_id, Some(&state.channel_id)),
                GossipHint::TopicObjectsChanged {
                    topic_id: TopicId::new(topic_id),
                    objects: Vec::new(),
                },
            ),
        )
        .await
        .map_err(anyhow::Error::from)
        .and_then(|published| published)
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
    /// 鍵更新の段の判断に使う参加状態。正本の参加の行から読む(メモリは確定の後に lease の task が読み直すまで
    /// 古いままで、確定前と取り違えると、確定していない世代の印を消してしまう)。
    async fn rotation_channel_state(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<JoinedPrivateChannelState>> {
        self.services
            .stored_private_channel_state(&joined_private_channel_key(topic_id, channel_id))
            .await
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
            .rotation_channel_state(&rotation.topic_id, &rotation.channel_id)
            .await?
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
}

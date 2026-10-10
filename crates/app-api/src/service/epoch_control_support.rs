//! private channel の参加・退出 record と回転の handoff grant を account 経路で届ける(#1221 R5-H AC-5)。
//!
//! 署名済みの envelope を、record が属する epoch の鍵で封じて blob に置き、DM と同じ outbox(`dm_outbox`)の行として
//! 積む。送信・再送・ACK は DM の account 再送 owner が行い、行の `dm_id` の接頭辞で `EpochControl` の offer にする。
//! 受け手は epoch の識別子で参加中の channel と epoch を選び、owner は参加者の表へ、参加者は grant を手元の旧 epoch の
//! replica へ置いてから ACK を返す。
//!
//! #1219 AC-5（ADR 0061 §2、ADR 0018 §8。2026-10-04 ユーザー判断）: owner の端末は、受けた参加者の record と、参加者
//! との follow の edge を本人の端末へ同期する。鍵の配布を行う端末（担当）は同期した記録も自分で受けた記録と同じに
//! 使い、遅れて届いた参加への grant は担当だけが出す。follow の記録はフォローの表に入れず、相互フォロー限定の資格の
//! 判定にだけ使う。

use kukuri_core::{
    AccountSyncItem, AccountSyncItemKey, ChannelParticipantV1, DirectMessageAckV1, FollowEdge,
    FollowEdgeStatus, PrivateReceivePayloadV1, ReceiveOfferReferenceV1, ReceiveOfferScopeV1,
    VerifiedReceiveOffer, build_direct_message_ack, seal_private_receive_payload,
};
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};
use kukuri_store::{DirectMessageOutboxRow, PrivateChannelFilter, PrivateChannelParticipantRow};
use kukuri_transport::EndpointAddr;

use super::account_sync_support::row_of;
use super::*;

/// `dm_outbox` の行のうち、epoch の制御 record を運ぶ行の `dm_id` の接頭辞(DM の行は `dm-`)。後ろは epoch の識別子。
pub(crate) const EPOCH_CONTROL_OUTBOX_PREFIX: &str = "epoch-control:";

const EPOCH_CONTROL_MIME: &str = "application/vnd.kukuri.epoch-control+json";

/// 参加者の表を 1 回に読む行数(rotation の宛先・friend-only の関係の確認)。
pub(crate) const PRIVATE_CHANNEL_PARTICIPANT_PAGE: usize = 128;

fn participant_row(participant: &PrivateChannelParticipantDocV1) -> PrivateChannelParticipantRow {
    PrivateChannelParticipantRow {
        channel_id: participant.channel_id.as_str().to_string(),
        epoch_id: participant.epoch_id.clone(),
        participant_pubkey: participant.participant_pubkey.as_str().to_string(),
        left_at: participant.left_at,
        updated_at: participant.left_at.unwrap_or(participant.joined_at),
    }
}

fn epoch_secret(secret_hex: &str) -> Result<[u8; 32]> {
    let mut secret = [0_u8; 32];
    hex::decode_to_slice(secret_hex, &mut secret)?;
    Ok(secret)
}

impl AppService {
    /// 自分の参加・退出 record を手元の epoch の replica と参加者の表へ置き、自分が owner でなければ owner へ届ける。
    pub(crate) async fn record_private_channel_participant(
        &self,
        participant: &PrivateChannelParticipantDocV1,
        owner_pubkey: &str,
        epoch_secret_hex: &str,
        replica: &ReplicaId,
    ) -> Result<()> {
        let envelope = persist_private_channel_participant(
            self.docs_sync(),
            self.keys(),
            participant,
            replica,
        )
        .await?;
        self.services
            .projection_store
            .put_private_channel_participant(participant_row(participant))
            .await?;
        if participant.participant_pubkey.as_str() != owner_pubkey {
            self.queue_epoch_control(
                owner_pubkey,
                participant.channel_id.as_str(),
                &participant.epoch_id,
                epoch_secret_hex,
                &envelope,
            )
            .await?;
        }
        Ok(())
    }

    /// 署名済みの envelope を epoch の鍵で封じ、`recipient` 宛の outbox の行として積む(ACK まで再送する)。
    pub(crate) async fn queue_epoch_control(
        &self,
        recipient: &str,
        channel_id: &str,
        epoch_id: &str,
        epoch_secret_hex: &str,
        envelope: &KukuriEnvelope,
    ) -> Result<()> {
        let sealed = seal_private_receive_payload(
            &epoch_secret(epoch_secret_hex)?,
            channel_id,
            epoch_id,
            &serde_json::to_vec(envelope)?,
        )?;
        let dm_id = format!("{EPOCH_CONTROL_OUTBOX_PREFIX}{}", sealed.epoch_key_id);
        let message_id = envelope.id.as_str().to_string();
        // #1221 R5-I: 送信待ちの保護参照で置く(ACK で外れる)。
        let stored = self
            .services
            .blob_service
            .put_owned_blob(
                sealed.encode()?,
                EPOCH_CONTROL_MIME,
                &format!("dm_outbox:{dm_id}/{message_id}"),
            )
            .await?;
        self.services
            .projection_store
            .put_direct_message_outbox(DirectMessageOutboxRow {
                dm_id,
                message_id,
                peer_pubkey: recipient.to_string(),
                frame_blob_hash: stored.hash,
                created_at: Utc::now().timestamp_millis(),
                last_attempt_at: None,
            })
            .await
    }

    /// 更新前からの参加者を参加者の表へ移す 1 ステップ(#1221 R5-H 決定 3)。owner の channel の現 epoch の旧 docs に
    /// ある参加 record を、key の窓 1 つ(128 件まで)ずつ移す。窓は参加者の pubkey の hex の接頭辞で切り、128 件で
    /// 埋まる接頭辞は 1 桁細かくしてから読む(全件を一度に読まない)。
    ///
    /// `cursor` は保存した位置で、`"<channel の key>\t<接頭辞>"` はその channel の途中、`"<channel の key>"` はその
    /// channel まで済んだこと、空は未着手。戻り値は次の位置と、終端へ達したか。rotation は参加者の表だけを読むので、
    /// 移し終える前の rotation も、移した分と表の分を宛先にする。
    pub async fn migrate_legacy_private_channel_participants(
        &self,
        cursor: &str,
    ) -> Result<(String, bool)> {
        let local = self.current_author_pubkey();
        let (finished, prefix) = match cursor.split_once('\t') {
            Some((key, prefix)) => (key, Some(prefix)),
            None => (cursor, None),
        };
        // 自分が owner の参加中の channel を、key の順に 1 件ずつ(全件を読まない。ADR 0061 §9)。
        let row = match prefix {
            Some(_) => self
                .services
                .projection_store
                .get_private_channel(finished)
                .await?
                .filter(|row| row.joined && row.owner_pubkey == local),
            None => self
                .services
                .projection_store
                .list_joined_private_channels(PrivateChannelFilter::Owner(&local), finished, 1)
                .await?
                .pop(),
        };
        let target = match row {
            Some(row) => {
                let key = row.channel_key.clone();
                self.services
                    .private_channel_state_from_row(row)
                    .await?
                    .map(|state| (key, state))
            }
            None => None,
        };
        let Some((key, state)) = target else {
            // 途中の channel から退出していれば、その channel は済みとして次へ進む。
            return Ok((finished.to_string(), prefix.is_none()));
        };
        let prefix = prefix.unwrap_or_default();
        let replica = current_private_channel_replica_id(&state);
        let page = self
            .docs_sync()
            .query_replica_keys(
                &replica,
                DocKeyQuery {
                    prefix: stable_key("channels/participants", prefix),
                    order: DocKeyOrder::Ascending,
                    limit: PRIVATE_CHANNEL_PARTICIPANT_PAGE,
                },
            )
            .await?;
        if page.reached_limit && prefix.len() < 64 {
            return Ok((format!("{key}\t{prefix}0"), false));
        }
        for entry in page.entries {
            let Some(pubkey) = entry
                .key
                .strip_prefix("channels/participants/")
                .and_then(|rest| rest.strip_suffix("/envelope"))
            else {
                continue;
            };
            if let Some(participant) = fetch_private_channel_participant_from_replica(
                self.docs_sync(),
                &replica,
                pubkey,
                DocFetchPolicy::LocalOnly,
            )
            .await?
                && participant.channel_id == state.channel_id
            {
                self.services
                    .projection_store
                    .put_private_channel_participant(participant_row(&participant))
                    .await?;
            }
        }
        // 同じ桁数の次の接頭辞(末尾の f は繰り上げる)。無ければこの channel は済み。
        let rest = prefix.trim_end_matches('f');
        let next = match rest.chars().last().and_then(|last| last.to_digit(16)) {
            Some(digit) => format!("{key}\t{}{:x}", &rest[..rest.len() - 1], digit + 1),
            None => key,
        };
        Ok((next, false))
    }

    /// owner の端末が受けた(または本人の別の端末から同期した)参加者の record を表へ入れる。表を変えたら `true`。
    /// - その相手を channel に初めて入れたら、手元の 2 方向の follow の edge を同期する(参加より前に観測した edge)。
    ///   受けた record(`publish`)の同期より先に書く。取り込む端末(担当)が、参加者の record を取り込む前に edge を
    ///   採り、遅れた参加の grant の資格をその edge でも判断するため(#1219 AC-5 監査 B-1)。
    /// - 受けた record で表を変えたら、本人の端末へ同期する。
    /// - 回転の前に参加したのに record の到着が遅れた人(#1221 R5-H B7): 現 epoch でない参加 record は、その相手の行が
    ///   channel にまだ 1 行も無く(退出した人・既に宛先に入った人は行を持つ)、参加の時刻が回転の時刻(現 epoch の開始)
    ///   より前のときだけ受け付ける。退出した人が参加の時刻を偽って送り直しても、行は有効へ戻らず、grant も出ない。
    ///   回転後の grant は担当だけが送る(#1219 AC-5。担当でない端末は同期で担当へ回す)。
    pub(crate) async fn take_in_private_channel_participant(
        &self,
        state: &JoinedPrivateChannelState,
        participant: &Pubkey,
        record: &ChannelParticipantV1,
        publish: bool,
    ) -> Result<bool> {
        let store = &self.services.projection_store;
        let channel_id = state.channel_id.as_str();
        let known = store
            .has_private_channel_participant(channel_id, participant.as_str())
            .await?;
        let late = record.left_at.is_none() && record.epoch_id != state.current_epoch_id;
        if late
            && (known
                || !super::remote_read_support::epoch_start_millis(&state.current_epoch_id)
                    .is_some_and(|rotated_at| record.joined_at < rotated_at))
        {
            return Ok(false);
        }
        if !store
            .put_private_channel_participant(PrivateChannelParticipantRow {
                channel_id: channel_id.to_string(),
                epoch_id: record.epoch_id.clone(),
                participant_pubkey: participant.as_str().to_string(),
                left_at: record.left_at,
                updated_at: record.left_at.unwrap_or(record.joined_at),
            })
            .await?
        {
            return Ok(false);
        }
        if !known {
            let local = self.current_author_pubkey();
            for (subject, target) in [
                (local.as_str(), participant.as_str()),
                (participant.as_str(), local.as_str()),
            ] {
                if let Some(edge) = self.services.store.get_follow_edge(subject, target).await? {
                    self.publish_account_sync_item(AccountSyncItem::participant_follow(&edge)?)
                        .await?;
                }
            }
        }
        if publish {
            self.publish_account_sync_item(AccountSyncItem::channel_participant(
                &state.channel_id,
                participant,
                record,
            )?)
            .await?;
        }
        if late && self.controls_private_channel(channel_id).await? {
            self.grant_current_epoch_to_late_participant(
                state,
                &record.epoch_id,
                participant.as_str(),
            )
            .await?;
        }
        Ok(true)
    }

    /// 本人の別の端末が受けた参加者の record の item を取り込む(#1219 AC-5)。自分が owner の参加中の channel の行が
    /// あるときだけ、採用の台帳へ採って表へ入れる。採ったら `true`(取得が自分の replica へ写す)。
    pub(super) async fn merge_channel_participant(
        &self,
        channel_id: &ChannelId,
        participant: &Pubkey,
        item: &AccountSyncItem,
    ) -> Result<bool> {
        let record: ChannelParticipantV1 = serde_json::from_value(
            item.value
                .clone()
                .context("a participant item must carry its record")?,
        )?;
        let Some(row) = self
            .services
            .projection_store
            .get_private_channel_by_id(channel_id.as_str())
            .await?
            .filter(|row| row.joined && row.owner_pubkey == self.current_author_pubkey())
        else {
            return Ok(false);
        };
        let Some(state) = self
            .services
            .stored_private_channel_state(&joined_private_channel_key(
                &row.topic_id,
                channel_id.as_str(),
            ))
            .await?
        else {
            return Ok(false);
        };
        // 同じ版の再受信でも取り込みをやり直す（取り込みは表の行が変わらなければ何もしない）。
        let adopted = self
            .services
            .projection_store
            .adopt_account_sync_row(&row_of(item)?)
            .await?;
        if !adopted && !self.holds_account_sync_item(item).await? {
            return Ok(false);
        }
        self.take_in_private_channel_participant(&state, participant, &record, false)
            .await?;
        Ok(adopted)
    }

    /// 自分の channel の参加者から自分への follow の edge を、本人の端末へ同期する(#1219 AC-5。相互フォロー限定の資格の
    /// 材料)。自分から参加者への edge は、自分のフォローの edge の item が運ぶ(#1211 AC-6)。それ以外の edge は同期しない。
    pub(crate) async fn share_participant_follow(&self, edge: &FollowEdge) -> Result<()> {
        if edge.target_pubkey.as_str() != self.current_author_pubkey()
            || !self
                .services
                .projection_store
                .has_private_channel_member(edge.subject_pubkey.as_str())
                .await?
        {
            return Ok(());
        }
        self.publish_account_sync_item(AccountSyncItem::participant_follow(edge)?)
            .await
    }

    /// 自分と相手が相互フォローか(#1219 AC-5)。向きごとに、手元の edge と本人の別の端末から同期した記録の新しい方を
    /// 使う。
    pub(crate) async fn participant_mutual(&self, participant: &str) -> Result<bool> {
        let local = self.current_author_pubkey();
        Ok(self.participant_follows(&local, participant).await?
            && self.participant_follows(participant, &local).await?)
    }

    async fn participant_follows(&self, subject: &str, target: &str) -> Result<bool> {
        let local = self
            .services
            .store
            .get_follow_edge(subject, target)
            .await?
            .map(|edge| (edge.updated_at, edge.status));
        let key = AccountSyncItemKey::ParticipantFollow {
            subject: Pubkey::from(subject.to_string()),
            target: Pubkey::from(target.to_string()),
        };
        let synced = self
            .services
            .projection_store
            .get_account_sync_row(&key.docs_key())
            .await?
            .and_then(|row| {
                Some((
                    row.updated_at,
                    serde_json::from_str(row.value.as_deref()?).ok()?,
                ))
            });
        Ok(local
            .into_iter()
            .chain(synced)
            .max_by_key(|(updated_at, _)| *updated_at)
            .is_some_and(|(_, status)| status == FollowEdgeStatus::Active))
    }

    /// この端末がその channel の担当か(参加の行の記録で判断する。メモリは書込みの直後に古い)。
    async fn controls_private_channel(&self, channel_id: &str) -> Result<bool> {
        let device_id = self.local_device_id().await?;
        Ok(self
            .services
            .projection_store
            .get_private_channel_by_id(channel_id)
            .await?
            .and_then(|row| row.controller)
            .map(|json| serde_json::from_str::<PrivateChannelController>(&json))
            .transpose()?
            .is_some_and(|controller| {
                controller.device_id == device_id && controller.transfer_to.is_none()
            }))
    }

    /// owner の端末の grant の宛先が、まだ資格を持つか(#1219 AC-5。送るたびに確かめる)。その channel の参加中の行が
    /// あり、相互フォロー限定なら相互フォロー。参加者の端末から owner への参加の record は確かめない。
    pub(crate) async fn epoch_control_recipient_qualified(
        services: &ServiceHandles,
        epoch_key_id: &str,
        recipient: &str,
    ) -> Result<bool> {
        let Some((state, _, _)) = services
            .private_channel_epoch_by_receive_key(epoch_key_id)
            .await?
        else {
            return Ok(true);
        };
        if state.owner_pubkey != services.keys.public_key_hex() || recipient == state.owner_pubkey {
            return Ok(true);
        }
        Ok(services
            .projection_store
            .is_active_private_channel_participant(state.channel_id.as_str(), recipient)
            .await?
            && (state.audience_kind != ChannelAudienceKind::FriendOnly
                || AppService::from_handles(services.clone())
                    .participant_mutual(recipient)
                    .await?))
    }

    /// 回転の後に届いた、直前の epoch の参加 record の参加者へ、現 epoch の handoff grant を送る(#1221 R5-H)。参加
    /// record は account 経路で届くので、回転より後に着きうる。回転のときに表にあれば受け取れた grant と同じもの。
    async fn grant_current_epoch_to_late_participant(
        &self,
        state: &JoinedPrivateChannelState,
        participant_epoch_id: &str,
        recipient: &str,
    ) -> Result<()> {
        let Some((previous_epoch_id, previous_secret_hex)) = self
            .archived_private_channel_epochs(state, 1)
            .await?
            .into_iter()
            .next()
            .filter(|(epoch_id, _)| epoch_id == participant_epoch_id)
        else {
            return Ok(());
        };
        let mut old = state.clone();
        old.current_epoch_id = previous_epoch_id;
        old.current_epoch_secret_hex = previous_secret_hex;
        self.distribute_epoch_handoff_grant(
            &old,
            &state.current_epoch_id,
            &state.current_epoch_secret_hex,
            recipient,
        )
        .await
    }

    /// outbox の制御 record の行を運ぶ offer の参照。
    pub(crate) async fn epoch_control_reference(
        services: &ServiceHandles,
        row: &DirectMessageOutboxRow,
        epoch_key_id: &str,
        provider_endpoint_id: String,
    ) -> Result<ReceiveOfferReferenceV1> {
        let payload = services
            .blob_service
            .fetch_local_blob(&row.frame_blob_hash)
            .await?
            .context("epoch control payload is missing")?;
        Ok(ReceiveOfferReferenceV1 {
            provider_endpoint_id,
            payload_hash: row.frame_blob_hash.clone(),
            payload_bytes: u32::try_from(payload.len())?,
            scope: ReceiveOfferScopeV1::EpochControl {
                epoch_key_id: epoch_key_id.to_string(),
            },
        })
    }

    /// 参加中の channel の epoch(現在と過去)の識別子で照合し、合った epoch の制御 record を受け取って ACK を返す。
    pub(crate) async fn ingest_epoch_control_offer(
        services: &ServiceHandles,
        offer: &VerifiedReceiveOffer,
        epoch_key_id: &str,
    ) -> Result<bool> {
        // 受信 route の識別子の索引で 1 件引く(参加中の全 channel・全 epoch を走査しない。ADR 0061 §9)。
        let Some((state, epoch_id, secret)) = services
            .private_channel_epoch_by_receive_key(epoch_key_id)
            .await?
        else {
            return Ok(false);
        };
        let provider = EndpointAddr::new(offer.reference().provider_endpoint_id.parse()?);
        let payload = services
            .blob_service
            .fetch_verified_receive_offer_payload(offer, provider.clone())
            .await?;
        let envelope: KukuriEnvelope =
            serde_json::from_slice(&PrivateReceivePayloadV1::decode(&payload)?.open(
                &secret,
                state.channel_id.as_str(),
                &epoch_id,
            )?)?;
        envelope.verify()?;
        let sender = offer.sender().as_str();
        anyhow::ensure!(
            envelope.pubkey.as_str() == sender,
            "epoch control sender mismatch"
        );
        let local = services.keys.public_key_hex();
        if let Some(participant) = parse_private_channel_participant(&envelope)? {
            anyhow::ensure!(
                state.owner_pubkey == local
                    && participant.channel_id == state.channel_id
                    && participant.epoch_id == epoch_id
                    && participant.participant_pubkey.as_str() == sender,
                "participant record does not belong to this owner's epoch"
            );
            AppService::from_handles(services.clone())
                .take_in_private_channel_participant(
                    &state,
                    &participant.participant_pubkey,
                    &ChannelParticipantV1 {
                        epoch_id: participant.epoch_id.clone(),
                        joined_at: participant.joined_at,
                        left_at: participant.left_at,
                    },
                    true,
                )
                .await?;
        } else if let Some(grant) = parse_private_channel_epoch_handoff_grant(&envelope)? {
            anyhow::ensure!(
                grant.channel_id == state.channel_id
                    && grant.recipient_pubkey.as_str() == local
                    && grant.owner_pubkey.as_str() == sender
                    && state.owner_pubkey == sender
                    && grant.old_epoch_id == epoch_id,
                "handoff grant does not belong to this participant's epoch"
            );
            // 回転を待つ参加者の手元の旧 epoch の replica へ置く。redeem は手元の grant を読む。
            let replica = private_channel_replica_for_epoch(state.channel_id.as_str(), &epoch_id);
            services
                .docs_sync
                .apply_doc_op(
                    &replica,
                    DocOp::SetJson {
                        key: stable_key("channels/rotation-grants", &format!("{local}/envelope")),
                        value: serde_json::to_value(&envelope)?,
                    },
                )
                .await?;
        } else {
            anyhow::bail!("unsupported epoch control record");
        }
        let ack = build_direct_message_ack(
            services.keys.as_ref(),
            &format!("{EPOCH_CONTROL_OUTBOX_PREFIX}{epoch_key_id}"),
            envelope.id.as_str(),
            offer.sender(),
            Utc::now().timestamp_millis(),
        )?;
        Self::publish_account_receive_dm_ack(services, sender, &ack, &provider).await?;
        Ok(true)
    }

    /// 送った制御 record の ACK。送った相手の署名なら、その行を outbox から消す。
    pub(crate) async fn ingest_epoch_control_ack(
        services: &ServiceHandles,
        offer: &VerifiedReceiveOffer,
        ack: DirectMessageAckV1,
    ) -> Result<bool> {
        ack.verify()?;
        let sender = offer.sender().as_str();
        anyhow::ensure!(
            ack.sender.as_str() == sender && ack.recipient == *offer.recipient(),
            "epoch control ack sender mismatch"
        );
        let Some(row) = services
            .projection_store
            .get_direct_message_outbox(&ack.dm_id, &ack.message_id)
            .await?
        else {
            return Ok(false);
        };
        if row.peer_pubkey != sender {
            return Ok(false);
        }
        services
            .projection_store
            .remove_direct_message_outbox(&ack.dm_id, &ack.message_id)
            .await?;
        Ok(true)
    }
}

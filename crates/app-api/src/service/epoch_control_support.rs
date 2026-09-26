//! private channel の参加・退出 record と回転の handoff grant を account 経路で届ける(#1221 R5-H AC-5)。
//!
//! 署名済みの envelope を、record が属する epoch の鍵で封じて blob に置き、DM と同じ outbox(`dm_outbox`)の行として
//! 積む。送信・再送・ACK は DM の account 再送 owner が行い、行の `dm_id` の接頭辞で `EpochControl` の offer にする。
//! 受け手は epoch の識別子で参加中の channel と epoch を選び、owner は参加者の表へ、参加者は grant を手元の旧 epoch の
//! replica へ置いてから ACK を返す。

use kukuri_core::{
    DirectMessageAckV1, PrivateReceivePayloadV1, ReceiveOfferReferenceV1, ReceiveOfferScopeV1,
    VerifiedReceiveOffer, build_direct_message_ack, receive_epoch_key_id,
    seal_private_receive_payload,
};
use kukuri_store::{DirectMessageOutboxRow, PrivateChannelParticipantRow};
use kukuri_transport::EndpointAddr;

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
        let stored = self
            .services
            .blob_service
            .put_blob(sealed.encode()?, EPOCH_CONTROL_MIME)
            .await?;
        self.services
            .projection_store
            .put_direct_message_outbox(DirectMessageOutboxRow {
                dm_id: format!("{EPOCH_CONTROL_OUTBOX_PREFIX}{}", sealed.epoch_key_id),
                message_id: envelope.id.as_str().to_string(),
                peer_pubkey: recipient.to_string(),
                frame_blob_hash: stored.hash,
                created_at: Utc::now().timestamp_millis(),
                last_attempt_at: None,
            })
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
        let matched = services
            .joined_private_channels
            .lock()
            .await
            .values()
            .find_map(|state| {
                private_channel_epoch_capabilities(state)
                    .into_iter()
                    .find_map(|epoch| {
                        let secret = epoch_secret(&epoch.namespace_secret_hex).ok()?;
                        (receive_epoch_key_id(&secret, state.channel_id.as_str(), &epoch.epoch_id)
                            .ok()?
                            == epoch_key_id)
                            .then(|| (state.clone(), epoch.epoch_id, secret))
                    })
            });
        let Some((state, epoch_id, secret)) = matched else {
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
            services
                .projection_store
                .put_private_channel_participant(participant_row(&participant))
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

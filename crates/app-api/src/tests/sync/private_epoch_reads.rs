//! #1221 R5-C: private channel の取込みは、発行者の端末から参加に要る制御 record だけを key 指定で読む。
//! 参加者の数を 10 倍にしても、provider から読む量が変わらないことを固定する。

use super::*;
use kukuri_transport::{EndpointAddr, HintStream};

/// token の発行者の宛先を 1 件解決する hint transport。gossip は何も届けない。
struct ResolvingHints;

#[async_trait]
impl HintTransport for ResolvingHints {
    async fn subscribe_hints(&self, _topic: &TopicId) -> Result<HintStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }

    async fn unsubscribe_hints(&self, _topic: &TopicId) -> Result<()> {
        Ok(())
    }

    async fn publish_hint(&self, _topic: &TopicId, _hint: GossipHint) -> Result<()> {
        Ok(())
    }

    async fn resolve_receive_destination(
        &self,
        _recipient: &Pubkey,
    ) -> Result<Option<EndpointAddr>> {
        Ok(Some(EndpointAddr::new(
            iroh::SecretKey::from_bytes(&[23; 32]).public(),
        )))
    }
}

fn app_over(
    docs_sync: Arc<CountingDocsSync>,
    hints: Arc<dyn HintTransport>,
    keys: KukuriKeys,
) -> AppService {
    let store = Arc::new(MemoryStore::default());
    app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hints,
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        keys,
    )
}

#[tokio::test]
async fn invite_import_reads_a_constant_amount_regardless_of_participants() {
    let topic = "kukuri:topic:private-epoch-reads";
    let mut counts = Vec::new();
    for others in [5, 50] {
        let owner_keys = generate_keys();
        let provider = Arc::new(CountingDocsSync::default());
        let owner = app_over(
            provider.clone(),
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            owner_keys.clone(),
        );
        let channel = owner
            .create_private_channel(CreatePrivateChannelInput {
                topic_id: TopicId::new(topic),
                label: "room".into(),
                audience_kind: ChannelAudienceKind::InviteOnly,
            })
            .await
            .expect("create channel");
        let state = owner
            .joined_private_channel_state(topic, channel.channel_id.as_str())
            .await
            .expect("owner state");
        let replica = current_private_channel_replica_id(&state);
        for _ in 0..others {
            let participant = generate_keys();
            persist_private_channel_participant(
                provider.as_ref(),
                &participant,
                &PrivateChannelParticipantDocV1 {
                    channel_id: state.channel_id.clone(),
                    topic_id: TopicId::new(topic),
                    epoch_id: state.current_epoch_id.clone(),
                    participant_pubkey: Pubkey::from(participant.public_key_hex()),
                    joined_at: Utc::now().timestamp_millis(),
                    is_owner: false,
                    join_mode: Some(PrivateChannelJoinMode::InviteToken),
                    sponsor_pubkey: Some(Pubkey::from(owner_keys.public_key_hex())),
                    share_token_id: None,
                    left_at: None,
                },
                &replica,
            )
            .await
            .expect("participant");
        }
        let invite = owner
            .export_private_channel_invite(topic, channel.channel_id.as_str(), None)
            .await
            .expect("invite");

        let member_docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
        let member = app_over(member_docs, Arc::new(ResolvingHints), generate_keys());
        provider.reset_records_returned();
        provider.clear_queries().await;
        member
            .import_private_channel_invite(invite.as_str())
            .await
            .expect("import from the issuer's device");
        counts.push(provider.records_returned());
        assert!(
            member
                .joined_private_channel_state(topic, channel.channel_id.as_str())
                .await
                .is_some(),
            "the member joins with the snapshot read from the issuer"
        );
        assert!(
            provider
                .queries()
                .await
                .iter()
                .all(|(_, query)| !matches!(query, DocQuery::Prefix(_))),
            "no participant list is read"
        );
    }
    assert_eq!(counts[0], counts[1], "provider reads: {counts:?}");
}

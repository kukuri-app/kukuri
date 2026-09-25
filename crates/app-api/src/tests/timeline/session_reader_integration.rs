use super::super::*;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn real_iroh_session_target_reads_its_bucket_without_importing_it() -> Result<()> {
    let publisher_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let client_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let publisher_docs = Arc::new(kukuri_docs_sync::IrohDocsSync::new(publisher_node.clone()));
    let client_docs = Arc::new(kukuri_docs_sync::IrohDocsSync::new(client_node.clone()));
    let blobs = Arc::new(MemoryBlobService::default());
    let topic = "real-remote-session";
    let store = Arc::new(MemoryStore::default());
    let publisher = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        publisher_docs.clone(),
        blobs.clone(),
        generate_keys(),
    );
    let id = publisher
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "remote session".into(),
                description: String::new(),
            },
        )
        .await?;
    let legacy = topic_replica_id(topic);
    let state_key = format!("sessions/live/{id}/state");
    let state = publisher_docs
        .query_replica(&legacy, DocQuery::Exact(state_key.clone()))
        .await?
        .into_iter()
        .next()
        .expect("state");
    let parsed: LiveSessionStateDocV1 = serde_json::from_slice(&state.value)?;
    let envelope_key = format!("envelopes/{}", parsed.last_envelope_id.as_str());
    let signed = publisher_docs
        .query_replica(&legacy, DocQuery::Exact(envelope_key.clone()))
        .await?
        .into_iter()
        .next()
        .expect("signed manifest");
    let envelope: KukuriEnvelope = serde_json::from_slice(&signed.value)?;
    let bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_unix_seconds(envelope.created_at)?,
    )?
    .replica_id();
    publisher_docs
        .apply_doc_op(
            &bucket,
            DocOp::SetBytes {
                key: state_key,
                value: state.value.clone(),
            },
        )
        .await?;
    publisher_docs
        .apply_doc_op(
            &bucket,
            DocOp::SetBytes {
                key: envelope_key,
                value: signed.value.clone(),
            },
        )
        .await?;
    let socket = publisher_node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    client_docs
        .import_peer_ticket(&format!("{}@{socket}", publisher_node.endpoint().addr().id))
        .await?;
    let client_store = Arc::new(MemoryStore::default());
    let client = app_service_from_dependencies(
        client_store.clone(),
        client_store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        client_docs.clone(),
        blobs,
        generate_keys(),
    );
    let (source, _, manifest) = client
        .fetch_live_session_state_and_manifest(topic, &id)
        .await?
        .expect("verified remote session");
    assert_eq!(source, bucket);
    assert_eq!(manifest.title, "remote session");
    let wrong_bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_index(TimeBucket::from_unix_seconds(envelope.created_at)?.index() + 1)?,
    )?
    .replica_id();
    publisher_docs
        .apply_doc_op(
            &wrong_bucket,
            DocOp::SetBytes {
                key: format!("sessions/live/{id}/state"),
                value: state.value,
            },
        )
        .await?;
    publisher_docs
        .apply_doc_op(
            &wrong_bucket,
            DocOp::SetBytes {
                key: format!("envelopes/{}", parsed.last_envelope_id.as_str()),
                value: signed.value,
            },
        )
        .await?;
    let wrong_reader = client
        .remote_post_readers(topic, None, &wrong_bucket, None)
        .await?
        .into_iter()
        .next()
        .expect("provider");
    assert!(
        load_verified_live_session(
            wrong_reader.as_ref(),
            client.services.blob_service.as_ref(),
            &wrong_bucket,
            topic,
            &id,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        .is_none(),
        "signed update cannot be moved into another day"
    );
    let room_id = publisher
        .create_game_room(
            topic,
            CreateGameRoomInput {
                title: "remote game".into(),
                description: String::new(),
                participants: vec!["a".into(), "b".into()],
            },
        )
        .await?;
    let game_key = format!("sessions/game/{room_id}/state");
    let game_state = publisher_docs
        .query_replica(&legacy, DocQuery::Exact(game_key.clone()))
        .await?
        .into_iter()
        .next()
        .expect("game state");
    let parsed: GameRoomStateDocV1 = serde_json::from_slice(&game_state.value)?;
    let game_envelope_key = format!("envelopes/{}", parsed.last_envelope_id.as_str());
    let game_envelope = publisher_docs
        .query_replica(&legacy, DocQuery::Exact(game_envelope_key.clone()))
        .await?
        .into_iter()
        .next()
        .expect("game manifest");
    let game_signed: KukuriEnvelope = serde_json::from_slice(&game_envelope.value)?;
    let game_bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_unix_seconds(game_signed.created_at)?,
    )?
    .replica_id();
    publisher_docs
        .apply_doc_op(
            &game_bucket,
            DocOp::SetBytes {
                key: game_key,
                value: game_state.value,
            },
        )
        .await?;
    publisher_docs
        .apply_doc_op(
            &game_bucket,
            DocOp::SetBytes {
                key: game_envelope_key,
                value: game_envelope.value,
            },
        )
        .await?;
    let (source, _, game_manifest) = client
        .fetch_game_room_state_and_manifest(topic, &room_id)
        .await?
        .expect("verified remote game");
    assert_eq!(source, game_bucket);
    assert_eq!(game_manifest.title, "remote game");
    assert_eq!(client_node.docs().list().await?.count().await, 0);
    client.shutdown().await;
    publisher.shutdown().await;
    client_docs.shutdown().await;
    publisher_docs.shutdown().await;
    client_node.shutdown().await?;
    publisher_node.shutdown().await?;
    Ok(())
}

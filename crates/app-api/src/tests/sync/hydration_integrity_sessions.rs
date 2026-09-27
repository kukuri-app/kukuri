//! reaction・live session・game room の反映(hydration)が、docs の record の申告値をそのまま信用しないことを固定する。
//!
//! 投稿(`hydration_integrity.rs`、#1248)と同じ前提に立つ。public topic の replica は topic id を知る誰もが書ける。
//! `reactions/<target>/<reaction id>/state`・`sessions/live/<id>/state`・`sessions/game/<id>/state` は署名を持たない。

use super::*;
use kukuri_core::{
    GameRoomKind, GameRoomManifestBlobV1, GameRoomStateDocV1, GameRoomStatus,
    LiveSessionManifestBlobV1, LiveSessionStateDocV1, LiveSessionStatus, ManifestBlobRef,
    build_game_session_envelope, build_live_session_envelope,
};

async fn write_garbage(docs_sync: &dyn DocsSync, replica: &ReplicaId, key: String) {
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key,
                value: serde_json::json!({ "not": "a state doc" }),
            },
        )
        .await
        .expect("write an unreadable record");
}

/// 反映が空の状態から始める viewer(新規の参加者)。
fn fresh_viewer(
    docs_sync: Arc<MemoryDocsSync>,
    blob_service: Arc<MemoryBlobService>,
) -> AppService {
    let store = Arc::new(MemoryStore::default());
    app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        generate_keys(),
    )
}

// (reaction c) reaction として読めない record が 1 件あっても、topic のタイムラインを止めない。
#[tokio::test]
async fn unreadable_reaction_record_does_not_break_the_timeline_of_the_topic() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let topic = TopicId::new("kukuri:topic:integrity-reaction-unreadable");
    let replica = topic_replica_id(topic.as_str());
    let honest = persist_test_post(
        docs_sync.as_ref(),
        None,
        &generate_keys(),
        &topic,
        PayloadRef::InlineText {
            text: "an honest post next to garbage".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    write_garbage(
        docs_sync.as_ref(),
        &replica,
        stable_key("reactions", "0000-target/0000-garbage/state"),
    )
    .await;

    let viewer = fresh_viewer(docs_sync, Arc::new(MemoryBlobService::default()));
    let view = viewer
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("an unreadable reaction record must not fail the timeline of the whole topic");
    assert!(
        view.items
            .iter()
            .any(|item| item.object_id == honest.id.as_str()),
        "an unreadable reaction record hid the honest posts of the topic"
    );
}

/// `signer` があれば、manifest に署名した envelope を `envelopes/<id>` へ置いて state から指す(修正後の書き込みと同じ形)。
/// `None` なら、署名で裏づけられない state だけを置く。
async fn write_live_session(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    signer: Option<&KukuriKeys>,
    manifest: &LiveSessionManifestBlobV1,
) {
    let stored = store_manifest_blob(blob_service, manifest, LIVE_MANIFEST_MIME)
        .await
        .expect("store manifest");
    let last_envelope_id = match signer {
        Some(signer) => {
            let envelope = build_live_session_envelope(
                signer,
                &manifest.topic_id,
                manifest.session_id.as_str(),
                manifest,
            )
            .expect("sign the manifest");
            persist_session_envelope(docs_sync, replica, &envelope)
                .await
                .expect("write envelope");
            envelope.id
        }
        None => EnvelopeId::from("0".repeat(64).as_str()),
    };
    let state = LiveSessionStateDocV1 {
        session_id: manifest.session_id.clone(),
        topic_id: manifest.topic_id.clone(),
        channel_id: manifest.channel_id.clone(),
        owner_pubkey: manifest.owner_pubkey.clone(),
        created_at: manifest.started_at,
        updated_at: Utc::now().timestamp_millis(),
        status: manifest.status.clone(),
        current_manifest: ManifestBlobRef {
            hash: stored.hash,
            mime: stored.mime,
            bytes: stored.bytes,
        },
        last_envelope_id,
    };
    persist_live_session_state(docs_sync, replica, &state)
        .await
        .expect("write live state");
}

fn live_manifest(
    session_id: &str,
    topic: &TopicId,
    channel_id: Option<&ChannelId>,
    owner: &str,
    status: LiveSessionStatus,
) -> LiveSessionManifestBlobV1 {
    LiveSessionManifestBlobV1 {
        session_id: session_id.into(),
        revision: 1,
        topic_id: topic.clone(),
        channel_id: channel_id.cloned(),
        owner_pubkey: Pubkey::from(owner),
        title: "a session the owner never started".into(),
        description: String::new(),
        status,
        started_at: Utc::now().timestamp_millis(),
        ended_at: None,
    }
}

// (live a-1) 攻撃者が、他人の pubkey を owner にした live session を置く。
#[tokio::test]
async fn live_session_that_claims_another_owner_is_not_listed_as_hosted_by_that_owner() {
    let (app, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-live-forged-owner");
    let replica = topic_replica_id(topic.as_str());
    let victim = generate_keys().public_key_hex();
    write_live_session(
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &replica,
        None,
        &live_manifest(
            "live-forged",
            &topic,
            None,
            &victim,
            LiveSessionStatus::Live,
        ),
    )
    .await;

    let sessions = app
        .list_live_sessions(topic.as_str())
        .await
        .expect("live sessions");
    let hosted_by_victim = sessions
        .iter()
        .filter(|session| session.host_pubkey == victim)
        .map(|session| session.title.clone())
        .collect::<Vec<_>>();
    assert!(
        hosted_by_victim.is_empty(),
        "a live session nobody signed is listed as hosted by the victim: {hosted_by_victim:?}"
    );
}

// (live c) live session として読めない record が 1 件あっても、一覧と、同じ replica の他の session を止めない。
#[tokio::test]
async fn unreadable_live_session_record_does_not_break_the_list_of_the_topic() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let topic = TopicId::new("kukuri:topic:integrity-live-unreadable");
    let replica = topic_replica_id(topic.as_str());
    write_garbage(
        docs_sync.as_ref(),
        &replica,
        stable_key("sessions/live", "0000-garbage/state"),
    )
    .await;
    let owner_keys = generate_keys();
    let owner = owner_keys.public_key_hex();
    let honest_id = format!("honest-1-{}", &owner[..16]);
    write_live_session(
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &replica,
        Some(&owner_keys),
        &live_manifest(
            honest_id.as_str(),
            &topic,
            None,
            &owner,
            LiveSessionStatus::Live,
        ),
    )
    .await;

    let viewer = fresh_viewer(docs_sync, blob_service);
    let sessions = viewer
        .list_live_sessions(topic.as_str())
        .await
        .expect("an unreadable record must not fail the live session list of the whole topic");
    assert!(
        sessions
            .iter()
            .any(|session| session.session_id == honest_id),
        "an unreadable record hid the other live sessions of the topic"
    );
}

/// `write_live_session` と同じ形。
async fn write_game_room(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    signer: Option<&KukuriKeys>,
    manifest: &GameRoomManifestBlobV1,
) {
    let stored = store_manifest_blob(blob_service, manifest, GAME_MANIFEST_MIME)
        .await
        .expect("store manifest");
    let last_envelope_id = match signer {
        Some(signer) => {
            let envelope = build_game_session_envelope(
                signer,
                &manifest.topic_id,
                manifest.room_id.as_str(),
                manifest,
            )
            .expect("sign the manifest");
            persist_session_envelope(docs_sync, replica, &envelope)
                .await
                .expect("write envelope");
            envelope.id
        }
        None => EnvelopeId::from("0".repeat(64).as_str()),
    };
    let state = GameRoomStateDocV1 {
        room_id: manifest.room_id.clone(),
        topic_id: manifest.topic_id.clone(),
        channel_id: manifest.channel_id.clone(),
        owner_pubkey: manifest.owner_pubkey.clone(),
        created_at: manifest.updated_at,
        updated_at: manifest.updated_at,
        status: manifest.status.clone(),
        current_manifest: ManifestBlobRef {
            hash: stored.hash,
            mime: stored.mime,
            bytes: stored.bytes,
        },
        last_envelope_id,
    };
    persist_game_room_state(docs_sync, replica, &state)
        .await
        .expect("write game state");
}

fn game_manifest(
    room_id: &str,
    topic: &TopicId,
    channel_id: Option<&ChannelId>,
    owner: &str,
) -> GameRoomManifestBlobV1 {
    GameRoomManifestBlobV1 {
        room_id: room_id.into(),
        score_revision: Some(1),
        topic_id: topic.clone(),
        channel_id: channel_id.cloned(),
        owner_pubkey: Pubkey::from(owner),
        title: "a room the owner never opened".into(),
        description: String::new(),
        status: GameRoomStatus::Waiting,
        phase_label: None,
        participants: Vec::new(),
        scores: Vec::new(),
        room_kind: GameRoomKind::ScoreGame,
        metaverse: None,
        updated_at: Utc::now().timestamp_millis(),
    }
}

// (game a) 攻撃者が、他人の pubkey を owner にした game room を置く。
#[tokio::test]
async fn game_room_that_claims_another_owner_is_not_listed_as_hosted_by_that_owner() {
    let (app, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-game-forged-owner");
    let replica = topic_replica_id(topic.as_str());
    let victim = generate_keys().public_key_hex();
    write_game_room(
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &replica,
        None,
        &game_manifest("game-forged", &topic, None, &victim),
    )
    .await;

    let rooms = app
        .list_game_rooms(topic.as_str())
        .await
        .expect("game rooms");
    let hosted_by_victim = rooms
        .iter()
        .filter(|room| room.host_pubkey == victim)
        .map(|room| room.title.clone())
        .collect::<Vec<_>>();
    assert!(
        hosted_by_victim.is_empty(),
        "a game room nobody signed is listed as hosted by the victim: {hosted_by_victim:?}"
    );
}

// (game b) public replica に置いた game room が private channel の id を申告する。
#[tokio::test]
async fn game_room_in_the_public_replica_that_claims_a_private_channel_is_not_listed_in_the_channel()
 {
    let (app, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-game-channel-claim");
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: topic.clone(),
            label: "members only".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    // 攻撃者は自分の鍵で正しく署名し、id も自分の pubkey に結び付ける。署名の検証だけでは防げない。
    let attacker_keys = generate_keys();
    let attacker = attacker_keys.public_key_hex();
    let injected_id = format!("injected-1-{}", &attacker[..16]);
    let public_replica = topic_replica_id(topic.as_str());
    write_game_room(
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &public_replica,
        Some(&attacker_keys),
        &game_manifest(injected_id.as_str(), &topic, Some(&channel_id), &attacker),
    )
    .await;
    hydrate_game_room_from_key(
        &app.services,
        topic.as_str(),
        &public_replica,
        stable_key("sessions/game", &format!("{injected_id}/state")).as_str(),
    )
    .await
    .expect("hydrate game room");

    let rooms = app
        .list_game_rooms_scoped(
            topic.as_str(),
            TimelineScope::Channel {
                channel_id: channel_id.clone(),
            },
        )
        .await
        .expect("channel game rooms");
    assert!(
        rooms.iter().all(|room| room.room_id != injected_id),
        "a game room read from the public replica was listed in the private channel"
    );
}

// (game c) game room として読めない record が 1 件あっても、一覧と、同じ replica の他の room を止めない。
#[tokio::test]
async fn unreadable_game_room_record_does_not_break_the_list_of_the_topic() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let topic = TopicId::new("kukuri:topic:integrity-game-unreadable");
    let replica = topic_replica_id(topic.as_str());
    write_garbage(
        docs_sync.as_ref(),
        &replica,
        stable_key("sessions/game", "0000-garbage/state"),
    )
    .await;
    let owner_keys = generate_keys();
    let owner = owner_keys.public_key_hex();
    let honest_id = format!("honest-1-{}", &owner[..16]);
    write_game_room(
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &replica,
        Some(&owner_keys),
        &game_manifest(honest_id.as_str(), &topic, None, &owner),
    )
    .await;

    let viewer = fresh_viewer(docs_sync, blob_service);
    let rooms = viewer
        .list_game_rooms(topic.as_str())
        .await
        .expect("an unreadable record must not fail the game room list of the whole topic");
    assert!(
        rooms.iter().any(|room| room.room_id == honest_id),
        "an unreadable record hid the other game rooms of the topic"
    );
}

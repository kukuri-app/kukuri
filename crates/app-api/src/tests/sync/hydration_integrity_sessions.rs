//! reaction・live session・game room の反映(hydration)が、docs の record の申告値をそのまま信用しないことを固定する。
//!
//! 投稿(`hydration_integrity.rs`、#1248)と同じ前提に立つ。public topic の replica は topic id を知る誰もが書ける。
//! `reactions/<target>/<reaction id>/state`・`sessions/live/<id>/state`・`sessions/game/<id>/state` は署名を持たない。

use super::*;
use kukuri_core::{
    GameRoomKind, GameRoomManifestBlobV1, GameRoomStateDocV1, GameRoomStatus,
    LiveSessionManifestBlobV1, LiveSessionStateDocV1, LiveSessionStatus, ManifestBlobRef,
    ObjectStatus, ReactionDocV1, ReactionKeyV1, build_game_session_envelope,
    build_live_session_envelope, build_reaction_envelope, deterministic_reaction_id,
    parse_reaction,
};

fn thumbs_up() -> ReactionKeyV1 {
    ReactionKeyV1::Emoji {
        emoji: "👍".into()
    }
}

/// `signer` が署名した reaction の envelope と、そこから作った doc を返す。
fn signed_reaction(
    signer: &KukuriKeys,
    claimed_author: &Pubkey,
    topic: &TopicId,
    channel_id: Option<&ChannelId>,
    replica: &ReplicaId,
    target: &EnvelopeId,
) -> (KukuriEnvelope, ReactionDocV1) {
    let key = thumbs_up();
    let reaction_id = deterministic_reaction_id(
        replica,
        target,
        claimed_author,
        key.normalized_key().expect("normalized key").as_str(),
    );
    let envelope = build_reaction_envelope(
        signer,
        topic,
        channel_id,
        target,
        key,
        &reaction_id,
        ObjectStatus::Active,
    )
    .expect("reaction envelope");
    let doc = parse_reaction(&envelope)
        .expect("parse reaction")
        .expect("reaction doc");
    (envelope, doc)
}

async fn write_reaction_entries(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope: Option<&KukuriEnvelope>,
    doc: &ReactionDocV1,
) {
    let base = format!(
        "{}/{}",
        doc.target_object_id.as_str(),
        doc.reaction_id.as_str()
    );
    if let Some(envelope) = envelope {
        docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key("reactions", &format!("{base}/envelope")),
                    value: serde_json::to_value(envelope).expect("envelope json"),
                },
            )
            .await
            .expect("write reaction envelope");
    }
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("reactions", &format!("{base}/state")),
                value: serde_json::to_value(doc).expect("state json"),
            },
        )
        .await
        .expect("write reaction state");
}

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

// (reaction a-1) 攻撃者が自分の鍵で署名した envelope を置き、state の author だけを viewer 自身の pubkey にする。
// viewer が付けていない reaction を「自分の reaction」として表示してはならない。
#[tokio::test]
async fn reaction_state_whose_author_differs_from_the_signed_envelope_is_not_shown_as_that_author()
{
    let (app, _, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-reaction-forged-author");
    let replica = topic_replica_id(topic.as_str());
    let target = EnvelopeId::from(
        app.create_post(topic.as_str(), "a post", None)
            .await
            .expect("create post")
            .as_str(),
    );
    let victim = Pubkey::from(app.current_author_pubkey());
    let attacker_keys = generate_keys();
    let (envelope, mut doc) =
        signed_reaction(&attacker_keys, &victim, &topic, None, &replica, &target);
    doc.author_pubkey = victim.clone();
    write_reaction_entries(docs_sync.as_ref(), &replica, Some(&envelope), &doc).await;
    sleep(Duration::from_millis(300)).await;
    // docs event を取りこぼした場合に備えて、hint と同じ反映も直接通す。
    hydrate_subscription_hint(
        &app.services,
        topic.as_str(),
        &replica,
        &GossipHint::TopicObjectsChanged {
            topic_id: topic.clone(),
            objects: vec![HintObjectRef {
                object_id: target.as_str().to_string(),
                object_kind: "reaction".into(),
            }],
        },
    )
    .await
    .expect("hydrate reactions");

    let view = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == target.as_str())
        .expect("the post is listed");
    assert!(
        item.my_reactions.is_empty(),
        "a reaction the viewer never made is shown as the viewer's own: {:?}",
        item.my_reactions
    );
}

// (reaction a-2) envelope を置かず、state だけで他人の reaction を装う。署名で裏づけられない state は数えない。
#[tokio::test]
async fn reaction_state_without_a_signed_envelope_is_not_counted() {
    let (app, _, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-reaction-state-only");
    let replica = topic_replica_id(topic.as_str());
    let target = EnvelopeId::from(
        app.create_post(topic.as_str(), "a post", None)
            .await
            .expect("create post")
            .as_str(),
    );
    let victim = Pubkey::from(generate_keys().public_key_hex());
    // 形を借りるための envelope。replica へは置かない。
    let (_, mut doc) = signed_reaction(&generate_keys(), &victim, &topic, None, &replica, &target);
    doc.author_pubkey = victim;
    write_reaction_entries(docs_sync.as_ref(), &replica, None, &doc).await;
    sleep(Duration::from_millis(300)).await;
    hydrate_subscription_hint(
        &app.services,
        topic.as_str(),
        &replica,
        &GossipHint::TopicObjectsChanged {
            topic_id: topic.clone(),
            objects: vec![HintObjectRef {
                object_id: target.as_str().to_string(),
                object_kind: "reaction".into(),
            }],
        },
    )
    .await
    .expect("hydrate reactions");

    let view = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == target.as_str())
        .expect("the post is listed");
    assert!(
        item.reaction_summary.is_empty(),
        "a reaction state that no signed envelope backs was counted: {:?}",
        item.reaction_summary
    );
}

// (reaction b) public replica に置いた reaction が、private channel の投稿と channel id を申告する。
#[tokio::test]
async fn reaction_in_the_public_replica_that_claims_a_private_channel_is_not_counted_in_the_channel()
 {
    let (app, _, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-reaction-channel-claim");
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: topic.clone(),
            label: "members only".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let scope = TimelineScope::Channel {
        channel_id: channel_id.clone(),
    };
    let target = EnvelopeId::from(
        app.create_post_in_channel(
            topic.as_str(),
            ChannelRef::PrivateChannel {
                channel_id: channel_id.clone(),
            },
            "from a member",
            None,
        )
        .await
        .expect("create member post")
        .as_str(),
    );
    let attacker_keys = generate_keys();
    let attacker = Pubkey::from(attacker_keys.public_key_hex());
    let public_replica = topic_replica_id(topic.as_str());
    let (envelope, doc) = signed_reaction(
        &attacker_keys,
        &attacker,
        &topic,
        Some(&channel_id),
        &public_replica,
        &target,
    );
    write_reaction_entries(docs_sync.as_ref(), &public_replica, Some(&envelope), &doc).await;
    sleep(Duration::from_millis(300)).await;
    hydrate_subscription_hint(
        &app.services,
        topic.as_str(),
        &public_replica,
        &GossipHint::TopicObjectsChanged {
            topic_id: topic.clone(),
            objects: vec![HintObjectRef {
                object_id: target.as_str().to_string(),
                object_kind: "reaction".into(),
            }],
        },
    )
    .await
    .expect("hydrate reactions");

    let view = app
        .list_timeline_scoped(topic.as_str(), scope, None, 20)
        .await
        .expect("private timeline");
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == target.as_str())
        .expect("the member post is listed");
    assert!(
        item.reaction_summary.is_empty(),
        "a reaction read from the public replica was counted on a private channel post: {:?}",
        item.reaction_summary
    );
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

// (live a-2) owner が開いている live session の state を、第三者が「終了」に書き換える。
#[tokio::test]
async fn live_session_is_not_ended_by_a_state_the_owner_did_not_write() {
    let (app, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-live-forced-end");
    let replica = topic_replica_id(topic.as_str());
    let session_id = app
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "the owner's session".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");
    let owner = app.current_author_pubkey();
    // 攻撃者は owner の鍵を持たない。state と manifest を同じ key へ書くだけ。
    write_live_session(
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &replica,
        None,
        &live_manifest(
            session_id.as_str(),
            &topic,
            None,
            &owner,
            LiveSessionStatus::Ended,
        ),
    )
    .await;
    sleep(Duration::from_millis(300)).await;
    hydrate_subscription_event(
        &app.services,
        topic.as_str(),
        &replica,
        stable_key("sessions/live", &format!("{session_id}/state")).as_str(),
    )
    .await
    .expect("hydrate live session");

    let sessions = app
        .list_live_sessions(topic.as_str())
        .await
        .expect("live sessions");
    let session = sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("the owner's session is listed");
    assert_eq!(
        session.status,
        LiveSessionStatus::Live,
        "a state the owner did not write ended the owner's live session"
    );
    assert_eq!(session.title, "the owner's session");
}

// (live b) public replica に置いた live session が private channel の id を申告する。
#[tokio::test]
async fn live_session_in_the_public_replica_that_claims_a_private_channel_is_not_listed_in_the_channel()
 {
    let (app, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-live-channel-claim");
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
    write_live_session(
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &public_replica,
        Some(&attacker_keys),
        &live_manifest(
            injected_id.as_str(),
            &topic,
            Some(&channel_id),
            &attacker,
            LiveSessionStatus::Live,
        ),
    )
    .await;
    hydrate_subscription_event(
        &app.services,
        topic.as_str(),
        &public_replica,
        stable_key("sessions/live", &format!("{injected_id}/state")).as_str(),
    )
    .await
    .expect("hydrate live session");

    let sessions = app
        .list_live_sessions_scoped(
            topic.as_str(),
            TimelineScope::Channel {
                channel_id: channel_id.clone(),
            },
        )
        .await
        .expect("channel live sessions");
    assert!(
        sessions
            .iter()
            .all(|session| session.session_id != injected_id),
        "a live session read from the public replica was listed in the private channel"
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

//! reaction・live session・game room の検証(#1252)の契約。修正前の再現は `hydration_integrity_sessions.rs`。
//!
//! 正しい record がこれまでどおり反映されること(INVAR-1)と、署名が正しくても受け入れない場合(別の鍵による上書き、
//! 別の reaction の key への置き直し、古い envelope の再掲)を固定する。

use super::shadowing_docs::ShadowingDocsSync;
use super::*;
use kukuri_core::{
    GameRoomKind, GameRoomManifestBlobV1, GameRoomStateDocV1, GameRoomStatus,
    LiveSessionManifestBlobV1, LiveSessionStateDocV1, LiveSessionStatus, ManifestBlobRef,
    ReactionKeyV1, SpatialContextV1, build_game_session_envelope, build_live_session_envelope,
};

fn thumbs_up() -> ReactionKeyV1 {
    ReactionKeyV1::Emoji {
        emoji: "👍".into()
    }
}

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

async fn reaction_hint(app: &AppService, topic: &TopicId, target: &str) {
    hydrate_subscription_hint(
        &app.services,
        topic.as_str(),
        &topic_replica_id(topic.as_str()),
        &GossipHint::TopicObjectsChanged {
            topic_id: topic.clone(),
            objects: vec![HintObjectRef {
                object_id: target.to_string(),
                object_kind: "reaction".into(),
                docs_author: None,
            }],
        },
    )
    .await
    .expect("hydrate reactions");
}

async fn reaction_count(app: &AppService, topic: &TopicId, target: &str) -> usize {
    app.list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline")
        .items
        .iter()
        .find(|item| item.object_id == target)
        .expect("the post is listed")
        .reaction_summary
        .iter()
        .map(|summary| summary.count)
        .sum()
}

async fn record_value(docs_sync: &dyn DocsSync, replica: &ReplicaId, key: &str) -> Vec<u8> {
    docs_sync
        .query_replica(replica, DocQuery::Exact(key.to_string()))
        .await
        .expect("query")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("record {key}"))
        .value
}

async fn reaction_keys_for(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    target: &str,
) -> (String, String) {
    let keys = docs_sync
        .query_replica(
            replica,
            DocQuery::Prefix(stable_key("reactions", &format!("{target}/"))),
        )
        .await
        .expect("query reactions")
        .into_iter()
        .map(|record| record.key)
        .collect::<Vec<_>>();
    let find = |suffix: &str| {
        keys.iter()
            .find(|key| key.ends_with(suffix))
            .unwrap_or_else(|| panic!("reaction {suffix} key"))
            .clone()
    };
    (find("/envelope"), find("/state"))
}

// INVAR-1: 別の参加者の正しい reaction は、走査でも、envelope の event だけでも反映される。
#[tokio::test]
async fn honest_reaction_is_counted_by_another_viewer() {
    let (author, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-honest-reaction");
    let replica = topic_replica_id(topic.as_str());
    let target = author
        .create_post(topic.as_str(), "a post", None)
        .await
        .expect("create post");
    author
        .toggle_reaction(topic.as_str(), target.as_str(), thumbs_up(), None)
        .await
        .expect("toggle reaction");

    let scanning_viewer = fresh_viewer(docs_sync.clone(), blob_service.clone());
    assert_eq!(
        reaction_count(&scanning_viewer, &topic, &target).await,
        1,
        "the full scan must project an honest reaction"
    );

    let event_viewer = fresh_viewer(docs_sync.clone(), blob_service);
    let (envelope_key, _) = reaction_keys_for(docs_sync.as_ref(), &replica, &target).await;
    let hydrated = hydrate_subscription_event(
        &event_viewer.services,
        topic.as_str(),
        &replica,
        envelope_key.as_str(),
    )
    .await
    .expect("hydrate the envelope event");
    assert_eq!(
        hydrated, 1,
        "the envelope event alone must project the reaction"
    );
}

// 取り消した reaction の古い署名つき envelope を置き直しても、reaction は復活しない。
#[tokio::test]
async fn replayed_older_reaction_envelope_does_not_revive_a_removed_reaction() {
    let (app, _, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-reaction-replay");
    let replica = topic_replica_id(topic.as_str());
    let target = app
        .create_post(topic.as_str(), "a post", None)
        .await
        .expect("create post");
    app.toggle_reaction(topic.as_str(), target.as_str(), thumbs_up(), None)
        .await
        .expect("add reaction");
    let (envelope_key, _) = reaction_keys_for(docs_sync.as_ref(), &replica, &target).await;
    let active_envelope: serde_json::Value = serde_json::from_slice(
        &record_value(docs_sync.as_ref(), &replica, envelope_key.as_str()).await,
    )
    .expect("envelope json");
    // envelope の created_at は ms。取り消しの署名時刻が必ず後になるようにする。
    sleep(Duration::from_millis(5)).await;
    app.toggle_reaction(topic.as_str(), target.as_str(), thumbs_up(), None)
        .await
        .expect("remove reaction");
    assert_eq!(reaction_count(&app, &topic, &target).await, 0);

    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: envelope_key,
                value: active_envelope,
            },
        )
        .await
        .expect("replay the older envelope");
    reaction_hint(&app, &topic, &target).await;
    assert_eq!(
        reaction_count(&app, &topic, &target).await,
        0,
        "an older signed envelope revived a reaction its author removed"
    );
}

// 正しく署名された reaction でも、別の投稿の key へ置き直したものは数えない(reaction id は replica・対象・著者・key から決まる)。
#[tokio::test]
async fn signed_reaction_moved_to_another_target_key_is_not_counted() {
    let (app, _, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-reaction-moved");
    let replica = topic_replica_id(topic.as_str());
    let liked = app
        .create_post(topic.as_str(), "liked", None)
        .await
        .expect("create post");
    let other = app
        .create_post(topic.as_str(), "not liked", None)
        .await
        .expect("create post");
    app.toggle_reaction(topic.as_str(), liked.as_str(), thumbs_up(), None)
        .await
        .expect("add reaction");
    let (envelope_key, _) = reaction_keys_for(docs_sync.as_ref(), &replica, &liked).await;
    let envelope: serde_json::Value = serde_json::from_slice(
        &record_value(docs_sync.as_ref(), &replica, envelope_key.as_str()).await,
    )
    .expect("envelope json");
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: envelope_key.replace(liked.as_str(), other.as_str()),
                value: envelope,
            },
        )
        .await
        .expect("copy the envelope under another target");
    reaction_hint(&app, &topic, &other).await;
    assert_eq!(
        reaction_count(&app, &topic, &other).await,
        0,
        "a reaction signed for another post was counted"
    );
    assert_eq!(reaction_count(&app, &topic, &liked).await, 1);
}

// INVAR-1: 正しい live session は、別の参加者の一覧に host つきで出て、終了も反映される。
#[tokio::test]
async fn honest_live_session_is_listed_and_ended_for_another_viewer() {
    let (owner, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-honest-live");
    let session_id = owner
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "honest live".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");

    let viewer = fresh_viewer(docs_sync.clone(), blob_service.clone());
    let sessions = viewer
        .list_live_sessions(topic.as_str())
        .await
        .expect("live sessions");
    let session = sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("the honest session is listed");
    assert_eq!(session.host_pubkey, owner.current_author_pubkey());
    assert_eq!(session.status, LiveSessionStatus::Live);

    owner
        .end_live_session(topic.as_str(), session_id.as_str())
        .await
        .expect("end live session");
    let hydrated = hydrate_subscription_event(
        &viewer.services,
        topic.as_str(),
        &topic_replica_id(topic.as_str()),
        stable_key("sessions/live", &format!("{session_id}/state")).as_str(),
    )
    .await
    .expect("hydrate the end");
    assert_eq!(hydrated, 1);
    let sessions = viewer
        .list_live_sessions(topic.as_str())
        .await
        .expect("live sessions");
    assert_eq!(
        sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .expect("listed")
            .status,
        LiveSessionStatus::Ended
    );
}

/// `signer` が署名した manifest の envelope・blob・state を、`persist_live_session_manifest` と同じ形で置く。
async fn write_signed_live_session(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    signer: &KukuriKeys,
    manifest: &LiveSessionManifestBlobV1,
) {
    let envelope = build_live_session_envelope(
        signer,
        &manifest.topic_id,
        manifest.session_id.as_str(),
        manifest,
    )
    .expect("sign the manifest");
    let stored = store_manifest_blob(blob_service, manifest, LIVE_MANIFEST_MIME)
        .await
        .expect("store manifest");
    persist_session_envelope(docs_sync, replica, &envelope)
        .await
        .expect("write envelope");
    persist_live_session_state(
        docs_sync,
        replica,
        &LiveSessionStateDocV1 {
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
            last_envelope_id: envelope.id,
        },
    )
    .await
    .expect("write state");
}

// 別の鍵で正しく署名した state を同じ session id の key へ置いても、owner の session を上書きできない。
// (a) 攻撃者自身を owner とする manifest: id が攻撃者の pubkey と結び付かない。
// (b) owner を被害者とする manifest: 署名者が owner ではない。
#[tokio::test]
async fn live_session_is_not_taken_over_by_a_state_signed_with_another_key() {
    let (owner, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-live-takeover");
    let replica = topic_replica_id(topic.as_str());
    let session_id = owner
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "the owner's session".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");
    let attacker_keys = generate_keys();
    for claimed_owner in [
        attacker_keys.public_key_hex(),
        owner.current_author_pubkey(),
    ] {
        write_signed_live_session(
            docs_sync.as_ref(),
            blob_service.as_ref(),
            &replica,
            &attacker_keys,
            &LiveSessionManifestBlobV1 {
                session_id: session_id.clone(),
                topic_id: topic.clone(),
                channel_id: None,
                owner_pubkey: Pubkey::from(claimed_owner.as_str()),
                title: "taken over".into(),
                description: String::new(),
                status: LiveSessionStatus::Ended,
                started_at: Utc::now().timestamp_millis(),
                ended_at: Some(Utc::now().timestamp_millis()),
            },
        )
        .await;
        let hydrated = hydrate_subscription_event(
            &owner.services,
            topic.as_str(),
            &replica,
            stable_key("sessions/live", &format!("{session_id}/state")).as_str(),
        )
        .await
        .expect("hydrate");
        assert_eq!(hydrated, 0, "a state signed with another key was projected");
        let sessions = owner
            .list_live_sessions(topic.as_str())
            .await
            .expect("live sessions");
        let session = sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .expect("the owner's session stays listed");
        assert_eq!(session.host_pubkey, owner.current_author_pubkey());
        assert_eq!(session.status, LiveSessionStatus::Live);
        assert_eq!(session.title, "the owner's session");
    }
    // 操作が読む state も検証を通す。in-memory の docs は key ごとに 1 record なので、上書き後は検証に通る state が無い
    // (iroh-docs では owner の entry が docs author ごとに残り、上限つきの読み出しで選ばれる)。
    assert!(
        owner
            .fetch_live_session_state_and_manifest(topic.as_str(), session_id.as_str())
            .await
            .expect("fetch")
            .is_none(),
        "an operation read a state that does not verify"
    );
}

// INVAR-1: 正しい ScoreGame は、別の参加者の一覧に出て、更新も反映される。
#[tokio::test]
async fn honest_score_game_is_listed_and_updated_for_another_viewer() {
    let (owner, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-honest-game");
    let room_id = owner
        .create_game_room(
            topic.as_str(),
            CreateGameRoomInput {
                title: "honest game".into(),
                description: String::new(),
                participants: vec!["a".into(), "b".into()],
            },
        )
        .await
        .expect("create game room");
    let viewer = fresh_viewer(docs_sync, blob_service);
    let rooms = viewer
        .list_game_rooms(topic.as_str())
        .await
        .expect("game rooms");
    let room = rooms
        .iter()
        .find(|room| room.room_id == room_id)
        .expect("the honest room is listed");
    assert_eq!(room.host_pubkey, owner.current_author_pubkey());

    owner
        .update_game_room(
            topic.as_str(),
            room_id.as_str(),
            UpdateGameRoomInput {
                status: GameRoomStatus::Running,
                phase_label: Some("round 1".into()),
                scores: vec![
                    GameScoreView {
                        participant_id: "participant-1".into(),
                        label: "a".into(),
                        score: 3,
                    },
                    GameScoreView {
                        participant_id: "participant-2".into(),
                        label: "b".into(),
                        score: 0,
                    },
                ],
            },
        )
        .await
        .expect("update game room");
    let hydrated = hydrate_subscription_event(
        &viewer.services,
        topic.as_str(),
        &topic_replica_id(topic.as_str()),
        stable_key("sessions/game", &format!("{room_id}/state")).as_str(),
    )
    .await
    .expect("hydrate the update");
    assert_eq!(hydrated, 1);
    let rooms = viewer
        .list_game_rooms(topic.as_str())
        .await
        .expect("game rooms");
    let room = rooms
        .iter()
        .find(|room| room.room_id == room_id)
        .expect("listed");
    assert_eq!(room.status, GameRoomStatus::Running);
}

// metaverse room は owner の署名を要求しない(訪問者も chat で manifest を書く)。その代わり、id が Spatial Context と owner から
// 決まる値でなければ反映せず、合っていても、署名つきの Dome Instance が無ければ一覧に出ない。
#[tokio::test]
async fn forged_metaverse_room_is_not_listed_as_the_victims_dome() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-forged-dome");
    let replica = topic_replica_id(topic.as_str());
    let victim = Pubkey::from(generate_keys().public_key_hex().as_str());
    let context = SpatialContextV1::Topic {
        topic_id: topic.clone(),
    };
    let bound_id = {
        let hash = kukuri_core::blob_hash(format!(
            "dome-instance:{}:{}",
            context.canonical_id(),
            victim.as_str()
        ));
        format!("dome-{}", &hash.as_str()[..24])
    };
    // 形を借りるために、攻撃者が自分の Dome を別の topic に作り、その metaverse の state を写す。
    let attacker_keys = generate_keys();
    let (shape_app, _, _, _) = local_app_with_memory_services();
    let shape_topic = "kukuri:topic:integrity-contract-forged-dome-shape";
    let shape_room_id = shape_app
        .create_metaverse_room(
            shape_topic,
            CreateMetaverseRoomInput {
                title: "shape".into(),
                description: String::new(),
                max_peers: None,
            },
        )
        .await
        .expect("create the shape Dome");
    let (_, _, shape_manifest) = shape_app
        .fetch_game_room_state_and_manifest(shape_topic, shape_room_id.as_str())
        .await
        .expect("fetch the shape Dome")
        .expect("the shape Dome");
    for room_id in ["dome-000000000000000000000000".to_string(), bound_id] {
        let mut metaverse = shape_manifest.metaverse.clone().expect("metaverse state");
        metaverse.instance_id = room_id.clone();
        metaverse.spatial_context = context.clone();
        let manifest = GameRoomManifestBlobV1 {
            room_id: room_id.clone(),
            topic_id: topic.clone(),
            channel_id: None,
            owner_pubkey: victim.clone(),
            title: "a Dome the victim never opened".into(),
            description: String::new(),
            status: GameRoomStatus::Waiting,
            phase_label: None,
            participants: Vec::new(),
            scores: Vec::new(),
            room_kind: GameRoomKind::MetaverseRoom,
            metaverse: Some(metaverse),
            updated_at: Utc::now().timestamp_millis(),
        };
        let envelope = build_game_session_envelope(
            &attacker_keys,
            &manifest.topic_id,
            manifest.room_id.as_str(),
            &manifest,
        )
        .expect("sign the manifest");
        let stored = store_manifest_blob(blob_service.as_ref(), &manifest, GAME_MANIFEST_MIME)
            .await
            .expect("store manifest");
        persist_session_envelope(docs_sync.as_ref(), &replica, &envelope)
            .await
            .expect("write envelope");
        persist_game_room_state(
            docs_sync.as_ref(),
            &replica,
            &GameRoomStateDocV1 {
                room_id: room_id.clone(),
                topic_id: topic.clone(),
                channel_id: None,
                owner_pubkey: victim.clone(),
                created_at: manifest.updated_at,
                updated_at: manifest.updated_at,
                status: GameRoomStatus::Waiting,
                current_manifest: ManifestBlobRef {
                    hash: stored.hash,
                    mime: stored.mime,
                    bytes: stored.bytes,
                },
                last_envelope_id: envelope.id,
            },
        )
        .await
        .expect("write state");
        hydrate_game_room_from_key(
            &app.services,
            topic.as_str(),
            &replica,
            stable_key("sessions/game", &format!("{room_id}/state")).as_str(),
        )
        .await
        .expect("hydrate");
    }
    let rows = LiveGameProjectionStore::list_topic_game_rooms(store.as_ref(), topic.as_str())
        .await
        .expect("rows");
    assert!(
        rows.iter()
            .all(|row| row.room_id != "dome-000000000000000000000000"),
        "a Dome whose id is not bound to its owner was projected"
    );
    let rooms = app
        .list_game_rooms(topic.as_str())
        .await
        .expect("game rooms");
    assert!(
        rooms.iter().all(|room| room.host_pubkey != victim.as_str()),
        "a Dome without a signed Dome Instance was listed as the victim's"
    );
}

fn app_over(docs_sync: Arc<ShadowingDocsSync>, blob_service: Arc<MemoryBlobService>) -> AppService {
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

// TR-8 / AC-5: 同じ key の先頭に読めない record(別の docs author の entry)が並んでいても、検証に通る record から反映する。
// 全件走査(新規の viewer の一覧)と、key 指定の個別反映の両方で確かめる。
#[tokio::test]
async fn invalid_records_placed_before_the_valid_ones_do_not_hide_sessions_and_reactions() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let owner = app_over(docs_sync.clone(), blob_service.clone());
    let topic = TopicId::new("kukuri:topic:integrity-contract-shadowed");
    let replica = topic_replica_id(topic.as_str());
    let target = owner
        .create_post(topic.as_str(), "a post", None)
        .await
        .expect("create post");
    owner
        .toggle_reaction(topic.as_str(), target.as_str(), thumbs_up(), None)
        .await
        .expect("toggle reaction");
    let session_id = owner
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "shadowed live".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");
    let room_id = owner
        .create_game_room(
            topic.as_str(),
            CreateGameRoomInput {
                title: "shadowed game".into(),
                description: String::new(),
                participants: vec!["a".into(), "b".into()],
            },
        )
        .await
        .expect("create game room");

    let garbage = serde_json::json!({ "not": "a record" });
    let live_key = stable_key("sessions/live", &format!("{session_id}/state"));
    let game_key = stable_key("sessions/game", &format!("{room_id}/state"));
    let (reaction_envelope_key, _) = reaction_keys_for(docs_sync.as_ref(), &replica, &target).await;
    for key in [&live_key, &game_key, &reaction_envelope_key] {
        docs_sync.shadow(key.as_str(), garbage.clone()).await;
    }

    // 全件走査。
    let scanning = app_over(docs_sync.clone(), blob_service.clone());
    assert!(
        scanning
            .list_live_sessions(topic.as_str())
            .await
            .expect("live sessions")
            .iter()
            .any(|session| session.session_id == session_id),
        "an invalid record in front hid the live session"
    );
    assert!(
        scanning
            .list_game_rooms(topic.as_str())
            .await
            .expect("game rooms")
            .iter()
            .any(|room| room.room_id == room_id),
        "an invalid record in front hid the game room"
    );
    assert_eq!(reaction_count(&scanning, &topic, &target).await, 1);

    // key 指定の個別反映。
    let event_viewer = app_over(docs_sync.clone(), blob_service);
    for key in [&live_key, &game_key, &reaction_envelope_key] {
        let hydrated = hydrate_subscription_event(
            &event_viewer.services,
            topic.as_str(),
            &replica,
            key.as_str(),
        )
        .await
        .expect("hydrate");
        assert_eq!(hydrated, 1, "{key}");
    }
}

// TR-5 / AC-6: 自分の pubkey を騙る reaction の state が docs にあっても、toggle は「取り消し」にならず、自分の reaction を付ける。
#[tokio::test]
async fn toggle_reaction_ignores_a_forged_state_that_claims_the_local_author() {
    let (app, _, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-toggle-forged");
    let replica = topic_replica_id(topic.as_str());
    let target = app
        .create_post(topic.as_str(), "a post", None)
        .await
        .expect("create post");
    let me = Pubkey::from(app.current_author_pubkey().as_str());
    let reaction_id = kukuri_core::deterministic_reaction_id(
        &replica,
        &EnvelopeId::from(target.as_str()),
        &me,
        "emoji:👍",
    );
    // 攻撃者が署名した envelope と、author を自分(viewer)にした state を、viewer の reaction の key へ置く。
    let forged_envelope = kukuri_core::build_reaction_envelope(
        &generate_keys(),
        &topic,
        None,
        &EnvelopeId::from(target.as_str()),
        thumbs_up(),
        &reaction_id,
        kukuri_core::ObjectStatus::Active,
    )
    .expect("forged envelope");
    let mut forged_state = kukuri_core::parse_reaction(&forged_envelope)
        .expect("parse")
        .expect("doc");
    forged_state.author_pubkey = me;
    for (suffix, value) in [
        ("envelope", serde_json::to_value(&forged_envelope).unwrap()),
        ("state", serde_json::to_value(&forged_state).unwrap()),
    ] {
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "reactions",
                        &format!("{target}/{}/{suffix}", reaction_id.as_str()),
                    ),
                    value,
                },
            )
            .await
            .expect("write forged record");
    }

    let state = app
        .toggle_reaction(topic.as_str(), target.as_str(), thumbs_up(), None)
        .await
        .expect("toggle reaction");
    assert_eq!(
        state.my_reactions.len(),
        1,
        "the toggle treated a forged state as the viewer's own reaction and removed it"
    );
}

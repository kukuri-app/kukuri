//! reaction・live session・game room の検証(#1252)の契約。修正前の再現は `hydration_integrity_sessions.rs`。
//!
//! 正しい record がこれまでどおり反映されること(INVAR-1)と、署名が正しくても受け入れない場合(別の鍵による上書き、
//! 別の reaction の key への置き直し、古い envelope の再掲)を固定する。

use super::shadowing_docs::ShadowingDocsSync;
use super::*;
use kukuri_core::{
    GameRoomKind, GameRoomManifestBlobV1, GameRoomStateDocV1, GameRoomStatus,
    LiveSessionStateDocV1, LiveSessionStatus, ManifestBlobRef, ReactionKeyV1, SpatialContextV1,
    build_game_session_envelope, build_live_session_envelope,
};

fn thumbs_up() -> ReactionKeyV1 {
    ReactionKeyV1::Emoji {
        emoji: "👍".into()
    }
}

fn fresh_viewer(docs_sync: Arc<dyn DocsSync>, blob_service: Arc<MemoryBlobService>) -> AppService {
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
    hydrate_reaction_cache_for_target_bounded(
        app.services.docs_sync.as_ref(),
        app.services.projection_store.as_ref(),
        topic.as_str(),
        &topic_replica_id(topic.as_str()),
        &EnvelopeId::from(target),
        DocFetchPolicy::LocalOnly,
        8,
    )
    .await
    .expect("reaction");
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

// AC-2 / AC-3: fresh viewer と操作は unsigned updated_at ではなく署名済み revision で候補を選ぶ。
#[tokio::test]
async fn live_session_selection_uses_signed_revision() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let owner = fresh_viewer(docs_sync.clone(), blob_service.clone());
    let topic = TopicId::new("kukuri:topic:integrity-contract-live-revision");
    let replica = topic_replica_id(topic.as_str());
    let session_id = owner
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "revision target".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");
    let state_key = stable_key("sessions/live", &format!("{session_id}/state"));
    let mut older_state: LiveSessionStateDocV1 = serde_json::from_slice(
        &record_value(docs_sync.as_ref(), &replica, state_key.as_str()).await,
    )
    .expect("old state");
    older_state.updated_at = i64::MAX;
    owner
        .end_live_session(topic.as_str(), session_id.as_str())
        .await
        .expect("end live session");
    docs_sync
        .shadow(
            state_key.as_str(),
            serde_json::to_value(older_state).expect("old state json"),
        )
        .await;

    let viewer = fresh_viewer(docs_sync, blob_service);
    let listed = viewer
        .list_live_sessions(topic.as_str())
        .await
        .expect("list live sessions")
        .remove(0);
    assert_eq!(listed.status, LiveSessionStatus::Ended);
    let (_, _, manifest) = viewer
        .fetch_live_session_state_and_manifest(topic.as_str(), session_id.as_str())
        .await
        .expect("fetch live session")
        .expect("live session");
    assert_eq!(manifest.revision, 2);
    assert_eq!(manifest.status, LiveSessionStatus::Ended);
}

// AC-2 / AC-3: fresh viewer と操作は unsigned updated_at ではなく署名済み revision で候補を選ぶ。
#[tokio::test]
async fn score_game_selection_uses_signed_revision() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let owner = fresh_viewer(docs_sync.clone(), blob_service.clone());
    let topic = TopicId::new("kukuri:topic:integrity-contract-game-revision");
    let replica = topic_replica_id(topic.as_str());
    let room_id = owner
        .create_game_room(
            topic.as_str(),
            CreateGameRoomInput {
                title: "revision target".into(),
                description: String::new(),
                participants: vec!["a".into(), "b".into()],
            },
        )
        .await
        .expect("create game room");
    let state_key = stable_key("sessions/game", &format!("{room_id}/state"));
    let mut older_state: GameRoomStateDocV1 = serde_json::from_slice(
        &record_value(docs_sync.as_ref(), &replica, state_key.as_str()).await,
    )
    .expect("old state");
    older_state.updated_at = i64::MAX;
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
                        score: 7,
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
    docs_sync
        .shadow(
            state_key.as_str(),
            serde_json::to_value(older_state).expect("old state json"),
        )
        .await;

    let viewer = fresh_viewer(docs_sync, blob_service);
    let listed = viewer
        .list_game_rooms(topic.as_str())
        .await
        .expect("list game rooms")
        .remove(0);
    assert_eq!(listed.status, GameRoomStatus::Running);
    assert_eq!(listed.scores[0].score, 7);
    let (_, _, manifest) = viewer
        .fetch_game_room_state_and_manifest(topic.as_str(), room_id.as_str())
        .await
        .expect("fetch game room")
        .expect("game room");
    assert_eq!(manifest.score_revision, Some(2));
    assert_eq!(manifest.scores[0].score, 7);
}

// 旧形式の本番 record は無い。revision を持たない live / ScoreGame manifest は受け入れない。
#[tokio::test]
async fn session_manifests_without_revision_are_rejected() {
    let (owner, _, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-contract-required-revision");
    let replica = topic_replica_id(topic.as_str());

    let session_id = owner
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "old live format".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");
    let live_key = stable_key("sessions/live", &format!("{session_id}/state"));
    let mut live_state: LiveSessionStateDocV1 = serde_json::from_slice(
        &record_value(docs_sync.as_ref(), &replica, live_key.as_str()).await,
    )
    .expect("live state");
    let (_, _, live_manifest) = owner
        .fetch_live_session_state_and_manifest(topic.as_str(), session_id.as_str())
        .await
        .expect("fetch live")
        .expect("live");
    let mut old_live = serde_json::to_value(live_manifest).expect("live json");
    old_live
        .as_object_mut()
        .expect("live object")
        .remove("revision");
    let live_envelope = build_live_session_envelope(
        owner.services.keys.as_ref(),
        &topic,
        session_id.as_str(),
        &old_live,
    )
    .expect("sign old live format");
    let live_blob = store_manifest_blob(blob_service.as_ref(), &old_live, LIVE_MANIFEST_MIME)
        .await
        .expect("store old live format");
    persist_session_envelope(docs_sync.as_ref(), &replica, &live_envelope)
        .await
        .expect("write old live envelope");
    live_state.last_envelope_id = live_envelope.id;
    live_state.current_manifest = ManifestBlobRef {
        hash: live_blob.hash,
        mime: live_blob.mime,
        bytes: live_blob.bytes,
    };
    persist_live_session_state(docs_sync.as_ref(), &replica, &live_state)
        .await
        .expect("write old live state");

    let room_id = owner
        .create_game_room(
            topic.as_str(),
            CreateGameRoomInput {
                title: "old game format".into(),
                description: String::new(),
                participants: vec!["a".into(), "b".into()],
            },
        )
        .await
        .expect("create game room");
    let game_key = stable_key("sessions/game", &format!("{room_id}/state"));
    let mut game_state: GameRoomStateDocV1 = serde_json::from_slice(
        &record_value(docs_sync.as_ref(), &replica, game_key.as_str()).await,
    )
    .expect("game state");
    let (_, _, game_manifest) = owner
        .fetch_game_room_state_and_manifest(topic.as_str(), room_id.as_str())
        .await
        .expect("fetch game")
        .expect("game");
    let mut old_game = serde_json::to_value(game_manifest).expect("game json");
    old_game
        .as_object_mut()
        .expect("game object")
        .remove("score_revision");
    let game_envelope = build_game_session_envelope(
        owner.services.keys.as_ref(),
        &topic,
        room_id.as_str(),
        &old_game,
    )
    .expect("sign old game format");
    let game_blob = store_manifest_blob(blob_service.as_ref(), &old_game, GAME_MANIFEST_MIME)
        .await
        .expect("store old game format");
    persist_session_envelope(docs_sync.as_ref(), &replica, &game_envelope)
        .await
        .expect("write old game envelope");
    game_state.last_envelope_id = game_envelope.id;
    game_state.current_manifest = ManifestBlobRef {
        hash: game_blob.hash,
        mime: game_blob.mime,
        bytes: game_blob.bytes,
    };
    persist_game_room_state(docs_sync.as_ref(), &replica, &game_state)
        .await
        .expect("write old game state");

    let viewer = fresh_viewer(docs_sync, blob_service);
    assert!(
        viewer
            .list_live_sessions(topic.as_str())
            .await
            .expect("list live sessions")
            .is_empty()
    );
    assert!(
        viewer
            .list_game_rooms(topic.as_str())
            .await
            .expect("list game rooms")
            .is_empty()
    );
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
            score_revision: None,
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
    let rows = LiveGameProjectionStore::list_channel_game_rooms(
        store.as_ref(),
        topic.as_str(),
        "public",
        100,
    )
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

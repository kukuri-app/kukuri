//! #1221 R5-H(2026-09-27 決定): 切替後の session(live・game・Dome)の更新は、旧 replica と回転の前の epoch の bucket
//! へ書かず、作成時刻の scope bucket(private は現 epoch)へ移し、日が変わった更新は locator で読み手へ示す。

use super::*;
use kukuri_core::ChannelId;
use kukuri_docs_sync::{BucketReplica, BucketScope, DocKeyOrder, DocKeyQuery, TimeBucket};

const TOPIC: &str = "kukuri:topic:session-rehome";

async fn keys_with_prefix(docs: &MemoryDocsSync, replica: &ReplicaId, prefix: &str) -> Vec<String> {
    docs.query_replica_keys(
        replica,
        DocKeyQuery {
            prefix: prefix.to_string(),
            order: DocKeyOrder::Ascending,
            limit: 64,
        },
    )
    .await
    .map(|page| page.entries.into_iter().map(|entry| entry.key).collect())
    .unwrap_or_default()
}

/// 同じ docs を読む別の端末(手元の行は無い)。`joined` を渡すと、その channel の capability だけを持つ。
fn reader_over(
    owner: &AppService,
    joined: Option<HashMap<String, JoinedPrivateChannelState>>,
) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let mut handles = owner.services.clone();
    handles.keys = Arc::new(generate_keys());
    handles.store = store.clone();
    handles.projection_store = store.clone();
    if let Some(joined) = joined {
        handles.joined_private_channels = Arc::new(Mutex::new(joined));
    }
    (AppService::from_handles(handles), store)
}

fn game_input() -> CreateGameRoomInput {
    CreateGameRoomInput {
        title: "room".into(),
        description: String::new(),
        participants: vec!["Alice".into(), "Bob".into()],
    }
}

fn scored(score: i64) -> UpdateGameRoomInput {
    UpdateGameRoomInput {
        status: GameRoomStatus::Running,
        phase_label: None,
        scores: vec![
            GameScoreView {
                participant_id: "participant-1".into(),
                label: "Alice".into(),
                score,
            },
            GameScoreView {
                participant_id: "participant-2".into(),
                label: "Bob".into(),
                score: 0,
            },
        ],
    }
}

// 切替前に作った live と game の session を切替後に更新しても、旧 replica のキーは増えない。更新は読み手に届く。
#[tokio::test]
async fn pre_switch_sessions_move_out_of_the_legacy_replica_on_the_first_switched_update() {
    let (owner, _, docs, _) = local_app_with_memory_services();
    let live = owner
        .create_live_session(
            TOPIC,
            CreateLiveSessionInput {
                title: "live".into(),
                description: String::new(),
            },
        )
        .await
        .expect("live");
    let game = owner
        .create_game_room(TOPIC, game_input())
        .await
        .expect("game");
    let legacy = topic_replica_id(TOPIC);
    let before = keys_with_prefix(&docs, &legacy, "").await;
    assert!(
        !before.is_empty(),
        "the sessions start in the legacy replica"
    );

    owner.switch_writer(1);
    owner
        .end_live_session(TOPIC, &live)
        .await
        .expect("end live");
    owner
        .update_game_room(TOPIC, &game, scored(3))
        .await
        .expect("update game");
    assert_eq!(keys_with_prefix(&docs, &legacy, "").await, before);

    let (reader, store) = reader_over(&owner, None);
    for (id, kind) in [
        (live.as_str(), "live-session"),
        (game.as_str(), "game-session"),
    ] {
        assert!(
            reader
                .read_session(TOPIC, &TimelineScope::Public, id, kind)
                .await
                .expect("read session")
        );
    }
    assert_eq!(
        store
            .get_live_session(TOPIC, &live)
            .await
            .unwrap()
            .unwrap()
            .status,
        LiveSessionStatus::Ended
    );
    assert_eq!(
        store
            .get_game_room(TOPIC, &game)
            .await
            .unwrap()
            .unwrap()
            .scores[0]
            .score,
        3
    );
}

// 回転の後の session の更新は、回転の前の epoch の bucket へ書かない。新しい epoch の参加者は更新を読め、回転で外れた
// 参加者(旧 epoch の capability だけを持つ)は読めない。
#[tokio::test]
async fn rotated_session_updates_skip_the_old_epoch_and_reach_only_current_members() {
    let (owner, _, docs, _) = local_app_with_memory_services();
    owner.switch_writer(1);
    let channel = owner
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "room".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let game = owner
        .create_game_room_in_channel(
            TOPIC,
            ChannelRef::PrivateChannel {
                channel_id: channel_id.clone(),
            },
            game_input(),
        )
        .await
        .expect("game");
    let excluded_view = owner.joined_private_channels.lock().await.clone();
    let rotated = owner
        .rotate_private_channel(TOPIC, &channel.channel_id)
        .await
        .expect("rotate");
    owner
        .update_game_room(TOPIC, &game, scored(5))
        .await
        .expect("update after rotate");

    let today = TimeBucket::from_unix_seconds(Utc::now().timestamp()).expect("bucket");
    let bucket = |epoch_id: &str| {
        BucketReplica::new(
            BucketScope::PrivateChannel {
                channel_id: channel.channel_id.clone(),
                epoch_id: epoch_id.into(),
            },
            today,
        )
        .expect("replica")
        .replica_id()
    };
    let old_state = docs
        .query_replica_with_policy(
            &bucket(&channel.current_epoch_id),
            DocQuery::Exact(format!("sessions/game/{game}/state")),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .unwrap();
    let old_state: GameRoomStateDocV1 = serde_json::from_slice(&old_state[0].value).unwrap();
    let new_state = docs
        .query_replica_with_policy(
            &bucket(&rotated.current_epoch_id),
            DocQuery::Exact(format!("sessions/game/{game}/state")),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .unwrap();
    let new_state: GameRoomStateDocV1 = serde_json::from_slice(&new_state[0].value).unwrap();
    assert!(new_state.updated_at > old_state.updated_at);
    assert_ne!(old_state.last_envelope_id, new_state.last_envelope_id);

    let scope = TimelineScope::Channel {
        channel_id: channel_id.clone(),
    };
    let (member, member_store) = reader_over(&owner, None);
    member
        .read_session(TOPIC, &scope, &game, "game-session")
        .await
        .expect("member reads");
    assert_eq!(
        member_store
            .get_game_room(TOPIC, &game)
            .await
            .unwrap()
            .unwrap()
            .scores[0]
            .score,
        5
    );
    let (excluded, excluded_store) = reader_over(&owner, Some(excluded_view));
    excluded
        .read_session(TOPIC, &scope, &game, "game-session")
        .await
        .expect("excluded reads");
    assert!(
        excluded_store
            .get_game_room(TOPIC, &game)
            .await
            .unwrap()
            .is_none_or(|row| row.scores[0].score == 0),
        "the excluded participant does not see the update"
    );
}

// 時刻を持たない id(Dome)の session も、現在の bucket の locator が指す replica(作成時の bucket)から読む。
#[tokio::test]
async fn a_session_locator_in_the_current_bucket_leads_to_the_state_replica() {
    let (reader, _, docs, _) = local_app_with_memory_services();
    let topic_bucket = |secs: i64| {
        BucketReplica::new(
            BucketScope::Topic {
                topic_id: TOPIC.into(),
            },
            TimeBucket::from_unix_seconds(secs).expect("bucket"),
        )
        .expect("replica")
        .replica_id()
    };
    let now = Utc::now().timestamp();
    let created = topic_bucket(now - 10 * 86_400);
    docs.apply_doc_op(
        &topic_bucket(now),
        DocOp::SetJson {
            key: "sessions/game/dome-located/locator".into(),
            value: serde_json::to_value(created.as_str()).unwrap(),
        },
    )
    .await
    .expect("locator");
    let candidates = reader
        .session_target_candidates(TOPIC, PUBLIC_CHANNEL_ID, None, "dome-located", "game")
        .await
        .expect("candidates");
    assert_eq!(candidates[0].0, created);
}

//! #1221 R5-H(2026-09-27 決定): 切替後の Dome の記録は旧 topic/channel replica へ書かない。private の context の
//! Dome の instance と hosting は公開の領域(`author::` の制御領域と公開 bucket)へ置かず、channel の bucket に置く。

use super::*;
use crate::{
    AcceptDomeConnectionProposalInput, CreateDomeConnectionProposalInput, DeleteDomeInput,
    StartOwnerDomeHostingInput,
};
use kukuri_core::{ChannelId, DomeDirection, SpatialContextV1};
use kukuri_docs_sync::{BucketReplica, BucketScope, DocKeyOrder, DocKeyQuery, TimeBucket};

const TOPIC: &str = "kukuri:topic:dome-placement";

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

fn today(scope: BucketScope) -> ReplicaId {
    BucketReplica::new(
        scope,
        TimeBucket::from_unix_seconds(Utc::now().timestamp()).expect("bucket"),
    )
    .expect("replica")
    .replica_id()
}

fn peer_over(owner: &AppService, store: Arc<MemoryStore>) -> AppService {
    let mut handles = owner.services.clone();
    handles.keys = Arc::new(generate_keys());
    handles.store = store.clone();
    handles.projection_store = store;
    AppService::from_handles(handles)
}

async fn create_dome(app: &AppService, channel: ChannelRef, title: &str) -> String {
    app.create_metaverse_room_in_channel(
        TOPIC,
        channel,
        CreateMetaverseRoomInput {
            title: title.into(),
            description: String::new(),
            max_peers: Some(4),
        },
    )
    .await
    .expect("create Dome")
}

async fn start_hosting(app: &AppService, context: SpatialContextV1, instance_id: &str) {
    app.start_owner_dome_hosting(StartOwnerDomeHostingInput {
        expected_generation: None,
        spatial_context: context,
        instance_id: instance_id.into(),
        endpoint_id: "owner-endpoint".into(),
        lease_duration_millis: 60_000,
    })
    .await
    .expect("start owner hosting");
}

// 切替後の Dome の操作(作成・hosting・接続の提案と受諾・削除、private の作成と hosting)は、旧 topic/channel replica へ
// 1 件も書かない。private の Dome の instance と hosting は、owner の制御領域と公開の topic bucket に無く、channel の
// bucket にある。
#[tokio::test]
async fn switched_dome_records_skip_legacy_replicas_and_private_ones_stay_in_the_channel() {
    let (owner, _, docs, _) = local_app_with_memory_services();
    owner.switch_writer(1);
    let peer = peer_over(&owner, Arc::new(MemoryStore::default()));
    let me = owner.keys().public_key();
    let public = SpatialContextV1::Topic {
        topic_id: TopicId::new(TOPIC),
    };

    let own_dome = create_dome(&owner, ChannelRef::Public, "own").await;
    let peer_dome = create_dome(&peer, ChannelRef::Public, "peer").await;
    start_hosting(&owner, public.clone(), &own_dome).await;
    // 相手の Dome は hint の読取りで知る(session は作成時の bucket にある)。
    owner
        .read_session(TOPIC, &TimelineScope::Public, &peer_dome, "game-session")
        .await
        .expect("read the peer Dome");
    owner
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "placement".into(),
            spatial_context: public.clone(),
            proposer_instance_id: own_dome.clone(),
            receiver_instance_id: peer_dome.clone(),
            proposer_direction: DomeDirection::East,
        })
        .await
        .expect("propose");
    peer.read_session(TOPIC, &TimelineScope::Public, &own_dome, "game-session")
        .await
        .expect("read the owner Dome");
    peer.accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
        spatial_context: public.clone(),
        proposal_id: "placement".into(),
    })
    .await
    .expect("accept");
    assert_eq!(
        owner
            .list_dome_connection_topology(public.clone())
            .await
            .expect("topology")
            .connections
            .len(),
        1
    );
    owner.stop_owner_dome_hosting(&own_dome).await;
    owner
        .delete_dome(DeleteDomeInput {
            spatial_context: public,
            instance_id: own_dome,
            expected_generation: 1,
            operation_id: "placement-delete".into(),
        })
        .await
        .expect("delete Dome");

    let channel = owner
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "domes".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let private = SpatialContextV1::Channel {
        topic_id: TopicId::new(TOPIC),
        channel_id: channel_id.clone(),
    };
    let private_dome = create_dome(
        &owner,
        ChannelRef::PrivateChannel {
            channel_id: channel_id.clone(),
        },
        "private",
    )
    .await;
    start_hosting(&owner, private, &private_dome).await;
    owner.stop_owner_dome_hosting(&private_dome).await;

    let legacy_channel =
        private_channel_replica_for_epoch(&channel.channel_id, &channel.current_epoch_id);
    assert_eq!(
        keys_with_prefix(&docs, &topic_replica_id(TOPIC), "").await,
        Vec::<String>::new(),
        "nothing is written to the legacy topic replica"
    );
    for prefix in ["metaverse/", "sessions/"] {
        assert_eq!(
            keys_with_prefix(&docs, &legacy_channel, prefix).await,
            Vec::<String>::new(),
            "no Dome record in the legacy channel replica"
        );
    }
    let author = author_replica_id(me.as_str());
    for prefix in ["metaverse/dome-instances/", "metaverse/dome-hosting/"] {
        assert!(
            keys_with_prefix(&docs, &author, prefix)
                .await
                .iter()
                .all(|key| !key.contains(&private_dome)),
            "the private Dome is not in the public control area"
        );
    }
    let public_bucket = today(BucketScope::Topic {
        topic_id: TOPIC.into(),
    });
    assert!(
        keys_with_prefix(&docs, &public_bucket, "")
            .await
            .iter()
            .all(|key| !key.contains(&private_dome)),
        "the private Dome is not in the public topic bucket"
    );
    let private_bucket = today(BucketScope::PrivateChannel {
        channel_id: channel.channel_id.clone(),
        epoch_id: channel.current_epoch_id.clone(),
    });
    let private_keys = keys_with_prefix(&docs, &private_bucket, "metaverse/").await;
    for key in [
        format!("metaverse/dome-instances/{private_dome}/state"),
        format!("metaverse/dome-hosting/{private_dome}/state"),
    ] {
        assert!(private_keys.contains(&key), "{key} in {private_keys:?}");
    }
}

// 行の無い channel の参加者は、owner の端末の hosting の heartbeat と、channel の bucket の locator から private の Dome を
// 一覧に出す。capability の無い端末(channel に参加していない)は、その Dome を読めない。
#[tokio::test]
async fn a_participant_lists_a_hosted_private_dome_and_an_outsider_cannot() {
    let (owner, _, _, _) = local_app_with_memory_services();
    owner.switch_writer(1);
    let channel = owner
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "domes".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let context = SpatialContextV1::Channel {
        topic_id: TopicId::new(TOPIC),
        channel_id: channel_id.clone(),
    };
    let dome = create_dome(
        &owner,
        ChannelRef::PrivateChannel {
            channel_id: channel_id.clone(),
        },
        "private",
    )
    .await;
    start_hosting(&owner, context, &dome).await;
    let heartbeat = owner
        .dome_host_sessions
        .lock()
        .await
        .get(&dome)
        .expect("owner session")
        .signed_heartbeat(Utc::now().timestamp_millis())
        .expect("signed heartbeat");

    // 参加者: 同じ docs と channel の capability を持ち、手元に Dome の行が無い。
    let participant = peer_over(&owner, Arc::new(MemoryStore::default()));
    participant
        .dome_host_heartbeats
        .lock()
        .await
        .insert(dome.clone(), heartbeat.clone());
    let scope = TimelineScope::Channel {
        channel_id: channel_id.clone(),
    };
    let listed = timeout(Duration::from_secs(10), async {
        loop {
            let rooms = participant
                .list_game_rooms_scoped(TOPIC, scope.clone())
                .await
                .expect("participant rooms");
            if rooms.iter().any(|room| room.room_id == dome) {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        listed.is_ok(),
        "the participant lists the hosted private Dome"
    );

    // 参加していない端末: channel を読めず、公開の一覧にも出ない。
    let (outsider, _, _, _) = local_app_with_memory_services();
    outsider.switch_writer(1);
    outsider
        .dome_host_heartbeats
        .lock()
        .await
        .insert(dome.clone(), heartbeat);
    assert!(outsider.list_game_rooms_scoped(TOPIC, scope).await.is_err());
    assert!(
        outsider
            .list_game_rooms(TOPIC)
            .await
            .expect("public rooms")
            .iter()
            .all(|room| room.room_id != dome)
    );
    owner.shutdown().await;
}

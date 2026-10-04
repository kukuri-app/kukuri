//! 受信した Dome host の heartbeat の台帳。最後の heartbeat から 30 秒で捨て、context ごとに一覧の窓
//! (`LIVE_GAME_LIST_LIMIT`)の件数まで置く。

use super::sync::CountingDocsSync;
use super::*;
use kukuri_core::{
    DomeHostHeartbeatV1, SignedDomeHostHeartbeatV1, SpatialContextV1, sign_envelope_json,
};

const TOPIC: &str = "kukuri:topic:dome-heartbeats";

fn topic_context() -> SpatialContextV1 {
    SpatialContextV1::Topic {
        topic_id: TopicId::new(TOPIC),
    }
}

/// owner の端末の host が context の Dome について送る heartbeat(受信の経路は署名を確かめない)。
fn heartbeat(context: &SpatialContextV1, sent_at: i64) -> (String, SignedDomeHostHeartbeatV1) {
    let keys = generate_keys();
    let instance_id = dome_instance_id(context, &keys.public_key());
    let heartbeat = DomeHostHeartbeatV1 {
        instance_id: instance_id.clone(),
        instance_generation: 1,
        lease_epoch: 1,
        session_id: "session".into(),
        host_pubkey: keys.public_key(),
        participants: 0,
        sleeping: true,
        sequence: 1,
        sent_at,
    };
    let envelope =
        sign_envelope_json(&keys, "dome-host-heartbeat", vec![], &heartbeat).expect("sign");
    (
        instance_id,
        SignedDomeHostHeartbeatV1 {
            heartbeat,
            envelope,
        },
    )
}

/// scope の task が hint で受け取ったときと同じく台帳へ置く。
async fn receive(
    app: &AppService,
    context: &SpatialContextV1,
    (instance_id, signed): (String, SignedDomeHostHeartbeatV1),
) {
    app.dome_host_heartbeats.lock().await.record(
        context,
        &instance_id,
        signed,
        Utc::now().timestamp_millis(),
    );
}

// 署名時刻から 30 秒を過ぎた heartbeat は台帳に置かない。topic の hint で受け取る経路で確かめる。
#[tokio::test]
async fn a_heartbeat_past_the_retention_does_not_list_its_host() {
    let hints = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let store = Arc::new(MemoryStore::default());
    let visitor = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    display_topic(&visitor, TOPIC)
        .await
        .expect("display the topic");
    let context = topic_context();
    let now = Utc::now().timestamp_millis();
    let stale = heartbeat(&context, now - 30_001);
    let fresh = heartbeat(&context, now);
    let fresh_host = fresh.1.heartbeat.host_pubkey.clone();
    for (instance_id, signed) in [stale, fresh] {
        hints
            .publish_hint(
                &TopicId::new(TOPIC),
                GossipHint::DomeHostHeartbeat {
                    topic_id: TopicId::new(TOPIC),
                    instance_id,
                    heartbeat: Box::new(signed),
                },
            )
            .await
            .expect("publish the heartbeat");
    }
    // task は届いた順に処理する。新しい方が入っていれば、古い方の処理も終わっている。
    let owners = timeout(Duration::from_secs(10), async {
        loop {
            let owners = visitor.heartbeat_dome_owners(&context).await;
            if owners.contains(&fresh_host) {
                return owners;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the fresh heartbeat is received");
    assert_eq!(owners, vec![fresh_host]);
    visitor.shutdown().await;
}

// private channel の hint で受け取った heartbeat は channel の context に置く。channel の context で instance id を導けない
// heartbeat(公開の context の Dome のもの)は置かない。
#[tokio::test]
async fn a_private_channel_heartbeat_is_kept_under_the_channel_context() {
    let hints = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let store = Arc::new(MemoryStore::default());
    let member = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let channel = member
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "domes".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = kukuri_core::ChannelId::new(channel.channel_id.clone());
    member
        .set_scope_display(crate::ScopeDisplayRequest {
            observer: "test-channel".into(),
            target: crate::ScopeDisplayTarget::Timeline {
                topic: TOPIC.into(),
                scope: TimelineScope::Channel {
                    channel_id: channel_id.clone(),
                },
            },
            visible: true,
        })
        .await
        .expect("display the channel");
    let channel_context = SpatialContextV1::Channel {
        topic_id: TopicId::new(TOPIC),
        channel_id,
    };
    let now = Utc::now().timestamp_millis();
    let public = heartbeat(&topic_context(), now);
    let private = heartbeat(&channel_context, now);
    let private_host = private.1.heartbeat.host_pubkey.clone();
    for (instance_id, signed) in [public, private] {
        hints
            .publish_hint(
                &private_channel_hint_topic(&channel.channel_id),
                GossipHint::DomeHostHeartbeat {
                    topic_id: TopicId::new(TOPIC),
                    instance_id,
                    heartbeat: Box::new(signed),
                },
            )
            .await
            .expect("publish the heartbeat");
    }
    let owners = timeout(Duration::from_secs(10), async {
        loop {
            let owners = member.heartbeat_dome_owners(&channel_context).await;
            if owners.contains(&private_host) {
                return owners;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the channel heartbeat is received");
    assert_eq!(owners, vec![private_host]);
    assert!(
        member
            .heartbeat_dome_owners(&topic_context())
            .await
            .is_empty()
    );
    member.shutdown().await;
}

// 1 つの context で一覧の窓を超える host の heartbeat を受け取っても、一覧の組み立てが読む owner は窓の件数で止まる。
#[tokio::test]
async fn listing_reads_stop_at_the_list_window_of_hosts() {
    let docs = Arc::new(CountingDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let visitor = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let context = topic_context();
    let mut listing_queries = Vec::new();
    for _ in 0..2 {
        for _ in 0..LIVE_GAME_LIST_LIMIT + 50 {
            receive(
                &visitor,
                &context,
                heartbeat(&context, Utc::now().timestamp_millis()),
            )
            .await;
        }
        docs.clear_queries().await;
        visitor.list_game_rooms(TOPIC).await.expect("list rooms");
        listing_queries.push(docs.queries().await.len());
    }
    assert_eq!(
        listing_queries[0], listing_queries[1],
        "{listing_queries:?}"
    );
    assert_eq!(
        visitor.heartbeat_dome_owners(&context).await.len(),
        LIVE_GAME_LIST_LIMIT
    );
}

// heartbeat でしか知らない Dome も、最後の heartbeat から 30 秒までは Instance を引き当てられる。hosting の状態は 5 秒までは
// 所有者の端末で稼働中、15 秒までは猶予期間、それ以降は heartbeat_timeout の停止(入室中の画面は退避する。ADR 0045)。
#[tokio::test]
async fn a_dome_known_from_its_heartbeat_reads_grace_then_closed_and_drops_a_stale_session() {
    let owner_docs = Arc::new(CountingDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let app = |docs: Arc<CountingDocsSync>| {
        let store = Arc::new(MemoryStore::default());
        app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            Arc::new(NoopHintTransport),
            docs,
            blobs.clone(),
            generate_keys(),
        )
    };
    let owner = app(owner_docs.clone());
    let visitor = app(Arc::new(CountingDocsSync::reading_from(owner_docs)));
    let context = topic_context();
    let dome = owner
        .create_metaverse_room(
            TOPIC,
            CreateMetaverseRoomInput {
                title: "hosted Dome".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create Dome");
    owner
        .start_owner_dome_hosting(crate::StartOwnerDomeHostingInput {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: dome.clone(),
            endpoint_id: "owner-endpoint".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .expect("start owner hosting");
    let sent_at = Utc::now().timestamp_millis();
    let heartbeat = owner_heartbeat(&owner, &dome, sent_at).await;
    receive(&visitor, &context, (dome.clone(), heartbeat)).await;
    let instance = visitor
        .hosting_instance(&context, &dome)
        .await
        .expect("look up the Instance")
        .expect("the Instance of the heartbeat host");
    let records = visitor
        .list_dome_hosting_records(&instance)
        .await
        .expect("hosting records");
    use kukuri_core::DomeHostingStateKindV1::{Closed, GracePeriod, OwnerHosted};
    for (elapsed, kind, reason) in [
        (1_000, OwnerHosted, None),
        (10_000, GracePeriod, Some("heartbeat_missing")),
        (20_000, Closed, Some("heartbeat_timeout")),
    ] {
        let state = visitor
            .hosting_authority_view(&instance, &records, sent_at + elapsed)
            .await
            .expect("hosting state")
            .state;
        assert_eq!(
            (state.kind, state.reason.as_deref(), state.last_heartbeat_at),
            (kind, reason, Some(sent_at)),
            "{elapsed} ms after the heartbeat"
        );
    }

    // lease と session に合わない heartbeat(host が session をやり直す前のものなど)は、hosting の状態の組み立てで捨てる。
    // sequence が小さい、次の session の heartbeat を置けるようにする。
    let now = Utc::now().timestamp_millis();
    let mut stale = owner_heartbeat(&owner, &dome, now).await;
    stale.heartbeat.session_id = "previous-session".into();
    stale.heartbeat.sequence += 100;
    receive(&visitor, &context, (dome.clone(), stale)).await;
    visitor
        .hosting_authority_view(&instance, &records, now)
        .await
        .expect("hosting state");
    let next = owner_heartbeat(&owner, &dome, now).await;
    let mut ledger = visitor.dome_host_heartbeats.lock().await;
    assert!(ledger.latest(&context, &dome, now).is_none());
    assert!(ledger.record(&context, &dome, next, now));
    drop(ledger);
    owner.shutdown().await;
}

async fn owner_heartbeat(
    owner: &AppService,
    dome: &str,
    sent_at: i64,
) -> SignedDomeHostHeartbeatV1 {
    owner
        .dome_host_sessions
        .lock()
        .await
        .get(dome)
        .expect("owner session")
        .signed_heartbeat(sent_at)
        .expect("signed heartbeat")
}

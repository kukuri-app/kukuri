use super::*;
use crate::PushLiveOverlayRequest;

const TOPIC: &str = "kukuri:topic:overlay";

#[tokio::test]
async fn three_devices_receive_independent_presses_and_resolve_the_original_attachment_without_mutating_posts()
 {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("02".repeat(32));
    let c_docs = MemoryDocsSync::with_docs_author("03".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let devices = [
        public_channels::device_with_store(
            "a",
            &network,
            a_docs.clone(),
            b_docs.clone(),
            blobs.clone(),
        ),
        public_channels::device_with_store("b", &network, b_docs, a_docs.clone(), blobs.clone()),
        public_channels::device_with_store("c", &network, c_docs, a_docs, blobs),
    ];
    let [(a, a_store), (b, _), (c, _)] = devices.as_slice() else {
        unreachable!()
    };
    let channel = a
        .create_public_channel(TOPIC, "Overlay", "", Vec::new(), Vec::new())
        .await
        .unwrap();
    let scope = ChannelRef::PublicChannel {
        channel_id: ChannelId::new(&channel.channel_id),
    };
    for (device, _) in &devices {
        assert!(
            device
                .get_public_channel(TOPIC, &channel.channel_id)
                .await
                .unwrap()
                .is_some()
        );
        device
            .set_scope_display(crate::ScopeDisplayRequest {
                observer: "overlay-test".into(),
                target: crate::ScopeDisplayTarget::Timeline {
                    topic: TOPIC.into(),
                    scope: scope.timeline_scope(),
                },
                visible: true,
            })
            .await
            .unwrap();
    }
    let mut live_ids = Vec::new();
    for device in [a, b] {
        live_ids.push(
            device
                .create_live_session_in_channel(
                    TOPIC,
                    scope.clone(),
                    CreateLiveSessionInput {
                        title: "Live".into(),
                        description: "".into(),
                        start_post: "".into(),
                    },
                    None,
                )
                .await
                .unwrap(),
        );
    }
    let post = a
        .create_post_with_attachments_in_channel(
            TOPIC,
            scope.clone(),
            "",
            None,
            vec![
                pending_image_attachment("image/png", &tiny_png_bytes()),
                pending_image_attachment(
                    "image/gif",
                    include_bytes!("../../../../apps/desktop/tests/fixtures/overlay-animated.gif"),
                ),
            ],
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    let before = a_store
        .get_object_projection(&EnvelopeId::from(post.as_str()))
        .await
        .unwrap()
        .unwrap();
    let target = PushLiveOverlayRequest {
        topic: TOPIC.into(),
        channel_id: channel.channel_id.clone(),
        object_id: post.clone(),
        attachment_index: 1,
        attachment_hash: before.attachments[1].hash.as_str().into(),
    };
    let mut received = c.subscribe_live_overlay_pushes();
    let visible = b
        .list_timeline_scoped(TOPIC, scope.timeline_scope(), None, 50)
        .await
        .unwrap();
    assert!(visible.items.iter().any(|item| item.object_id == post));
    for _ in 0..10 {
        a.push_live_overlay(target.clone()).await.unwrap();
        b.push_live_overlay(target.clone()).await.unwrap();
    }
    let mut seen = BTreeSet::new();
    let mut authors = BTreeSet::new();
    for _ in 0..20 {
        let event = timeout(Duration::from_secs(5), received.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(seen.insert(event.event_id.clone()));
        authors.insert(event.sender_pubkey.clone());
        let post = c
            .resolve_live_overlay_push(&event.event_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(post.object_id, target.object_id);
        assert_eq!(post.attachments[1].hash, target.attachment_hash);
        assert_eq!(
            post.channel_id.as_deref(),
            Some(channel.channel_id.as_str())
        );
    }
    assert_eq!(authors.len(), 2);
    assert!(
        received.try_recv().is_err(),
        "local/network echoes do not add requests"
    );
    let after = a_store
        .get_object_projection(&EnvelopeId::from(post.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before, after, "Push leaves the original post untouched");
    let mut unrelated = target.clone();
    unrelated.attachment_hash = "not-an-attachment".into();
    assert!(a.push_live_overlay(unrelated).await.is_err());
    let mut other_scope = target.clone();
    other_scope.channel_id = "pc-1700000000000-bbbbbbbbbbbbbbbb".into();
    assert!(a.push_live_overlay(other_scope).await.is_err());
    c.mute_author(a.current_author_pubkey().as_str())
        .await
        .unwrap();
    a.push_live_overlay(target.clone()).await.unwrap();
    assert!(
        timeout(Duration::from_millis(100), received.recv())
            .await
            .is_err()
    );
    a.end_live_session(TOPIC, &live_ids[0]).await.unwrap();
    b.end_live_session(TOPIC, &live_ids[1]).await.unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            if a.get_live_session(TOPIC, &live_ids[1])
                .await
                .unwrap()
                .is_some_and(|live| live.status == LiveSessionStatus::Ended)
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(a.push_live_overlay(target).await.is_err());
}

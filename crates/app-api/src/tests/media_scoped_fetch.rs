use super::*;

#[tokio::test]
async fn create_post_with_image_attachment_surfaces_attachment_metadata() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let object_id = app
        .create_post_with_attachments(
            "kukuri:topic:image-write",
            "caption",
            None,
            vec![PendingAttachment {
                bookmarked_media_hash: None,
                mime: "image/png".into(),
                bytes: b"fake-image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
        )
        .await
        .expect("create image post");
    let timeline = app
        .list_timeline("kukuri:topic:image-write", None, 10)
        .await
        .expect("timeline");

    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("image post");
    assert_eq!(post.content, "caption");
    assert_eq!(post.attachments.len(), 1);
    assert_eq!(post.attachments[0].mime, "image/png");
    assert_eq!(post.attachments[0].role, "image_original");
    assert_eq!(post.attachments[0].status, BlobViewStatus::Available);
    let hash = post.attachments[0].hash.as_str();
    assert_eq!(
        app.blob_media_payload_for_post(hash, "image/png", Some(&object_id))
            .await
            .unwrap()
            .unwrap()
            .bytes_base64,
        "ZmFrZS1pbWFnZQ=="
    );
    assert!(
        app.blob_media_payload_for_post(hash, "image/png", Some("missing-post"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn expired_projection_blocks_media_bytes_until_source_is_revalidated() {
    let store = Arc::new(SqliteStore::connect_memory().await.unwrap());
    let app = AppService::new(
        store.clone(),
        Arc::new(FakeTransport::new("expired-media", FakeNetwork::default())),
    );
    let object_id = app
        .create_post_with_attachments(
            "kukuri:topic:expired-media",
            "caption",
            None,
            vec![PendingAttachment {
                bookmarked_media_hash: None,
                mime: "image/png".into(),
                bytes: b"cached-image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
        )
        .await
        .unwrap();
    let row = store
        .get_object_projection(&EnvelopeId::from(object_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    let hash = row.attachments[0].hash.as_str().to_string();
    store.put_remote_object_projection(row).await.unwrap();
    assert!(
        app.blob_media_payload_for_post(&hash, "image/png", Some(&object_id))
            .await
            .unwrap()
            .is_some()
    );
    sqlx::query(
        "UPDATE remote_content_cache SET last_used_at = 0 \
         WHERE kind = 'projection' AND cache_key = ?1",
    )
    .bind(&object_id)
    .execute(store.pool())
    .await
    .unwrap();
    assert!(
        app.blob_media_payload_for_post(&hash, "image/png", Some(&object_id))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn private_post_media_requires_current_channel_membership() {
    let app = AppService::new(
        Arc::new(MemoryStore::default()),
        Arc::new(FakeTransport::new("private-media", FakeNetwork::default())),
    );
    let topic = "kukuri:topic:private-media-fetch";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "media".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .unwrap();
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let object_id = app
        .create_post_with_attachments_in_channel(
            topic,
            ChannelRef::PrivateChannel {
                channel_id: channel_id.clone(),
            },
            "private image",
            None,
            vec![PendingAttachment {
                bookmarked_media_hash: None,
                mime: "image/png".into(),
                bytes: b"private image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    let page = app
        .list_timeline_scoped(
            topic,
            TimelineScope::Channel {
                channel_id: channel_id.clone(),
            },
            None,
            20,
        )
        .await
        .unwrap();
    let hash = &page.items[0].attachments[0].hash;
    assert!(
        app.blob_media_payload_for_post(hash, "image/png", Some(&object_id))
            .await
            .unwrap()
            .is_some()
    );
    app.leave_private_channel(topic, channel_id.as_str())
        .await
        .unwrap();
    assert!(
        app.blob_media_payload_for_post(hash, "image/png", Some(&object_id))
            .await
            .unwrap()
            .is_none()
    );
}

use super::super::*;

#[tokio::test]
async fn create_public_post_persists_profile_post_doc_and_lists_profile_timeline() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service,
        keys,
    );
    let topic = "kukuri:topic:profile-doc";

    let object_id = app
        .create_post(topic, "hello profile", None)
        .await
        .expect("create post");
    let profile_docs = author_profile_post_docs(docs_sync.as_ref(), author_pubkey.as_str()).await;

    assert_eq!(profile_docs.len(), 1);
    assert_eq!(profile_docs[0].author_pubkey.as_str(), author_pubkey);
    assert_eq!(
        profile_docs[0].profile_topic_id,
        author_profile_topic_id(author_pubkey.as_str())
    );
    assert_eq!(profile_docs[0].published_topic_id.as_str(), topic);
    assert_eq!(profile_docs[0].object_id.as_str(), object_id);
    assert_eq!(profile_docs[0].object_kind, "post");

    let timeline = app
        .list_profile_timeline(author_pubkey.as_str(), None, 20)
        .await
        .expect("profile timeline");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("profile post");

    assert_eq!(post.content, "hello profile");
    assert_eq!(post.published_topic_id.as_deref(), Some(topic));
    assert_eq!(post.channel_id, None);
    assert_eq!(post.audience_label, "Public");
    assert!(post.reaction_summary.is_empty());
    assert!(post.my_reactions.is_empty());
}

#[tokio::test]
async fn expired_content_observations_do_not_reach_post_profile_or_attachment_views() {
    let (app, store, _docs_sync, _blob_service) = local_app_with_memory_services();
    let profile = app
        .set_my_profile(ProfileInput {
            name: Some("retention-owner".into()),
            display_name: Some("保持期限の確認用".into()),
            about: None,
            picture_upload: None,
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect("set profile");
    let topic = "kukuri:topic:observation-retention";
    let object_id = app
        .create_post_with_attachments(
            topic,
            "retention body",
            None,
            vec![pending_image_attachment(
                "image/png",
                tiny_png_bytes().as_slice(),
            )],
        )
        .await
        .expect("create observed post");

    for (subject_kind, subject_id) in [
        ("post", object_id.as_str()),
        ("profile", profile.pubkey.as_str()),
    ] {
        assert!(
            store
                .put_content_observation(ContentObservationRow {
                    subject_kind: subject_kind.to_string(),
                    subject_id: subject_id.to_string(),
                    node_base_url: "https://expired-observer.example".to_string(),
                    capability: "community_index".to_string(),
                    observed_at: 1,
                })
                .await
                .expect("store expired observation")
        );
    }

    let timeline = app
        .list_timeline(topic, None, 20)
        .await
        .expect("list timeline");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("observed post");
    assert_eq!(post.provenance, None);
    assert_eq!(post.attachments.len(), 1);
    assert_eq!(post.attachments[0].provenance, None);

    let author = app
        .build_author_social_view(profile.pubkey.as_str())
        .await
        .expect("build author view");
    assert_eq!(author.provenance, None);
    assert!(
        store
            .list_content_observations("post", object_id.as_str())
            .await
            .expect("list post observations")
            .is_empty()
    );
    assert!(
        store
            .list_content_observations("profile", profile.pubkey.as_str())
            .await
            .expect("list profile observations")
            .is_empty()
    );
}

#[tokio::test]
async fn public_reply_is_indexed_in_profile_timeline() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service,
        keys,
    );
    let topic = "kukuri:topic:profile-replies";

    let root_id = app
        .create_post(topic, "root", None)
        .await
        .expect("root post");
    let reply_id = app
        .create_post(topic, "reply", Some(root_id.as_str()))
        .await
        .expect("reply post");
    let profile_docs = author_profile_post_docs(docs_sync.as_ref(), author_pubkey.as_str()).await;
    let reply_doc = profile_docs
        .iter()
        .find(|doc| doc.object_id.as_str() == reply_id)
        .expect("reply profile doc");

    assert_eq!(reply_doc.object_kind, "comment");
    assert_eq!(
        reply_doc.reply_to_object_id.as_ref().map(|id| id.as_str()),
        Some(root_id.as_str())
    );
    assert_eq!(
        reply_doc.root_id.as_ref().map(|id| id.as_str()),
        Some(root_id.as_str())
    );

    let timeline = app
        .list_profile_timeline(author_pubkey.as_str(), None, 20)
        .await
        .expect("profile timeline");
    let reply = timeline
        .items
        .iter()
        .find(|post| post.object_id == reply_id)
        .expect("profile reply");

    assert_eq!(reply.object_kind, "comment");
    assert_eq!(reply.reply_to.as_deref(), Some(root_id.as_str()));
    assert_eq!(reply.root_id.as_deref(), Some(root_id.as_str()));
    assert_eq!(reply.published_topic_id.as_deref(), Some(topic));
}

#[tokio::test]
async fn private_channel_post_is_not_indexed_in_profile_timeline() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service,
        keys,
    );
    let topic = "kukuri:topic:profile-private";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "core".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");

    let private_object_id = app
        .create_post_in_channel(
            topic,
            ChannelRef::PrivateChannel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
            "private hello",
            None,
        )
        .await
        .expect("create private post");

    let profile_docs = author_profile_post_docs(docs_sync.as_ref(), author_pubkey.as_str()).await;
    assert!(profile_docs.is_empty());

    let timeline = app
        .list_profile_timeline(author_pubkey.as_str(), None, 20)
        .await
        .expect("profile timeline");
    assert!(
        timeline
            .items
            .iter()
            .all(|post| post.object_id != private_object_id)
    );
}

#[tokio::test]
async fn set_my_profile_with_avatar_upload_persists_blob_backed_profile_and_author_view() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let avatar_bytes = tiny_png_bytes();

    let updated = app
        .set_my_profile(ProfileInput {
            name: Some("avatar-owner".into()),
            display_name: Some("Avatar Owner".into()),
            about: Some("blob avatar".into()),
            picture_upload: Some(PendingAttachment {
                mime: "image/png".into(),
                bytes: avatar_bytes.clone(),
                role: AssetRole::ProfileAvatar,
            }),
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect("set profile");

    let asset = updated.picture_asset.clone().expect("profile avatar asset");
    let stored_profile = store
        .get_profile(updated.pubkey.as_str())
        .await
        .expect("stored profile")
        .expect("stored profile value");
    let profile_doc = author_profile_doc(docs_sync.as_ref(), updated.pubkey.as_str())
        .await
        .expect("profile doc");
    let stored_blob = blob_service
        .fetch_blob(&asset.hash)
        .await
        .expect("fetch avatar blob")
        .expect("avatar blob");
    let local_profile = app.get_my_profile().await.expect("get my profile");
    let author_social = app
        .get_author_social_view(updated.pubkey.as_str())
        .await
        .expect("author social view");

    assert_eq!(asset.mime, "image/png");
    assert_eq!(asset.role, AssetRole::ProfileAvatar);
    assert_eq!(stored_blob, avatar_bytes);
    assert_eq!(stored_profile.picture_asset, updated.picture_asset);
    assert_eq!(profile_doc.picture_asset, updated.picture_asset);
    assert_eq!(local_profile.picture_asset, updated.picture_asset);
    assert_eq!(
        author_social
            .picture_asset
            .as_ref()
            .map(|value| value.hash.as_str()),
        Some(asset.hash.as_str())
    );
    assert_eq!(
        author_social
            .picture_asset
            .as_ref()
            .map(|value| value.mime.as_str()),
        Some("image/png")
    );
    assert_eq!(
        author_social
            .picture_asset
            .as_ref()
            .map(|value| value.role.as_str()),
        Some("profile_avatar")
    );
}

#[tokio::test]
async fn set_my_profile_preserves_blob_avatar_when_only_text_changes() {
    let (app, store, docs_sync, _) = local_app_with_memory_services();
    let initial = app
        .set_my_profile(ProfileInput {
            name: Some("asset-owner".into()),
            display_name: Some("Asset Owner".into()),
            about: Some("blob avatar".into()),
            picture_upload: Some(PendingAttachment {
                mime: "image/png".into(),
                bytes: tiny_png_bytes(),
                role: AssetRole::ProfileAvatar,
            }),
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect("set initial profile");
    let initial_asset = initial.picture_asset.expect("initial avatar asset");

    let updated = app
        .set_my_profile(ProfileInput {
            name: Some("asset-owner".into()),
            display_name: Some("Updated Asset Owner".into()),
            about: Some("updated text".into()),
            picture_upload: None,
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect("set profile");

    let stored_profile = store
        .get_profile(updated.pubkey.as_str())
        .await
        .expect("stored profile")
        .expect("stored profile value");
    let profile_doc = author_profile_doc(docs_sync.as_ref(), updated.pubkey.as_str())
        .await
        .expect("profile doc");
    let local_profile = app.get_my_profile().await.expect("get my profile");
    let author_social = app
        .get_author_social_view(updated.pubkey.as_str())
        .await
        .expect("author social view");

    assert_eq!(updated.display_name.as_deref(), Some("Updated Asset Owner"));
    assert_eq!(updated.picture_asset.as_ref(), Some(&initial_asset));
    assert_eq!(stored_profile.picture_asset.as_ref(), Some(&initial_asset));
    assert_eq!(profile_doc.picture_asset.as_ref(), Some(&initial_asset));
    assert_eq!(local_profile.picture_asset.as_ref(), Some(&initial_asset));
    assert_eq!(
        author_social
            .picture_asset
            .as_ref()
            .map(|asset| asset.hash.as_str()),
        Some(initial_asset.hash.as_str())
    );
}

#[tokio::test]
async fn create_same_topic_repost_persists_repost_object_and_profile_repost_doc() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service,
        keys,
    );
    let topic = "kukuri:topic:repost-same";

    let source_object_id = app
        .create_post(topic, "hello repost", None)
        .await
        .expect("create source post");
    let repost_object_id = app
        .create_repost(topic, topic, source_object_id.as_str(), None)
        .await
        .expect("create repost");

    let timeline = app.list_timeline(topic, None, 20).await.expect("timeline");
    let repost = timeline
        .items
        .iter()
        .find(|post| post.object_id == repost_object_id)
        .expect("repost item");

    assert_eq!(repost.object_kind, "repost");
    assert_eq!(repost.published_topic_id.as_deref(), Some(topic));
    assert!(repost.repost_of.is_some());
    assert_eq!(repost.repost_commentary, None);
    assert!(!repost.is_threadable);

    let profile_docs = author_profile_repost_docs(docs_sync.as_ref(), author_pubkey.as_str()).await;
    assert_eq!(profile_docs.len(), 1);
    assert_eq!(profile_docs[0].object_id.as_str(), repost_object_id);
    assert_eq!(profile_docs[0].published_topic_id.as_str(), topic);
    assert_eq!(profile_docs[0].repost_of.source_topic_id.as_str(), topic);
}

#[tokio::test]
async fn create_post_rejects_content_over_limit() {
    let (app, _store, _docs_sync, _blob_service) = local_app_with_memory_services();
    let topic = "kukuri:topic:post-length";

    let oversized = "a".repeat(crate::service::MAX_POST_CONTENT_CHARS + 1);
    let error = app
        .create_post(topic, oversized.as_str(), None)
        .await
        .expect_err("post content over the limit should be rejected");
    assert!(
        error.to_string().contains("post content"),
        "unexpected error: {error}"
    );

    let at_limit = "a".repeat(crate::service::MAX_POST_CONTENT_CHARS);
    app.create_post(topic, at_limit.as_str(), None)
        .await
        .expect("post content at the limit should be accepted");
}

#[tokio::test]
async fn create_repost_rejects_commentary_over_limit() {
    let (app, _store, _docs_sync, _blob_service) = local_app_with_memory_services();
    let topic = "kukuri:topic:repost-length";
    let source_object_id = app
        .create_post(topic, "source", None)
        .await
        .expect("source post");

    let oversized = "a".repeat(crate::service::MAX_REPOST_COMMENTARY_CHARS + 1);
    let error = app
        .create_repost(
            topic,
            topic,
            source_object_id.as_str(),
            Some(oversized.as_str()),
        )
        .await
        .expect_err("repost commentary over the limit should be rejected");
    assert!(
        error.to_string().contains("repost commentary"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn set_my_profile_rejects_text_fields_over_limit() {
    let (app, _store, _docs_sync, _blob_service) = local_app_with_memory_services();

    let error = app
        .set_my_profile(ProfileInput {
            name: Some("a".repeat(crate::service::MAX_PROFILE_NAME_CHARS + 1)),
            display_name: None,
            about: None,
            picture_upload: None,
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect_err("profile name over the limit should be rejected");
    assert!(
        error.to_string().contains("profile name"),
        "unexpected error: {error}"
    );

    let error = app
        .set_my_profile(ProfileInput {
            name: None,
            display_name: None,
            about: Some("a".repeat(crate::service::MAX_PROFILE_ABOUT_CHARS + 1)),
            picture_upload: None,
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect_err("profile about over the limit should be rejected");
    assert!(
        error.to_string().contains("profile about"),
        "unexpected error: {error}"
    );
}

// #1670 AC-1.1: NIP-05 の識別子は前後の空白を除いて小文字にして署名し、自分の行と、プロフィールを読んだ
// 他の端末の行に同じ値が入る。空にすると欄の無い profile になる。
#[tokio::test]
async fn set_my_profile_signs_the_normalized_nip05_for_readers() {
    let (app, _store, docs_sync, _blob_service) = local_app_with_memory_services();
    let author = app.current_author_pubkey();
    let profile = app
        .set_my_profile(ProfileInput {
            name: Some("alice".into()),
            nip05: Some(" Alice@Example.COM ".into()),
            ..Default::default()
        })
        .await
        .expect("set profile");
    assert_eq!(profile.nip05.as_deref(), Some("alice@example.com"));
    assert_eq!(
        app.get_my_profile().await.expect("my profile").nip05,
        profile.nip05
    );

    let reader_store = Arc::new(MemoryStore::default());
    let reader = app_service_from_dependencies(
        reader_store.clone(),
        reader_store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    crate::service::author_state_support::hydrate_author_state(
        &reader.services,
        reader.current_author_pubkey().as_str(),
        author.as_str(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate author");
    assert_eq!(
        reader_store
            .get_profile(author.as_str())
            .await
            .expect("reader profile")
            .and_then(|profile| profile.nip05)
            .as_deref(),
        Some("alice@example.com")
    );

    let cleared = app
        .set_my_profile(ProfileInput {
            name: Some("alice".into()),
            nip05: Some("  ".into()),
            ..Default::default()
        })
        .await
        .expect("clear nip05");
    assert_eq!(cleared.nip05, None);
}

// #1670 AC-1.2: 形に合わない識別子は保存せず、署名・store・docs への書込みを行わない。
#[tokio::test]
async fn set_my_profile_rejects_an_invalid_nip05_without_writing() {
    let (app, store, docs_sync, _blob_service) = local_app_with_memory_services();
    let author = app.current_author_pubkey();
    let long_name = format!("{}@example.com", "a".repeat(65));
    for invalid in [
        "alice@localhost",
        "a b@example.com",
        "alice@192.0.2.1",
        long_name.as_str(),
    ] {
        let error = app
            .set_my_profile(ProfileInput {
                name: Some("alice".into()),
                nip05: Some(invalid.into()),
                ..Default::default()
            })
            .await
            .expect_err(invalid);
        assert!(error.to_string().contains("nip05"), "{invalid}: {error}");
    }
    assert_eq!(
        store.get_profile(author.as_str()).await.expect("profile"),
        None
    );
    assert!(
        author_profile_doc(docs_sync.as_ref(), author.as_str())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn list_profile_timeline_ignores_profile_post_with_signer_mismatch() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service,
        keys,
    );
    let topic = "kukuri:topic:profile-invalid";
    let valid_object_id = app
        .create_post(topic, "valid profile post", None)
        .await
        .expect("valid post");
    let forged_content = KukuriProfilePostEnvelopeContentV1 {
        author_pubkey: Pubkey::from(author_pubkey.as_str()),
        profile_topic_id: author_profile_topic_id(author_pubkey.as_str()),
        published_topic_id: TopicId::new(topic),
        object_id: EnvelopeId::from("forged-profile-post"),
        created_at: 123,
        object_kind: "post".into(),
        content: "forged profile post".into(),
        attachments: Vec::new(),
        reply_to_object_id: None,
        root_id: None,
        content_labels: Vec::new(),
    };
    let forged_envelope = kukuri_core::sign_envelope_json(
        &generate_keys(),
        "profile-post",
        vec![
            vec!["author".into(), author_pubkey.clone()],
            vec!["object".into(), "profile-post".into()],
            vec!["published_topic".into(), topic.into()],
            vec!["post".into(), forged_content.object_id.as_str().to_string()],
        ],
        &forged_content,
    )
    .expect("forged envelope");
    let replica = author_replica_id(author_pubkey.as_str());
    docs_sync
        .open_replica(&replica)
        .await
        .expect("open author replica");
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("profile/posts", forged_content.object_id.as_str()),
                value: serde_json::to_value(AuthorProfilePostDocV1 {
                    author_pubkey: forged_content.author_pubkey.clone(),
                    profile_topic_id: forged_content.profile_topic_id.clone(),
                    published_topic_id: forged_content.published_topic_id.clone(),
                    object_id: forged_content.object_id.clone(),
                    created_at: forged_content.created_at,
                    object_kind: forged_content.object_kind.clone(),
                    content: forged_content.content.clone(),
                    attachments: forged_content.attachments.clone(),
                    reply_to_object_id: None,
                    root_id: None,
                    content_labels: Vec::new(),
                    envelope_id: forged_envelope.id.clone(),
                })
                .expect("forged doc json"),
            },
        )
        .await
        .expect("persist forged profile doc");
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("envelopes", forged_envelope.id.as_str()),
                value: serde_json::to_value(&forged_envelope).expect("forged envelope json"),
            },
        )
        .await
        .expect("persist forged envelope");

    let profile_docs = author_profile_post_docs(docs_sync.as_ref(), author_pubkey.as_str()).await;
    assert_eq!(profile_docs.len(), 2);

    let timeline = app
        .list_profile_timeline(author_pubkey.as_str(), None, 20)
        .await
        .expect("profile timeline");

    assert!(
        timeline
            .items
            .iter()
            .any(|post| post.object_id == valid_object_id)
    );
    assert!(
        timeline
            .items
            .iter()
            .all(|post| post.object_id != forged_content.object_id.as_str())
    );
}

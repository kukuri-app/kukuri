//! #1839 AC-1: Live を開始 post と結び付け、作成した場所の外からも同じ Live を解決する。
use super::sync::CountingDocsSync;
use super::*;
use kukuri_core::LiveRefV1;

/// 書いた docs と blob を共有し、手元の行(store)だけが別の端末。
fn viewer_of(docs: Arc<dyn DocsSync>, blobs: Arc<MemoryBlobService>) -> AppService {
    let store = Arc::new(MemoryStore::default());
    app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs,
        blobs,
        generate_keys(),
    )
}

fn input(start_post: &str) -> CreateLiveSessionInput {
    CreateLiveSessionInput {
        title: "live".into(),
        description: "watch".into(),
        start_post: start_post.into(),
    }
}

fn thumbnail_image(value: u8) -> Vec<u8> {
    let mut image = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut image)
        .encode(&[value, value, value], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    image
}

#[tokio::test]
async fn live_thumbnail_manual_image_wins_and_auto_updates_replace_one_record() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:live-thumbnails";
    display_topic(&owner, topic).await.unwrap();
    let session_id = owner
        .create_live_session(topic, input("watch"))
        .await
        .unwrap();
    let viewer = viewer_of(docs.clone(), blobs);
    resolve(&viewer, topic, &session_id).await.unwrap();
    let make = |custom, image: Option<u8>| crate::SetLiveThumbnailInput {
        topic: topic.into(),
        session_id: session_id.clone(),
        custom,
        data_url: image.map(|bytes| {
            format!(
                "data:image/jpeg;base64,{}",
                BASE64_STANDARD.encode(thumbnail_image(bytes))
            )
        }),
    };
    let automatic = make(false, Some(12));
    let chosen = make(true, Some(240));
    owner.set_live_thumbnail(automatic.clone()).await.unwrap();
    assert_eq!(
        viewer
            .get_live_thumbnail(topic, &session_id)
            .await
            .unwrap()
            .unwrap()
            .data_url,
        automatic.data_url
    );
    assert!(viewer.set_live_thumbnail(chosen.clone()).await.is_err());
    owner.set_live_thumbnail(chosen.clone()).await.unwrap();
    owner.set_live_thumbnail(automatic.clone()).await.unwrap();
    let manual = viewer
        .get_live_thumbnail(topic, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(manual.custom);
    assert_eq!(manual.data_url, chosen.data_url);
    owner.set_live_thumbnail(make(true, None)).await.unwrap();
    for _ in 0..4 {
        owner.set_live_thumbnail(automatic.clone()).await.unwrap();
    }
    let current = viewer
        .get_live_thumbnail(topic, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!current.custom);
    assert_eq!(current.data_url, automatic.data_url);
    let (replica, _, _) = owner
        .fetch_live_session_state_and_manifest(topic, &session_id)
        .await
        .unwrap()
        .unwrap();
    let records = docs
        .query_replica(
            &replica,
            DocQuery::Exact(format!("media/live/{session_id}/thumbnail")),
        )
        .await
        .unwrap();
    assert_eq!(
        records.len(),
        1,
        "automatic updates replace the same record"
    );
    let mut invalid: serde_json::Value = serde_json::from_slice(&records[0].value).unwrap();
    invalid["content"] = serde_json::Value::String("changed after signing".into());
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: records[0].key.clone(),
            value: invalid,
        },
    )
    .await
    .unwrap();
    assert!(
        viewer
            .get_live_thumbnail(topic, &session_id)
            .await
            .unwrap()
            .is_none(),
        "unsigned replacement must not be displayed"
    );
    owner.set_live_thumbnail(automatic.clone()).await.unwrap();
    owner.end_live_session(topic, &session_id).await.unwrap();
    assert!(owner.set_live_thumbnail(automatic).await.is_err());
}

#[tokio::test]
async fn live_thumbnail_keeps_the_private_channel_audience() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:private-live-thumbnail";
    let channel = owner
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "private".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .unwrap();
    let session_id = owner
        .create_live_session_in_channel(
            topic,
            ChannelRef::PrivateChannel {
                channel_id: ChannelId::new(channel.channel_id),
            },
            input("private"),
            None,
        )
        .await
        .unwrap();
    owner
        .set_live_thumbnail(crate::SetLiveThumbnailInput {
            topic: topic.into(),
            session_id: session_id.clone(),
            custom: true,
            data_url: Some(format!(
                "data:image/jpeg;base64,{}",
                BASE64_STANDARD.encode(thumbnail_image(3))
            )),
        })
        .await
        .unwrap();
    let stranger = viewer_of(docs, blobs);
    assert!(resolve(&stranger, topic, &session_id).await.is_none());
    assert!(
        stranger
            .get_live_thumbnail(topic, &session_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn live_thumbnail_reaches_a_private_member_through_remote_docs() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:remote-private-thumbnail";
    let channel = owner
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "private".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .unwrap();
    let invite = owner
        .export_private_channel_invite(topic, &channel.channel_id, None)
        .await
        .unwrap();
    let member_docs = Arc::new(CountingDocsSync::reading_from(docs.clone()));
    let hints = Arc::new(TrackingHintTransport::default());
    *hints.resolved_destination.lock().await = Some(iroh::EndpointAddr::new(
        iroh::SecretKey::from_bytes(&[23; 32]).public(),
    ));
    let store = Arc::new(MemoryStore::default());
    let member = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hints,
        member_docs.clone(),
        blobs,
        generate_keys(),
    );
    member.import_private_channel_invite(&invite).await.unwrap();
    let id = owner
        .create_live_session_in_channel(
            topic,
            ChannelRef::PrivateChannel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
            input("members"),
            None,
        )
        .await
        .unwrap();
    let image = format!(
        "data:image/jpeg;base64,{}",
        BASE64_STANDARD.encode(thumbnail_image(240))
    );
    owner
        .set_live_thumbnail(crate::SetLiveThumbnailInput {
            topic: topic.into(),
            session_id: id.clone(),
            custom: true,
            data_url: Some(image.clone()),
        })
        .await
        .unwrap();
    member
        .set_session_display(crate::SessionDisplayRequest {
            topic: topic.into(),
            scope: TimelineScope::Channel {
                channel_id: ChannelId::new(channel.channel_id),
            },
            session_id: id.clone(),
            kind: "live".into(),
            visible: true,
            replica_id: String::new(),
            observer: "thumbnail-member".into(),
            retry: false,
        })
        .await
        .unwrap();
    let key = format!("media/live/{id}/thumbnail");
    let (replica, _, _) = owner
        .fetch_live_session_state_and_manifest(topic, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        member_docs
            .query_replica_with_policy(
                &replica,
                DocQuery::Exact(key),
                kukuri_docs_sync::DocFetchPolicy::LocalOnly
            )
            .await
            .unwrap()
            .is_empty()
    );
    let found = member
        .get_live_thumbnail(topic, &id)
        .await
        .unwrap()
        .expect("remote member thumbnail");
    assert_eq!(found.data_url, Some(image));
}

#[tokio::test]
async fn live_thumbnail_owner_keeps_priority_checks_local() {
    let remote = Arc::new(CountingDocsSync::default());
    let docs = Arc::new(CountingDocsSync::reading_from(remote.clone()));
    let owner = viewer_of(docs, Arc::new(MemoryBlobService::default()));
    let topic = "kukuri:topic:owner-thumbnail";
    let id = owner.create_live_session(topic, input("")).await.unwrap();
    owner
        .set_live_thumbnail(crate::SetLiveThumbnailInput {
            topic: topic.into(),
            session_id: id.clone(),
            custom: false,
            data_url: Some(format!(
                "data:image/jpeg;base64,{}",
                BASE64_STANDARD.encode(thumbnail_image(12))
            )),
        })
        .await
        .unwrap();
    owner.end_live_session(topic, &id).await.unwrap();
    assert!(
        remote.queries().await.is_empty(),
        "the owner's locally signed live and thumbnail priority checks require no peer reads"
    );
}

async fn post_with_live(app: &AppService, topic: &str, session_id: &str) -> PostView {
    app.list_timeline(topic, None, 50)
        .await
        .expect("timeline")
        .items
        .into_iter()
        .find(|post| {
            post.live
                .as_ref()
                .is_some_and(|live| live.session_id == session_id)
        })
        .expect("post with the live")
}

async fn resolve(app: &AppService, topic: &str, session_id: &str) -> Option<LiveSessionView> {
    display_remote_session(app, topic, session_id, "live").await;
    app.get_live_session(topic, session_id)
        .await
        .expect("get live")
}

// 開始 post は Live を参照し、Live の manifest は開始 post を指す。別の端末は Live の topic を開かずに、id と元の topic から
// 同じ owner・revision・開始 post を読み、終了も同じく読む。後から来た端末(手元の行が無い)も docs と blob から戻す。
#[tokio::test]
async fn start_post_and_live_resolve_from_another_device_and_restore() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:live-object";
    display_topic(&owner, topic).await.expect("open the topic");
    let session_id = owner
        .create_live_session(topic, input("going live"))
        .await
        .expect("create live");

    let start = post_with_live(&owner, topic, &session_id).await;
    assert_eq!(start.content, "going live");
    assert_eq!(
        start.live,
        Some(PostLiveView {
            session_id: session_id.clone(),
            topic_id: topic.into(),
            channel_id: None,
        })
    );
    let own = owner
        .get_live_session(topic, &session_id)
        .await
        .expect("own live")
        .expect("own live exists");
    assert_eq!(own.start_post_id.as_deref(), Some(start.object_id.as_str()));

    let viewer = viewer_of(docs.clone(), blobs.clone());
    assert!(
        viewer
            .get_live_session(topic, &session_id)
            .await
            .expect("before display")
            .is_none(),
        "nothing is read before the display request"
    );
    let seen = resolve(&viewer, topic, &session_id)
        .await
        .expect("resolved");
    assert_eq!(seen.host_pubkey, own.host_pubkey);
    assert_eq!(seen.start_post_id, own.start_post_id);
    assert_eq!(seen.status, LiveSessionStatus::Live);

    owner
        .end_live_session(topic, &session_id)
        .await
        .expect("end live");
    let ended = resolve(&viewer, topic, &session_id)
        .await
        .expect("resolved ended");
    assert_eq!(ended.status, LiveSessionStatus::Ended);
    assert!(ended.ended_at.is_some());
    assert_eq!(ended.start_post_id, own.start_post_id);
    let error = viewer
        .join_live_session(topic, &session_id)
        .await
        .expect_err("ended live rejects viewers");
    assert!(error.to_string().contains("ended"));

    let late = viewer_of(docs, blobs);
    let restored = resolve(&late, topic, &session_id).await.expect("restored");
    assert_eq!(restored.status, LiveSessionStatus::Ended);
    assert_eq!(restored.start_post_id, own.start_post_id);
    for app in [owner, viewer, late] {
        app.shutdown().await;
    }
}

// 第三者は別の topic に同じ Live の紹介 post を作れる。紹介は Live の id と元の場所だけを持ち、Live の owner・開始 post・
// 公開範囲は変わらない。読めていない Live は紹介できない。
#[tokio::test]
async fn third_party_introduces_the_same_live_in_another_topic() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:live-origin";
    let other = "kukuri:topic:live-intro";
    let session_id = owner
        .create_live_session(topic, input("start"))
        .await
        .expect("create live");
    let own = owner
        .get_live_session(topic, &session_id)
        .await
        .expect("own")
        .expect("own exists");

    let third = viewer_of(docs, blobs);
    display_topic(&third, other)
        .await
        .expect("open the other topic");
    let live = LiveRefV1 {
        session_id: session_id.clone(),
        topic_id: TopicId::new(topic),
        channel_id: None,
    };
    let error = third
        .create_post_with_attachments_in_channel(
            other,
            ChannelRef::Public,
            "look",
            None,
            Vec::new(),
            Vec::new(),
            Some(live.clone()),
        )
        .await
        .expect_err("an unread live cannot be introduced");
    assert!(error.to_string().contains("live session not found"));

    resolve(&third, topic, &session_id).await.expect("resolved");
    let intro_id = third
        .create_post_with_attachments_in_channel(
            other,
            ChannelRef::Public,
            "look",
            None,
            Vec::new(),
            Vec::new(),
            Some(live),
        )
        .await
        .expect("introduce");
    let intro = post_with_live(&third, other, &session_id).await;
    assert_eq!(intro.object_id, intro_id);
    assert_eq!(intro.content, "look");
    assert_eq!(intro.published_topic_id.as_deref(), Some(other));
    assert_eq!(
        intro.live.as_ref().map(|live| live.topic_id.as_str()),
        Some(topic)
    );
    let after = third
        .get_live_session(topic, &session_id)
        .await
        .expect("after intro")
        .expect("still resolved");
    assert_eq!(after.host_pubkey, own.host_pubkey);
    assert_eq!(after.start_post_id, own.start_post_id);
    assert_ne!(after.start_post_id.as_deref(), Some(intro_id.as_str()));
    assert_eq!(after.channel_id, None);
    owner.shutdown().await;
    third.shutdown().await;
}

// 限定 channel の Live は、開始 post が同じ channel に置かれ、その channel の中でだけ紹介できる。参加していない端末と、
// 退出した端末には返さない。公開の場所の post が限定 Live を指していても、参照を持たない投稿として扱う。
#[tokio::test]
async fn limited_live_stays_in_its_channel() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:live-limited";
    let channel = owner
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "core".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let in_channel = ChannelRef::PrivateChannel {
        channel_id: channel_id.clone(),
    };
    // 招待の発行は epoch を新しくするので、参加を先に済ませてから Live を作る。
    let member = viewer_of(docs.clone(), blobs.clone());
    let invite = owner
        .export_private_channel_invite(topic, channel.channel_id.as_str(), None)
        .await
        .expect("invite");
    member
        .import_private_channel_invite(invite.as_str())
        .await
        .expect("join");
    let session_id = owner
        .create_live_session_in_channel(topic, in_channel.clone(), input("members"), None)
        .await
        .expect("create limited live");
    let live = LiveRefV1 {
        session_id: session_id.clone(),
        topic_id: TopicId::new(topic),
        channel_id: Some(channel_id.clone()),
    };
    // 開始 post は Live と同じ channel に置かれ、その行は限定 Live への参照を持つ。
    let start_post_id = owner
        .get_live_session(topic, &session_id)
        .await
        .expect("own")
        .and_then(|view| view.start_post_id)
        .expect("start post id");
    let start = owner
        .services
        .projection_store
        .get_object_projection(&EnvelopeId::from(start_post_id.as_str()))
        .await
        .expect("start row")
        .expect("start post stored");
    assert_eq!(start.channel_id, channel.channel_id);
    assert_eq!(start.live, Some(live.clone()));

    for (target, scope) in [
        (topic, ChannelRef::Public),
        ("kukuri:topic:live-elsewhere", ChannelRef::Public),
    ] {
        let error = owner
            .create_post_with_attachments_in_channel(
                target,
                scope,
                "leak",
                None,
                Vec::new(),
                Vec::new(),
                Some(live.clone()),
            )
            .await
            .expect_err("a limited live stays in its channel");
        assert!(
            error
                .to_string()
                .contains("only be referenced in its channel")
        );
    }
    owner
        .create_post_with_attachments_in_channel(
            topic,
            in_channel,
            "inside",
            None,
            Vec::new(),
            Vec::new(),
            Some(live.clone()),
        )
        .await
        .expect("introduce inside the channel");

    // 公開の場所に置かれた、限定 Live を指す post(別の実装が作ったもの)。
    let crafted = build_post_envelope_with_docs_author(
        owner.services.keys.as_ref(),
        &TopicId::new(topic),
        PayloadRef::InlineText {
            text: "crafted".into(),
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
        None,
        Some(live.clone()),
    )
    .expect("crafted post");
    let replica = owner
        .services
        .scope_write_replica(topic, None, crafted.created_at)
        .expect("public replica");
    owner
        .ingest_event(&replica, crafted.clone())
        .await
        .expect("ingest crafted");
    let public = owner
        .list_timeline(topic, None, 20)
        .await
        .expect("public timeline")
        .items
        .into_iter()
        .find(|post| post.object_id == crafted.id.as_str())
        .expect("crafted post listed");
    assert_eq!(public.live, None);

    let outsider = viewer_of(docs, blobs);
    assert!(
        outsider
            .get_live_session(topic, &session_id)
            .await
            .expect("outsider")
            .is_none()
    );

    member
        .set_session_display(crate::SessionDisplayRequest {
            topic: topic.into(),
            scope: TimelineScope::Channel {
                channel_id: channel_id.clone(),
            },
            replica_id: String::new(),
            session_id: session_id.clone(),
            kind: "live".into(),
            observer: "test-member".into(),
            visible: true,
            retry: false,
        })
        .await
        .expect("display the limited live");
    let seen = member
        .get_live_session(topic, &session_id)
        .await
        .expect("member")
        .expect("member reads the limited live");
    assert_eq!(seen.start_post_id, Some(start_post_id));
    member
        .leave_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("leave");
    assert!(
        member
            .get_live_session(topic, &session_id)
            .await
            .expect("after leaving")
            .is_none()
    );
    for app in [owner, outsider, member] {
        app.shutdown().await;
    }
}

// 開始 post の無い旧版の Live(manifest に `start_post_id` が無い)は、開始 post の無い Live として読む。
#[tokio::test]
async fn legacy_live_without_a_start_post_is_read_as_is() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:live-legacy";
    let session_id = owner
        .create_live_session(topic, input(""))
        .await
        .expect("create live");
    let (replica, state, mut manifest) = owner
        .fetch_live_session_state_and_manifest(topic, &session_id)
        .await
        .expect("read")
        .expect("exists");
    manifest.revision += 1;
    manifest.start_post_id = None;
    owner
        .persist_live_session_manifest(&replica, topic, manifest, state.created_at)
        .await
        .expect("legacy manifest");

    let viewer = viewer_of(docs, blobs);
    let seen = resolve(&viewer, topic, &session_id)
        .await
        .expect("resolved");
    assert_eq!(seen.start_post_id, None);
    assert_eq!(seen.status, LiveSessionStatus::Live);
    owner.shutdown().await;
    viewer.shutdown().await;
}

// 対象の Live を id と元の topic から読む処理は、同じ topic の無関係な投稿・Live の件数で増えない。
#[tokio::test]
async fn resolving_a_live_does_not_grow_with_unrelated_records() {
    let (owner, _, source_docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:live-scale";
    let session_id = owner
        .create_live_session(topic, input("scale"))
        .await
        .expect("create live");
    let docs = Arc::new(CountingDocsSync::default());
    // 切替前の writer は、投稿も Live の state も topic の replica に置く。
    let replicas = vec![topic_replica_id(topic)];
    for replica in &replicas {
        docs.open_replica(replica).await.expect("open");
        for record in source_docs
            .query_replica(replica, DocQuery::Prefix(String::new()))
            .await
            .expect("records")
        {
            docs.apply_doc_op(
                replica,
                DocOp::SetBytes {
                    key: record.key,
                    value: record.value,
                },
            )
            .await
            .expect("copy");
        }
    }
    let mut baseline = None;
    for count in [0, 500] {
        for replica in &replicas {
            for index in 0..count {
                for key in [
                    format!("sessions/live/unrelated-{index}/state"),
                    format!("objects/unrelated-{index}/state"),
                ] {
                    docs.apply_doc_op(
                        replica,
                        DocOp::SetJson {
                            key,
                            value: serde_json::json!({}),
                        },
                    )
                    .await
                    .expect("unrelated");
                }
            }
        }
        let viewer = viewer_of(docs.clone(), blobs.clone());
        docs.reset_records_returned();
        docs.clear_queries().await;
        let seen = resolve(&viewer, topic, &session_id)
            .await
            .expect("resolved");
        assert!(seen.start_post_id.is_some());
        let observed = docs.records_returned();
        assert_eq!(observed, *baseline.get_or_insert(observed), "{count}");
        assert!(
            docs.queries()
                .await
                .iter()
                .all(|(_, query)| !matches!(query, DocQuery::Prefix(prefix) if prefix.is_empty())),
            "no full replica read"
        );
        viewer.shutdown().await;
    }
    owner.shutdown().await;
}

// #1839 AC-2: A の開始 post の Live を B が別の topic で紹介し、C は紹介 post から同じ Live を見つけて参加する。返信と
// リアクションは開始 post に付き、紹介 post 自身のリアクションとは合算しない。同じ場所の 2 つ目の Live は独立している。
#[tokio::test]
async fn introduction_leads_to_the_same_live_and_its_start_post() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let origin = "kukuri:topic:live-flow-origin";
    let elsewhere = "kukuri:topic:live-flow-elsewhere";
    let session_id = owner
        .create_live_session(origin, input("start"))
        .await
        .expect("create live");
    let introducer = viewer_of(docs.clone(), blobs.clone());
    resolve(&introducer, origin, &session_id)
        .await
        .expect("introducer resolves");
    let intro_id = introducer
        .create_post_with_attachments_in_channel(
            elsewhere,
            ChannelRef::Public,
            "look",
            None,
            Vec::new(),
            Vec::new(),
            Some(LiveRefV1 {
                session_id: session_id.clone(),
                topic_id: TopicId::new(origin),
                channel_id: None,
            }),
        )
        .await
        .expect("introduce");

    let viewer = viewer_of(docs, blobs);
    display_topic(&viewer, elsewhere)
        .await
        .expect("open elsewhere");
    let intro = post_with_live(&viewer, elsewhere, &session_id).await;
    assert_eq!(intro.object_id, intro_id);
    let live = intro.live.expect("intro refers to the live");
    let found = resolve(&viewer, &live.topic_id, &live.session_id)
        .await
        .expect("viewer resolves from the intro");
    let start_post_id = found.start_post_id.expect("start post");
    viewer
        .join_live_session(&live.topic_id, &live.session_id)
        .await
        .expect("join from the intro");

    let reply_id = viewer
        .create_post(origin, "hi", Some(start_post_id.as_str()))
        .await
        .expect("reply to the start post");
    let thread = viewer
        .list_thread(origin, &start_post_id, None, 20)
        .await
        .expect("thread");
    assert!(thread.items.iter().any(|post| post.object_id == reply_id));
    let emoji = |emoji: &str| ReactionKeyV1::Emoji {
        emoji: emoji.into(),
    };
    let on_start = viewer
        .toggle_reaction(
            origin,
            &start_post_id,
            emoji("👍"),
            Some(ChannelRef::Public),
        )
        .await
        .expect("react to the start post");
    let on_intro = viewer
        .toggle_reaction(elsewhere, &intro_id, emoji("🎉"), Some(ChannelRef::Public))
        .await
        .expect("react to the intro");
    assert_eq!(on_start.target_object_id, start_post_id);
    let keys = |state: &ReactionStateView| {
        state
            .reaction_summary
            .iter()
            .map(|summary| (summary.emoji.clone(), summary.count))
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&on_start), vec![(Some("👍".to_string()), 1)]);
    assert_eq!(keys(&on_intro), vec![(Some("🎉".to_string()), 1)]);

    let second = owner
        .create_live_session(origin, input("second"))
        .await
        .expect("second live");
    let sessions = owner.list_live_sessions(origin).await.expect("list");
    let starts = sessions
        .iter()
        .filter(|session| {
            [session_id.as_str(), second.as_str()].contains(&session.session_id.as_str())
        })
        .map(|session| {
            session
                .start_post_id
                .clone()
                .expect("each live has its start post")
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        starts.len(),
        2,
        "two lives in one place keep their own start posts"
    );
    for app in [owner, introducer, viewer] {
        app.shutdown().await;
    }
}

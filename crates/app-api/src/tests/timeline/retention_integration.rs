//! #1909 AC-2: 公開 topic を保持する K は、画面の読みをせずに保持の取り込みだけで、topic の投稿(本文の blob・画像・動画・
//! リアクション)、公開スレッド(タイトルを変えた版 1)とその投稿(画像つき)、終えた Live(版 2)、Dome(session・Instance・
//! Preset・資材)を取り込む。A を止めた後に来た B は、K を同じ topic の参加者として知るだけ(K 専用の識別子も優先の設定も
//! 持たない)で、K から同じ内容の新しい版を読む。
use super::super::*;
use super::relay_integration::{Author, ChangingNeighbors, Relay, eventually, seed, ticket};
use super::remote_reader_integration::ScopedReadHints;
use kukuri_core::{BlobHash, MetaverseAssetKind};

fn blob_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

async fn has_payload(
    app: &AppService,
    bytes: &[u8],
    mime: &str,
    post: Option<&str>,
) -> Result<bool> {
    let payload = app
        .blob_media_payload_for_post(&blob_hash(bytes), mime, post)
        .await?;
    Ok(payload.is_some_and(|payload| payload.bytes_base64 == BASE64_STANDARD.encode(bytes)))
}

#[tokio::test]
async fn later_reader_reads_retained_topic_content_from_the_retaining_peer() -> Result<()> {
    let topic = TopicId::new("kukuri:topic:retention-relay");
    let author = Author::new().await?;
    let store = Arc::new(MemoryStore::default());
    let a = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        Arc::new(author.docs.clone()),
        Arc::new(author.blobs.clone()),
        author.keys.clone(),
    );
    a.switch_writer(1);

    // A は topic の投稿(本文の blob・画像・動画)とリアクション、公開スレッド(版 1)と画像つきの投稿、終えた Live、
    // 資材を足した Dome を作る。
    let body = "保持する投稿の本文。".repeat(8);
    let (image, video, poster) = (
        b"retained-image".to_vec(),
        b"retained-video".to_vec(),
        b"retained-poster".to_vec(),
    );
    let attachment = |mime: &str, bytes: &[u8], role| PendingAttachment {
        mime: mime.into(),
        bytes: bytes.to_vec(),
        role,
    };
    let post = a
        .create_post_with_attachments(
            topic.as_str(),
            &body,
            None,
            vec![
                attachment("image/png", &image, AssetRole::ImageOriginal),
                attachment("video/mp4", &video, AssetRole::VideoManifest),
                attachment("image/jpeg", &poster, AssetRole::VideoPoster),
            ],
        )
        .await?;
    a.toggle_reaction(
        topic.as_str(),
        &post,
        ReactionKeyV1::Emoji {
            emoji: "👍".into()
        },
        None,
    )
    .await?;
    let channel = a
        .create_public_channel(topic.as_str(), "版 0", "最初の投稿", Vec::new(), Vec::new())
        .await?;
    a.update_public_channel_title(topic.as_str(), &channel.channel_id, "版 1")
        .await?;
    let thread_image = b"retained-thread-image".to_vec();
    let thread_post = a
        .create_post_with_attachments_in_channel(
            topic.as_str(),
            ChannelRef::PublicChannel {
                channel_id: ChannelId::new(channel.channel_id.as_str()),
            },
            "スレッドの投稿",
            None,
            vec![attachment(
                "image/png",
                &thread_image,
                AssetRole::ImageOriginal,
            )],
            Vec::new(),
            None,
        )
        .await?;
    let live = a
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "終えた Live".into(),
                description: String::new(),
                start_post: String::new(),
            },
        )
        .await?;
    a.end_live_session(topic.as_str(), &live).await?;
    let dome = a
        .create_metaverse_room(
            topic.as_str(),
            CreateMetaverseRoomInput {
                title: "保持する Dome".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await?;
    let asset_bytes = minimal_metaverse_glb_bytes();
    let asset = a
        .import_metaverse_room_asset(
            topic.as_str(),
            ImportMetaverseRoomAssetInput {
                room_id: dome.clone(),
                kind: MetaverseAssetKind::Vrm,
                mime_type: "model/vrm".into(),
                name: Some("avatar.vrm".into()),
                bytes: asset_bytes.clone(),
            },
        )
        .await?;

    // K は保持を設定するだけで(画面の読みをしない)、A の内容と参照する blob を取り込む。
    let dir = tempfile::tempdir()?;
    let k = Relay::with_store(
        Arc::new(NoopHintTransport),
        SqliteStore::connect_file(dir.path().join("k.db")).await?,
    )
    .await?;
    k.app.import_peer_ticket(&ticket(&author.node)).await?;
    k.app.set_topic_retention(topic.as_str(), true).await?;
    let held = || async {
        let projection = k.app.services.projection_store.as_ref();
        let blobs = k.app.services.blob_service.as_ref();
        let status = k.app.topic_retention().await?;
        let mut held = vec![(
            "no retries",
            status
                .topics
                .iter()
                .all(|retained| retained.pending_retries == 0 && retained.given_up == 0),
        )];
        held.push((
            "post",
            projection
                .get_object_projection(&EnvelopeId::from(post.as_str()))
                .await?
                .is_some(),
        ));
        held.push((
            "thread post",
            projection
                .get_object_projection(&EnvelopeId::from(thread_post.as_str()))
                .await?
                .is_some(),
        ));
        held.push((
            "thread version 1",
            projection
                .get_public_channel_row(&channel.channel_id)
                .await?
                .is_some_and(|row| row.title == "版 1"),
        ));
        held.push((
            "ended live",
            projection
                .get_live_session(topic.as_str(), &live)
                .await?
                .is_some_and(|row| row.status == LiveSessionStatus::Ended),
        ));
        held.push((
            "dome",
            projection
                .get_game_room(topic.as_str(), &dome)
                .await?
                .is_some(),
        ));
        for (name, bytes) in [
            ("body", &body.as_bytes().to_vec()),
            ("image", &image),
            ("video", &video),
            ("poster", &poster),
            ("thread image", &thread_image),
            ("dome asset", &asset_bytes),
        ] {
            held.push((
                name,
                blobs
                    .local_blob_status(&BlobHash::new(blob_hash(bytes)))
                    .await?
                    != BlobStatus::Missing,
            ));
        }
        Ok::<_, anyhow::Error>((held, status))
    };
    let retained =
        eventually(|| async { Ok(held().await?.0.iter().all(|(_, held)| *held)) }).await?;
    assert!(
        retained,
        "K retains the topic's content and blobs: {:?}",
        held().await?
    );
    a.shutdown().await;
    author.go_offline().await?;

    // B は K を同じ topic の参加者(gossip の近隣)として知るだけで、全体の台帳には何も無い。
    let b = Relay::new(Arc::new(ScopedReadHints(seed(&k.node)))).await?;
    let timeline = eventually(|| async {
        let page = b.app.list_timeline(topic.as_str(), None, 20).await?;
        Ok(page.items.iter().any(|item| {
            item.object_id == post
                && item.content == body
                && item
                    .reaction_summary
                    .iter()
                    .any(|reaction| reaction.emoji.as_deref() == Some("👍") && reaction.count == 1)
        }))
    })
    .await?;
    assert!(
        timeline,
        "B reads the post, its body and its reaction from K"
    );
    for (bytes, mime) in [
        (&image, "image/png"),
        (&video, "video/mp4"),
        (&poster, "image/jpeg"),
    ] {
        assert!(
            has_payload(&b.app, bytes, mime, Some(&post)).await?,
            "B reads the {mime} attachment from K"
        );
    }
    let thread = b
        .app
        .get_public_channel(topic.as_str(), &channel.channel_id)
        .await?
        .expect("B reads the public channel from K");
    assert_eq!(thread.title, "版 1", "B reads the renamed version");
    let scope = TimelineScope::PublicChannel {
        channel_id: ChannelId::new(channel.channel_id.as_str()),
    };
    let listed = eventually(|| async {
        let page = b
            .app
            .list_timeline_scoped(topic.as_str(), scope.clone(), None, 20)
            .await?;
        Ok(page.items.iter().any(|item| item.object_id == thread_post))
    })
    .await?;
    assert!(listed, "B reads the public channel's post from K");
    assert!(has_payload(&b.app, &thread_image, "image/png", Some(&thread_post)).await?);
    // session は、列を開いたときの lease の読み直しで届く。
    b.app
        .reread_scope(topic.as_str(), &TimelineScope::Public)
        .await;
    let sessions = b.app.list_live_sessions(topic.as_str()).await?;
    assert!(
        sessions
            .iter()
            .any(|session| session.session_id == live && session.status == LiveSessionStatus::Ended),
        "B reads the ended version of the Live from K: {sessions:?}"
    );
    let rooms = b.app.list_game_rooms(topic.as_str()).await?;
    assert!(
        rooms.iter().any(|room| room.room_id == dome),
        "B lists the Dome from K: {rooms:?}"
    );
    assert!(has_payload(&b.app, &asset_bytes, "model/vrm", None).await?);
    assert_eq!(asset.blob_hash, blob_hash(&asset_bytes));

    b.finish().await?;
    k.finish().await
}

/// 端末が保持する(record cache の、他の名義を含む)Dome Instance の state が指す manifest の hash。
async fn held_instance(app: &AppService, replica: &ReplicaId, key: &str) -> Result<Option<String>> {
    Ok(app
        .services
        .docs_sync
        .query_local_source(replica, key, None, 8)
        .await?
        .iter()
        .find_map(|record| {
            serde_json::from_slice::<kukuri_core::DomeInstanceStateDocV1>(&record.value).ok()
        })
        .map(|state| state.current_manifest.hash.as_str().to_string()))
}

/// #1909 AC-2: 保持する K2 は、手元に保持した新しい版の Dome Instance を、古い版を持つ参加者 K1 から読んだ版で上書き
/// しない。保持した Preset は手元から読むので、owner が止まっていても取り直しを待たない。K2 だけを知る B は新しい版を読む。
#[tokio::test]
async fn a_retained_dome_is_not_rolled_back_by_a_participant_with_an_older_version() -> Result<()> {
    let topic = TopicId::new("kukuri:topic:retention-rollback");
    let author = Author::new().await?;
    let store = Arc::new(MemoryStore::default());
    let a = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        Arc::new(author.docs.clone()),
        Arc::new(author.blobs.clone()),
        author.keys.clone(),
    );
    a.switch_writer(1);
    let dome = a
        .create_metaverse_room(
            topic.as_str(),
            CreateMetaverseRoomInput {
                title: "版の後退の確認".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await?;
    let owner = Pubkey::from(a.current_author_pubkey().as_str());
    let context = kukuri_core::SpatialContextV1::Topic {
        topic_id: topic.clone(),
    };
    let replica = kukuri_docs_sync::author_replica_id(owner.as_str());
    let key = kukuri_docs_sync::stable_key(
        "metaverse/dome-instances",
        &format!("{}/state", dome_instance_id(&context, &owner)),
    );
    let version = || async {
        let (state, _) = a
            .fetch_dome_instance_manifest(&context, &owner)
            .await?
            .expect("Dome Instance");
        Ok::<_, anyhow::Error>(state.current_manifest.hash.as_str().to_string())
    };
    let v1 = version().await?;

    // K1 は版 1 を保持してから保持をやめる(cache の行は残り、他の参加者へ返す)。
    let dir = tempfile::tempdir()?;
    let k1 = Relay::with_store(
        Arc::new(NoopHintTransport),
        SqliteStore::connect_file(dir.path().join("k1.db")).await?,
    )
    .await?;
    k1.app.import_peer_ticket(&ticket(&author.node)).await?;
    k1.app.set_topic_retention(topic.as_str(), true).await?;
    let k1_held = eventually(|| async {
        Ok(held_instance(&k1.app, &replica, &key).await? == Some(v1.clone()))
    })
    .await?;
    assert!(k1_held, "K1 holds the version 1");
    k1.app.set_topic_retention(topic.as_str(), false).await?;

    // A は資材を足して版 2 にする。K2 は版 2 と Preset を、取り直しを残さずに保持する。
    a.import_metaverse_room_asset(
        topic.as_str(),
        ImportMetaverseRoomAssetInput {
            room_id: dome,
            kind: kukuri_core::MetaverseAssetKind::Vrm,
            mime_type: "model/vrm".into(),
            name: Some("avatar.vrm".into()),
            bytes: minimal_metaverse_glb_bytes(),
        },
    )
    .await?;
    let v2 = version().await?;
    assert_ne!(v1, v2, "the Instance changed");
    let neighbors = Arc::new(ChangingNeighbors(std::sync::Mutex::new(Vec::new())));
    let k2 = Relay::with_store(
        neighbors.clone(),
        SqliteStore::connect_file(dir.path().join("k2.db")).await?,
    )
    .await?;
    k2.app.import_peer_ticket(&ticket(&author.node)).await?;
    k2.app.set_topic_retention(topic.as_str(), true).await?;
    let counts = || async {
        Ok::<_, anyhow::Error>(
            k2.app
                .topic_retention()
                .await?
                .topics
                .iter()
                .map(|topic| (topic.pending_retries, topic.given_up))
                .collect::<Vec<_>>(),
        )
    };
    let k2_held = eventually(|| async {
        Ok(
            held_instance(&k2.app, &replica, &key).await? == Some(v2.clone())
                && counts().await? == [(0, 0)],
        )
    })
    .await?;
    assert!(k2_held, "K2 holds the version 2 with no retries");

    // A が止まり、K2 の topic の参加者に K1 が現れる。K2 の作り直しの後の最初の回(反映した版の記録が空)を回す。
    a.shutdown().await;
    author.go_offline().await?;
    *neighbors.0.lock().expect("neighbors") = vec![seed(&k1.node)];
    k2.app
        .step_topic_retention(
            topic.as_str(),
            &mut crate::topic_retention::RetentionState::default(),
        )
        .await?;
    assert_eq!(
        held_instance(&k2.app, &replica, &key).await?,
        Some(v2.clone()),
        "K2 keeps the version 2"
    );
    assert_eq!(
        counts().await?,
        [(0, 0)],
        "K2 reads its held Preset without a retry"
    );

    // K2 だけを知る B は版 2 を読む。
    let b = Relay::new(Arc::new(ScopedReadHints(seed(&k2.node)))).await?;
    let read = b
        .app
        .fetch_dome_instance_manifest(&context, &owner)
        .await?
        .map(|(state, _)| state.current_manifest.hash.as_str().to_string());
    assert_eq!(read, Some(v2), "B reads the version 2 from K2");
    b.finish().await?;
    k2.finish().await?;
    k1.finish().await
}

//! 本人がオフラインの後に来た参加者が、同じ topic の参加者から取得する(#1395)。
use super::super::*;
use super::remote_reader_integration::ScopedReadHints;
use kukuri_core::AssetRef;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

fn seed(node: &kukuri_iroh_node::IrohDocsNode) -> SeedPeer {
    let socket = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    SeedPeer {
        endpoint_id: node.endpoint().addr().id.to_string(),
        addr_hint: Some(socket.to_string()),
    }
}

fn ticket(node: &kukuri_iroh_node::IrohDocsNode) -> String {
    let peer = seed(node);
    format!("{}@{}", peer.endpoint_id, peer.addr_hint.expect("socket"))
}

/// 投稿者 A。自分の bucket へ書き、後でオフラインになる。
struct Author {
    node: Arc<kukuri_iroh_node::IrohDocsNode>,
    docs: kukuri_docs_sync::IrohDocsSync,
    blobs: IrohBlobService,
    keys: KukuriKeys,
    docs_author: String,
}

impl Author {
    async fn new() -> Result<Self> {
        let node = kukuri_iroh_node::IrohDocsNode::memory().await?;
        let docs = kukuri_docs_sync::IrohDocsSync::new(node.clone());
        let keys = generate_keys();
        let docs_author = docs
            .use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
            .await?;
        Ok(Self {
            blobs: IrohBlobService::new(node.clone()),
            node,
            docs,
            keys,
            docs_author,
        })
    }

    async fn post(
        &self,
        topic: &TopicId,
        text: &str,
        reply_to: Option<&KukuriEnvelope>,
    ) -> Result<KukuriEnvelope> {
        self.post_with(
            topic,
            PayloadRef::InlineText { text: text.into() },
            Vec::new(),
            reply_to,
        )
        .await
    }

    async fn post_with(
        &self,
        topic: &TopicId,
        payload: PayloadRef,
        attachments: Vec<AssetRef>,
        reply_to: Option<&KukuriEnvelope>,
    ) -> Result<KukuriEnvelope> {
        let post = build_post_envelope_with_docs_author(
            &self.keys,
            topic,
            payload,
            attachments,
            Vec::new(),
            reply_to,
            ObjectVisibility::Public,
            None,
            Vec::new(),
            Some(&self.docs_author),
        )?;
        let replica = BucketReplica::new(
            BucketScope::Topic {
                topic_id: topic.as_str().into(),
            },
            TimeBucket::from_unix_seconds(post.created_at)?,
        )?
        .replica_id();
        persist_post_object(
            &self.docs,
            &replica,
            post.to_post_object()?.expect("post"),
            post.clone(),
        )
        .await?;
        Ok(post)
    }

    /// 自分の投稿を取り下げ、投稿の bucket へ書く。
    async fn withdraw(&self, post: &KukuriEnvelope) -> Result<()> {
        let envelope = build_post_withdrawal_envelope(
            &self.keys,
            post,
            1,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )?;
        let replica = BucketReplica::new(
            BucketScope::Topic {
                topic_id: post.topic_id().expect("topic").as_str().into(),
            },
            TimeBucket::from_unix_seconds(post.created_at)?,
        )?
        .replica_id();
        persist_post_withdrawal(
            &self.docs,
            &WithdrawalWriteRow {
                withdrawal_envelope_id: envelope.id.clone(),
                replica_id: replica,
                target_object_id: post.id.clone(),
                envelope,
                target_replica_id: None,
            },
        )
        .await
    }

    /// 投稿に reaction を付け、投稿の bucket へ書く。
    async fn react(&self, post: &KukuriEnvelope) -> Result<()> {
        let topic = post.topic_id().expect("topic");
        let replica = BucketReplica::new(
            BucketScope::Topic {
                topic_id: topic.as_str().into(),
            },
            TimeBucket::from_unix_seconds(post.created_at)?,
        )?
        .replica_id();
        let reaction_key = ReactionKeyV1::Emoji {
            emoji: "👍".into()
        };
        let reaction_id = deterministic_reaction_id(
            &replica,
            &post.id,
            &self.keys.public_key(),
            reaction_key.normalized_key()?.as_str(),
        );
        let envelope = build_reaction_envelope(
            &self.keys,
            &topic,
            None,
            &post.id,
            reaction_key,
            &reaction_id,
            ObjectStatus::Active,
        )?;
        let reaction = parse_reaction(&envelope)?.expect("reaction doc");
        persist_reaction_doc(&self.docs, &replica, &reaction, &envelope).await
    }

    /// 表示名とアバターの profile を自分の author replica へ書く。
    async fn set_profile(&self, display_name: &str, avatar: AssetRef) -> Result<()> {
        let envelope = build_profile_envelope_with_docs_author(
            &self.keys,
            &KukuriProfileEnvelopeContentV1 {
                author_pubkey: self.keys.public_key(),
                name: None,
                display_name: Some(display_name.into()),
                about: None,
                picture_asset: Some(avatar),
            },
            Some(&self.docs_author),
        )?;
        let profile = parse_profile(&envelope)?.expect("profile");
        crate::service::profile_docs_support::persist_profile_doc(&self.docs, &profile, &envelope)
            .await
    }

    async fn go_offline(self) -> Result<()> {
        self.docs.shutdown().await;
        self.node.shutdown().await
    }
}

/// 取得した内容を remote cache に持つ参加者(本番と同じ `with_account_store` の構成)。
struct Relay {
    node: Arc<kukuri_iroh_node::IrohDocsNode>,
    docs: Arc<kukuri_docs_sync::IrohDocsSync>,
    app: AppService,
}

impl Relay {
    async fn new(hints: Arc<dyn HintTransport>) -> Result<Self> {
        let node = kukuri_iroh_node::IrohDocsNode::memory().await?;
        let store = Arc::new(SqliteStore::connect_memory().await?);
        node.install_remote_cache(store.clone())?;
        let docs = Arc::new(kukuri_docs_sync::IrohDocsSync::with_account_store(
            node.clone(),
            store.clone(),
        ));
        let blobs = Arc::new(IrohBlobService::with_account_store(
            node.clone(),
            store.clone(),
        ));
        // 本番と同じく、アカウントの鍵から導いた docs author で書く(ADR 0053)。
        let keys = generate_keys();
        docs.use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
            .await?;
        let app = app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            hints,
            docs.clone(),
            blobs,
            keys,
        );
        Ok(Self { node, docs, app })
    }

    async fn finish(self) -> Result<()> {
        self.app.shutdown().await;
        self.docs.shutdown().await;
        self.node.shutdown().await
    }
}

fn has(page: &TimelineView, post: &KukuriEnvelope) -> bool {
    page.items
        .iter()
        .any(|item| item.object_id == post.id.as_str())
}

/// AC-1: A の投稿を取得した B は、A がオフラインでも、後から来た C へ一覧(timeline と thread)を返す。
/// C の全体の台帳にはオフラインの A だけがあり、B は同じ topic の参加者としてだけ分かる。
#[tokio::test]
async fn later_reader_lists_an_offline_authors_posts_from_a_topic_peer() -> Result<()> {
    let topic = TopicId::new("relay-offline-author");
    let author = Author::new().await?;
    let root = author.post(&topic, "relayed root", None).await?;
    let reply = author.post(&topic, "relayed reply", Some(&root)).await?;
    let author_ticket = ticket(&author.node);

    let relay = Relay::new(Arc::new(NoopHintTransport)).await?;
    relay.docs.import_peer_ticket(&author_ticket).await?;
    let relay_page = relay.app.list_timeline(topic.as_str(), None, 20).await?;
    assert!(has(&relay_page, &root) && has(&relay_page, &reply));
    author.go_offline().await?;

    let reader = Relay::new(Arc::new(ScopedReadHints(seed(&relay.node)))).await?;
    reader.docs.import_peer_ticket(&author_ticket).await?;
    let page = reader.app.list_timeline(topic.as_str(), None, 20).await?;
    assert!(
        has(&page, &root) && has(&page, &reply),
        "the timeline must list the offline author's posts from the topic peer"
    );
    let thread = reader
        .app
        .list_thread(topic.as_str(), root.id.as_str(), None, 20)
        .await?;
    assert!(has(&thread, &root) && has(&thread, &reply));

    reader.finish().await?;
    relay.finish().await
}

/// 条件が成り立つまで待つ。本文の取り直しは背景で進み、照合済みの範囲の読み直しは同じ提供者に対して
/// `RANGE_CHECK_INTERVAL_MS`(30 秒)の間隔を置くので、その 2 倍まで待つ。
async fn eventually<F, Fut>(mut check: F) -> Result<bool>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    while tokio::time::Instant::now() < deadline {
        if check().await? {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Ok(false)
}

fn content_of(page: &TimelineView, post: &KukuriEnvelope) -> Option<String> {
    page.items
        .iter()
        .find(|item| item.object_id == post.id.as_str())
        .map(|item| item.content.clone())
}

/// AC-2: 見出しを返した B は、その本文と、B が表示した画像を持つ。C の blob の台帳にはオフラインの A しか無く
/// (B は巡回する候補窓の外)、それでも C は本文と画像を B から取る。
#[tokio::test]
async fn later_reader_fetches_the_body_and_image_from_the_peer_that_listed_them() -> Result<()> {
    let topic = TopicId::new("relay-offline-body");
    let author = Author::new().await?;
    let body = author
        .blobs
        .put_blob(b"relayed body".to_vec(), "text/plain")
        .await?;
    let image = author
        .blobs
        .put_blob(b"relayed image bytes".to_vec(), "image/png")
        .await?;
    let post = author
        .post_with(
            &topic,
            PayloadRef::BlobText {
                hash: body.hash.clone(),
                mime: "text/plain".into(),
                bytes: body.bytes,
            },
            vec![AssetRef {
                hash: image.hash.clone(),
                mime: "image/png".into(),
                bytes: image.bytes,
                role: AssetRole::ImageOriginal,
            }],
            None,
        )
        .await?;
    let author_ticket = ticket(&author.node);

    let relay = Relay::new(Arc::new(NoopHintTransport)).await?;
    relay.app.import_peer_ticket(&author_ticket).await?;
    assert!(
        eventually(|| async {
            let page = relay.app.list_timeline(topic.as_str(), None, 20).await?;
            Ok(content_of(&page, &post).as_deref() == Some("relayed body"))
        })
        .await?
    );
    assert!(
        relay
            .app
            .blob_media_payload_for_post(image.hash.as_str(), "image/png", Some(post.id.as_str()))
            .await?
            .is_some()
    );
    author.go_offline().await?;

    let reader = Relay::new(Arc::new(ScopedReadHints(seed(&relay.node)))).await?;
    reader.app.import_peer_ticket(&author_ticket).await?;
    assert!(
        eventually(|| async {
            let page = reader.app.list_timeline(topic.as_str(), None, 20).await?;
            Ok(content_of(&page, &post).as_deref() == Some("relayed body"))
        })
        .await?,
        "the body must be fetched from the peer that listed the post"
    );
    let payload = reader
        .app
        .blob_media_payload_for_post(image.hash.as_str(), "image/png", Some(post.id.as_str()))
        .await?
        .expect("the image must be fetched from the peer that listed the post");
    assert_eq!(payload.mime, "image/png");

    reader.finish().await?;
    relay.finish().await
}

fn display_name_of(page: &TimelineView, post: &KukuriEnvelope) -> Option<String> {
    page.items
        .iter()
        .find(|item| item.object_id == post.id.as_str())
        .and_then(|item| item.author_display_name.clone())
}

async fn shows_author(app: &AppService, topic: &TopicId, post: &KukuriEnvelope) -> Result<bool> {
    let page = app.list_timeline(topic.as_str(), None, 20).await?;
    Ok(display_name_of(&page, post).as_deref() == Some("Relayed Author"))
}

/// #1419 AC-1: A の投稿と profile を表示した B は、A がオフラインでも、後から来た C へ A の表示名とアバターを返す。
/// C の全体の台帳にはオフラインの A だけがあり、B は同じ topic の参加者としてだけ分かる。
#[tokio::test]
async fn later_reader_gets_an_offline_authors_profile_and_avatar_from_a_topic_peer() -> Result<()> {
    let topic = TopicId::new("relay-offline-profile");
    let author = Author::new().await?;
    let avatar = author
        .blobs
        .put_blob(b"relayed avatar bytes".to_vec(), "image/png")
        .await?;
    author
        .set_profile(
            "Relayed Author",
            AssetRef {
                hash: avatar.hash.clone(),
                mime: "image/png".into(),
                bytes: avatar.bytes,
                role: AssetRole::ProfileAvatar,
            },
        )
        .await?;
    let post = author.post(&topic, "post with a profile", None).await?;
    let author_ticket = ticket(&author.node);

    let relay = Relay::new(Arc::new(NoopHintTransport)).await?;
    relay.app.import_peer_ticket(&author_ticket).await?;
    assert!(eventually(|| shows_author(&relay.app, &topic, &post)).await?);
    assert!(
        relay
            .app
            .blob_media_payload(avatar.hash.as_str(), "image/png")
            .await?
            .is_some()
    );
    author.go_offline().await?;

    let reader = Relay::new(Arc::new(ScopedReadHints(seed(&relay.node)))).await?;
    reader.app.import_peer_ticket(&author_ticket).await?;
    assert!(
        eventually(|| shows_author(&reader.app, &topic, &post)).await?,
        "the display name must come from the topic peer"
    );
    let payload = reader
        .app
        .blob_media_payload(avatar.hash.as_str(), "image/png")
        .await?
        .expect("the avatar must be fetched from the peer that returned the profile");
    assert_eq!(
        BASE64_STANDARD.decode(payload.bytes_base64)?,
        b"relayed avatar bytes"
    );

    reader.finish().await?;
    relay.finish().await
}

/// #1419 INVAR-4: 読んだ profile を中継のために保持した C も、A がオンラインなら A が更新した profile を反映する
/// (保持した旧い版で提供者の新しい版を隠さない)。
#[tokio::test]
async fn a_relayed_profile_copy_does_not_hide_the_authors_update() -> Result<()> {
    let author = Author::new().await?;
    let avatar = author
        .blobs
        .put_blob(b"updated avatar bytes".to_vec(), "image/png")
        .await?;
    let avatar = AssetRef {
        hash: avatar.hash.clone(),
        mime: "image/png".into(),
        bytes: avatar.bytes,
        role: AssetRole::ProfileAvatar,
    };
    author.set_profile("Before Update", avatar.clone()).await?;
    let reader = Relay::new(Arc::new(NoopHintTransport)).await?;
    reader.app.import_peer_ticket(&ticket(&author.node)).await?;
    let author_pubkey = author.keys.public_key_hex();
    let local = reader.app.current_author_pubkey();
    let display_name = || async {
        crate::service::author_state_support::hydrate_author_profile(
            &reader.app.services,
            &local,
            &author_pubkey,
            None,
        )
        .await?;
        Ok::<_, anyhow::Error>(
            reader
                .app
                .services
                .store
                .get_profile(&author_pubkey)
                .await?
                .and_then(|profile| profile.display_name),
        )
    };
    assert_eq!(display_name().await?.as_deref(), Some("Before Update"));

    author.set_profile("After Update", avatar).await?;
    assert_eq!(
        display_name().await?.as_deref(),
        Some("After Update"),
        "the reader must ask the author instead of stopping at its own relayed copy"
    );

    reader.finish().await?;
    author.go_offline().await
}

fn withdrawn(page: &TimelineView, post: &KukuriEnvelope) -> bool {
    page.items
        .iter()
        .any(|item| item.object_id == post.id.as_str() && item.withdrawal.is_some())
}

/// AC-5: A の取り下げを反映した B は、A がオフラインでも、既に投稿を持つ C へ取り下げの記録を返す。
#[tokio::test]
async fn reader_learns_an_offline_authors_withdrawal_from_a_topic_peer() -> Result<()> {
    let topic = TopicId::new("relay-offline-withdrawal");
    let author = Author::new().await?;
    let post = author.post(&topic, "to be withdrawn", None).await?;
    let author_ticket = ticket(&author.node);

    let relay = Relay::new(Arc::new(NoopHintTransport)).await?;
    relay.docs.import_peer_ticket(&author_ticket).await?;
    assert!(has(
        &relay.app.list_timeline(topic.as_str(), None, 20).await?,
        &post
    ));
    let reader = Relay::new(Arc::new(ScopedReadHints(seed(&relay.node)))).await?;
    reader.docs.import_peer_ticket(&author_ticket).await?;
    let page = reader.app.list_timeline(topic.as_str(), None, 20).await?;
    assert!(has(&page, &post) && !withdrawn(&page, &post));

    author.withdraw(&post).await?;
    assert!(
        eventually(|| async {
            Ok(withdrawn(
                &relay.app.list_timeline(topic.as_str(), None, 20).await?,
                &post,
            ))
        })
        .await?
    );
    author.go_offline().await?;

    assert!(
        eventually(|| async {
            Ok(withdrawn(
                &reader.app.list_timeline(topic.as_str(), None, 20).await?,
                &post,
            ))
        })
        .await?,
        "the withdrawal must reach the reader from the topic peer"
    );

    reader.finish().await?;
    relay.finish().await
}

fn reacted(page: &TimelineView, post: &KukuriEnvelope) -> bool {
    page.items.iter().any(|item| {
        item.object_id == post.id.as_str()
            && item
                .reaction_summary
                .iter()
                .any(|reaction| reaction.emoji.as_deref() == Some("👍"))
    })
}

/// AC-6: B が保持する reaction は、操作者がオフラインでも、後から来た C が投稿を B から取ったときに B から反映される。
#[tokio::test]
async fn later_reader_gets_an_offline_authors_reaction_from_a_topic_peer() -> Result<()> {
    let topic = TopicId::new("relay-offline-reaction");
    let author = Author::new().await?;
    let post = author.post(&topic, "reacted", None).await?;
    author.react(&post).await?;
    let author_ticket = ticket(&author.node);

    let relay = Relay::new(Arc::new(NoopHintTransport)).await?;
    relay.docs.import_peer_ticket(&author_ticket).await?;
    assert!(reacted(
        &relay.app.list_timeline(topic.as_str(), None, 20).await?,
        &post
    ));
    author.go_offline().await?;

    let reader = Relay::new(Arc::new(ScopedReadHints(seed(&relay.node)))).await?;
    reader.docs.import_peer_ticket(&author_ticket).await?;
    assert!(
        reacted(
            &reader.app.list_timeline(topic.as_str(), None, 20).await?,
            &post
        ),
        "the reaction must reach the later reader from the topic peer"
    );

    reader.finish().await?;
    relay.finish().await
}

/// 一覧に live session と game room があるか。
async fn sessions_listed(
    app: &AppService,
    topic: &TopicId,
    session_id: &str,
    room_id: &str,
) -> Result<(bool, bool)> {
    let live = app.list_live_sessions(topic.as_str()).await?;
    let games = app.list_game_rooms(topic.as_str()).await?;
    Ok((
        live.iter().any(|session| session.session_id == session_id),
        games.iter().any(|room| room.room_id == room_id),
    ))
}

/// AC-7: B が保持する live session と game room は、owner がオフラインでも、後から来た C の読み直しで B から
/// (state・署名つき manifest・manifest の blob ごと)反映される。
#[tokio::test]
async fn later_reader_gets_an_offline_owners_sessions_from_a_topic_peer() -> Result<()> {
    let topic = TopicId::new("relay-offline-sessions");
    let author = Author::new().await?;
    let store = Arc::new(MemoryStore::default());
    let owner = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        Arc::new(author.docs.clone()),
        Arc::new(author.blobs.clone()),
        author.keys.clone(),
    );
    let session_id = owner
        .create_live_session(
            topic.as_str(),
            CreateLiveSessionInput {
                title: "relayed live".into(),
                description: String::new(),
            },
        )
        .await?;
    let room_id = owner
        .create_game_room(
            topic.as_str(),
            CreateGameRoomInput {
                title: "relayed game".into(),
                description: String::new(),
                participants: vec!["a".into(), "b".into()],
            },
        )
        .await?;
    owner.shutdown().await;
    let author_ticket = ticket(&author.node);

    let relay = Relay::new(Arc::new(NoopHintTransport)).await?;
    relay.app.import_peer_ticket(&author_ticket).await?;
    relay
        .app
        .reread_scope(topic.as_str(), &TimelineScope::Public)
        .await;
    let listed = |app| sessions_listed(app, &topic, &session_id, &room_id);
    assert_eq!(listed(&relay.app).await?, (true, true));
    author.go_offline().await?;

    let reader = Relay::new(Arc::new(ScopedReadHints(seed(&relay.node)))).await?;
    reader.app.import_peer_ticket(&author_ticket).await?;
    reader
        .app
        .reread_scope(topic.as_str(), &TimelineScope::Public)
        .await;
    assert_eq!(
        listed(&reader.app).await?,
        (true, true),
        "the sessions must reach the later reader from the topic peer"
    );

    reader.finish().await?;
    relay.finish().await
}

async fn send_chat(app: &AppService, topic: &TopicId, room_id: &str, seq: u64) -> Result<String> {
    let message_id = format!("chat-{seq}");
    app.publish_metaverse_room_event(
        topic.as_str(),
        PublishMetaverseRoomEventInput {
            room_id: room_id.to_string(),
            peer_id: "visitor".into(),
            seq,
            event: kukuri_core::MetaverseRoomEventV1::ChatMessage {
                message: kukuri_core::MetaverseRoomChatMessageV1 {
                    room_id: room_id.to_string(),
                    message_id: message_id.clone(),
                    author_peer_id: "visitor".into(),
                    display_name: None,
                    body: message_id.clone(),
                    created_at: Utc::now().timestamp_millis(),
                },
            },
        },
    )
    .await?;
    Ok(message_id)
}

/// AC-7 の Regression の固定: 他の参加者へ提供するための保持を、自分の書込みと取り違えない。他人の Dome の訪問者が
/// chat を続けて送っても、前の chat は履歴に残る(訪問者が書いた state は手元が正本)。
#[tokio::test]
async fn a_visitor_keeps_its_own_dome_chat_history() -> Result<()> {
    let topic = TopicId::new("relay-visitor-chat");
    let author = Author::new().await?;
    let store = Arc::new(MemoryStore::default());
    let owner = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        Arc::new(author.docs.clone()),
        Arc::new(author.blobs.clone()),
        author.keys.clone(),
    );
    let room_id = owner
        .create_metaverse_room(
            topic.as_str(),
            CreateMetaverseRoomInput {
                title: "dome".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await?;
    let visitor = Relay::new(Arc::new(ScopedReadHints(seed(&author.node)))).await?;
    visitor
        .app
        .import_peer_ticket(&ticket(&author.node))
        .await?;
    visitor
        .app
        .read_session(
            topic.as_str(),
            &TimelineScope::Public,
            &room_id,
            "game-session",
        )
        .await?;
    let first = send_chat(&visitor.app, &topic, &room_id, 1).await?;
    let second = send_chat(&visitor.app, &topic, &room_id, 2).await?;
    let history = visitor
        .app
        .services
        .projection_store
        .get_game_room(topic.as_str(), &room_id)
        .await?
        .and_then(|room| room.metaverse)
        .map(|metaverse| {
            metaverse
                .chat_history
                .into_iter()
                .map(|message| message.message_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(history, vec![first, second]);

    owner.shutdown().await;
    visitor.finish().await?;
    author.go_offline().await
}

/// 同じ topic の neighbor を途中で入れ替えられる hint(neighbor は時間とともに変わる)。
struct ChangingNeighbors(std::sync::Mutex<Vec<SeedPeer>>);

#[async_trait]
impl HintTransport for ChangingNeighbors {
    async fn subscribe_hints(&self, _: &TopicId) -> Result<kukuri_transport::HintStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
    async fn unsubscribe_hints(&self, _: &TopicId) -> Result<()> {
        Ok(())
    }
    async fn publish_hint(&self, _: &TopicId, _: GossipHint) -> Result<()> {
        Ok(())
    }
    async fn topic_read_candidates(&self, _: &TopicId) -> Result<Vec<SeedPeer>> {
        Ok(self.0.lock().expect("neighbors").clone())
    }
}

/// #1419 AC-2: C の取得元の記録は、見出しを返した D(画像を表示していないので画像を持たない)だけを指す。
/// 画像を表示して持つ B が後から同じ topic の neighbor になれば、C は画像を B から取る。
#[tokio::test]
async fn reader_fetches_an_image_from_a_topic_neighbor_that_holds_it() -> Result<()> {
    let topic = TopicId::new("relay-image-neighbor");
    let author = Author::new().await?;
    let image = author
        .blobs
        .put_blob(b"neighbor image bytes".to_vec(), "image/png")
        .await?;
    let post = author
        .post_with(
            &topic,
            PayloadRef::InlineText {
                text: "image post".into(),
            },
            vec![AssetRef {
                hash: image.hash.clone(),
                mime: "image/png".into(),
                bytes: image.bytes,
                role: AssetRole::ImageOriginal,
            }],
            None,
        )
        .await?;
    let author_ticket = ticket(&author.node);

    let holder = Relay::new(Arc::new(NoopHintTransport)).await?;
    holder.app.import_peer_ticket(&author_ticket).await?;
    assert!(has(
        &holder.app.list_timeline(topic.as_str(), None, 20).await?,
        &post
    ));
    assert!(
        holder
            .app
            .blob_media_payload_for_post(image.hash.as_str(), "image/png", Some(post.id.as_str()))
            .await?
            .is_some()
    );
    let lister = Relay::new(Arc::new(NoopHintTransport)).await?;
    lister.app.import_peer_ticket(&author_ticket).await?;
    assert!(has(
        &lister.app.list_timeline(topic.as_str(), None, 20).await?,
        &post
    ));
    author.go_offline().await?;

    let neighbors = Arc::new(ChangingNeighbors(std::sync::Mutex::new(vec![seed(
        &lister.node,
    )])));
    let reader = Relay::new(neighbors.clone()).await?;
    reader.app.import_peer_ticket(&author_ticket).await?;
    assert!(has(
        &reader.app.list_timeline(topic.as_str(), None, 20).await?,
        &post
    ));
    *neighbors.0.lock().expect("neighbors") = vec![seed(&lister.node), seed(&holder.node)];
    // 表示中の timeline の読み直しで、新しい neighbor とも接続する(本番では gossip で接続済み)。
    assert!(has(
        &reader.app.list_timeline(topic.as_str(), None, 20).await?,
        &post
    ));
    let payload = reader
        .app
        .blob_media_payload_for_post(image.hash.as_str(), "image/png", Some(post.id.as_str()))
        .await?
        .expect("the image must be fetched from the topic neighbor that holds it");
    assert_eq!(
        BASE64_STANDARD.decode(payload.bytes_base64)?,
        b"neighbor image bytes"
    );

    reader.finish().await?;
    lister.finish().await?;
    holder.finish().await
}

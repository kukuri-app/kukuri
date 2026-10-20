//! #1840 AC-1: 公開 channel(公開スレッド)を既存の channel の基盤に足す。招待の無い端末が ID から metadata と最初の投稿を
//! 読み、同じ channel へ投稿・返信・リアクションし、作成者には同じ channel の文脈で届く。topic の公開タイムラインと別の
//! 公開 channel には出ない。
use super::sync::CountingDocsSync;
use super::*;
use crate::PublicChannelView;
use kukuri_core::{
    KukuriPostEnvelopeContentV1, KukuriPublicChannelContentV1, build_public_channel_envelope,
    sign_envelope_json_at,
};
use kukuri_docs_sync::{BucketReplica, BucketScope, MemoryDocsSync, TimeBucket};
use kukuri_store::PublicChannelOrder;

pub(super) const TOPIC: &str = "kukuri:topic:public-threads";

/// 自分の docs に書き、相手の docs を provider として読む端末。hint は同じ試験用の network で届き、blob は共有する。
pub(super) fn device(
    name: &str,
    network: &FakeNetwork,
    own: MemoryDocsSync,
    peer: MemoryDocsSync,
    blobs: Arc<MemoryBlobService>,
) -> AppService {
    device_with_store(name, network, own, peer, blobs).0
}

/// `device` と、その端末の手元の store。
pub(super) fn device_with_store(
    name: &str,
    network: &FakeNetwork,
    own: MemoryDocsSync,
    peer: MemoryDocsSync,
    blobs: Arc<MemoryBlobService>,
) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new(name, network.clone()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        Arc::new(CountingDocsSync::reading_with(own, Arc::new(peer))),
        blobs,
        generate_keys(),
    );
    app.switch_writer(1);
    (app, store)
}

/// 公開 channel の詳細の列を開く(topic と channel の lease を取り、channel の hint を受ける)。
async fn open_channel(app: &AppService, channel_id: &str) {
    app.set_scope_display(crate::ScopeDisplayRequest {
        observer: format!("test-channel:{channel_id}"),
        target: crate::ScopeDisplayTarget::Timeline {
            topic: TOPIC.into(),
            scope: channel_ref(channel_id).timeline_scope(),
        },
        visible: true,
    })
    .await
    .expect("open the public channel column");
}

pub(super) fn channel_ref(channel_id: &str) -> ChannelRef {
    ChannelRef::PublicChannel {
        channel_id: ChannelId::new(channel_id),
    }
}

/// 投稿の表示が、この公開 channel の文脈を持つか(channel の ID と、公開スレッドの公開範囲)。
fn in_channel(post: &PostView, channel_id: &str) -> bool {
    post.channel_id.as_deref() == Some(channel_id) && post.audience_label == "Public thread"
}

async fn channel_posts(app: &AppService, channel_id: &str) -> Vec<PostView> {
    app.list_timeline_scoped(TOPIC, channel_ref(channel_id).timeline_scope(), None, 50)
        .await
        .expect("channel timeline")
        .items
}

#[tokio::test]
async fn a_device_without_an_invite_reads_a_public_channel_by_id_and_writes_into_it() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("02".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);

    let channel = a
        .create_public_channel(TOPIC, " 雑談 ", "最初の投稿", Vec::new())
        .await
        .expect("create the public channel");
    assert!(ChannelId::new(channel.channel_id.as_str()).is_public_channel());
    assert_eq!(channel.title, "雑談");
    assert_eq!(channel.creator_pubkey, a.current_author_pubkey());
    let first_post_id = channel.first_post_id.clone().expect("the first post");
    let public_channel = channel_ref(&channel.channel_id);
    open_channel(&a, &channel.channel_id).await;

    // B は招待なしに ID から metadata を読み、metadata の最初の投稿を ID から辿る。
    assert_eq!(
        b.get_public_channel(TOPIC, &channel.channel_id)
            .await
            .expect("get the public channel"),
        Some(channel.clone())
    );
    let thread = b
        .list_thread(TOPIC, &first_post_id, None, 50)
        .await
        .expect("the first post on B")
        .items;
    let first = thread
        .iter()
        .find(|post| post.object_id == first_post_id)
        .expect("the first post resolves by id");
    assert_eq!(first.content, "最初の投稿");
    assert!(in_channel(first, &channel.channel_id), "{first:?}");

    // B の投稿(添付つき)・返信・emoji と custom のリアクション。
    let b_post = b
        .create_post_with_attachments_in_channel(
            TOPIC,
            public_channel.clone(),
            "Bの投稿",
            None,
            vec![pending_image_attachment("image/png", &tiny_png_bytes())],
            Vec::new(),
            None,
        )
        .await
        .expect("B posts into the channel");
    let reply = b
        .create_post_with_attachments_in_channel(
            TOPIC,
            public_channel.clone(),
            "返信",
            Some(first_post_id.as_str()),
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect("B replies to the first post");
    b.toggle_reaction(
        TOPIC,
        &first_post_id,
        ReactionKeyV1::Emoji {
            emoji: "👍".into()
        },
        Some(public_channel.clone()),
    )
    .await
    .expect("emoji reaction");
    let asset = b
        .create_custom_reaction_asset(CreateCustomReactionAssetInput {
            search_key: "party".into(),
            mime: "image/png".into(),
            bytes: tiny_png_bytes(),
            width: 128,
            height: 128,
        })
        .await
        .expect("custom reaction asset");
    b.toggle_reaction(
        TOPIC,
        &first_post_id,
        ReactionKeyV1::CustomAsset {
            asset_id: asset.asset_id.clone(),
            snapshot: reaction_snapshot_from_view(&asset),
        },
        Some(public_channel.clone()),
    )
    .await
    .expect("custom reaction");

    // A には同じ channel の文脈で届く(リアクションは channel の hint で届く)。
    let reacted = |posts: &[PostView]| {
        posts
            .iter()
            .any(|post| post.object_id == first_post_id && post.reaction_summary.len() == 2)
    };
    let mut posts = channel_posts(&a, &channel.channel_id).await;
    for _ in 0..50 {
        if reacted(&posts) {
            break;
        }
        sleep(Duration::from_millis(100)).await;
        posts = channel_posts(&a, &channel.channel_id).await;
    }
    let b_view = posts
        .iter()
        .find(|post| post.object_id == b_post)
        .expect("B's post on A");
    assert!(in_channel(b_view, &channel.channel_id), "{b_view:?}");
    assert_eq!(b_view.attachments.len(), 1);
    let first_on_a = posts
        .iter()
        .find(|post| post.object_id == first_post_id)
        .expect("the first post on A");
    assert!(first_on_a.reaction_summary.iter().any(|entry| {
        entry.reaction_key_kind == "emoji"
            && entry.emoji.as_deref() == Some("👍")
            && entry.count == 1
    }));
    assert!(first_on_a.reaction_summary.iter().any(|entry| {
        entry.reaction_key_kind == "custom_asset"
            && entry
                .custom_asset
                .as_ref()
                .map(|asset| asset.asset_id.as_str())
                == Some(asset.asset_id.as_str())
    }));
    let replies = a
        .list_thread(TOPIC, &first_post_id, None, 50)
        .await
        .expect("the thread on A")
        .items;
    assert!(
        replies
            .iter()
            .any(|post| post.object_id == reply && in_channel(post, &channel.channel_id))
    );

    // 公開 channel の投稿は種類を合わせて bookmark でき、作成者のプロフィールには出ない。private の形の ID は受け付けない。
    b.bookmark_post_in_channel(TOPIC, &first_post_id, public_channel.clone())
        .await
        .expect("bookmark in the public channel");
    assert!(
        b.bookmark_post_in_channel(TOPIC, &b_post, ChannelRef::Public)
            .await
            .is_err()
    );
    assert!(
        a.list_profile_timeline(&a.current_author_pubkey(), None, 50)
            .await
            .expect("profile")
            .items
            .is_empty()
    );
    assert!(
        b.create_post_in_channel(
            TOPIC,
            ChannelRef::PublicChannel {
                channel_id: ChannelId::new("channel-1-abcdef01"),
            },
            "private form",
            None,
        )
        .await
        .is_err()
    );

    // topic の公開タイムラインと、別の公開 channel には出ない。
    let other = a
        .create_public_channel(TOPIC, "別のスレッド", "", Vec::new())
        .await
        .expect("another public channel");
    assert_eq!(other.first_post_id, None);
    for app in [&a, &b] {
        let public = app
            .list_timeline(TOPIC, None, 50)
            .await
            .expect("topic timeline")
            .items;
        assert!(public.is_empty(), "{public:?}");
        assert!(channel_posts(app, &other.channel_id).await.is_empty());
    }

    // 取り下げも対象の channel の種類を求める。
    assert!(
        b.withdraw_post(
            TOPIC,
            &b_post,
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )
        .await
        .is_err()
    );
    b.withdraw_post(
        TOPIC,
        &b_post,
        public_channel,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .await
    .expect("withdraw in the public channel");
}

// #1840 AC-3: 行に作成者の profile の名前が付く。手元に無い profile は取得が背景で読み、読めたら次の取得で付く。
#[tokio::test]
async fn public_channel_rows_carry_the_creator_profile_names() {
    let a_docs = MemoryDocsSync::with_docs_author("05".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("06".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);
    a.set_my_profile(ProfileInput {
        name: Some("alice".into()),
        display_name: Some("Alice".into()),
        ..Default::default()
    })
    .await
    .expect("set the creator profile");
    let names =
        |view: &PublicChannelView| (view.creator_name.clone(), view.creator_display_name.clone());
    let expected = (Some("alice".to_string()), Some("Alice".to_string()));

    let channel = a
        .create_public_channel(TOPIC, "名前つき", "", Vec::new())
        .await
        .expect("create");
    assert_eq!(names(&channel), expected);
    let listed = a
        .list_public_channels(TOPIC, PublicChannelOrder::Created, None, 20)
        .await
        .expect("list on A");
    assert_eq!(
        listed.items.iter().map(names).collect::<Vec<_>>(),
        vec![expected.clone()]
    );

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let read = b
            .get_public_channel(TOPIC, &channel.channel_id)
            .await
            .expect("get on B")
            .expect("the channel by id");
        if names(&read) == expected {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the creator profile never arrived: {read:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

// 作成から日を跨いだ公開 channel でも、ID から作成時の最初の投稿を辿れる(作成日の bucket から読む)。
#[tokio::test]
async fn the_first_post_of_an_older_public_channel_resolves_by_id() {
    let a_docs = MemoryDocsSync::with_docs_author("03".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("04".repeat(32));
    let b = device(
        "b",
        &FakeNetwork::default(),
        b_docs,
        a_docs.clone(),
        Arc::new(MemoryBlobService::default()),
    );
    let creator = generate_keys();
    let created_ms = Utc::now().timestamp_millis() - 3 * 86_400_000;
    let channel_id = ChannelId::public_channel(created_ms, &creator.public_key_hex());
    let topic = TopicId::new(TOPIC);
    let first = sign_envelope_json_at(
        &creator,
        "post",
        vec![
            vec!["topic".into(), TOPIC.into()],
            vec!["object".into(), "post".into()],
            vec!["channel".into(), channel_id.as_str().into()],
        ],
        &KukuriPostEnvelopeContentV1 {
            object_kind: "post".into(),
            topic_id: topic.clone(),
            channel_id: Some(channel_id.clone()),
            payload_ref: PayloadRef::InlineText {
                text: "3 日前の最初の投稿".into(),
            },
            attachments: Vec::new(),
            media_manifest_refs: Vec::new(),
            visibility: ObjectVisibility::Public,
            reply_to: None,
            root_id: None,
            repost_of: None,
            content_labels: Vec::new(),
            live: None,
        },
        created_ms / 1_000,
    )
    .expect("first post");
    let bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: TOPIC.into(),
        },
        TimeBucket::from_unix_seconds(created_ms / 1_000).expect("bucket"),
    )
    .expect("replica")
    .replica_id();
    persist_post_object(
        &a_docs,
        &bucket,
        first.to_post_object().expect("parse").expect("post"),
        first.clone(),
    )
    .await
    .expect("persist the first post");
    let metadata = build_public_channel_envelope(
        &creator,
        &KukuriPublicChannelContentV1 {
            channel_id: channel_id.clone(),
            topic_id: topic,
            title: "古いスレッド".into(),
            first_post_id: Some(first.id.clone()),
        },
    )
    .expect("metadata");
    a_docs
        .apply_doc_op(
            &bucket,
            DocOp::SetJson {
                key: crate::public_channels::public_channel_location(TOPIC, &channel_id)
                    .expect("location")
                    .1,
                value: serde_json::to_value(&metadata).expect("json"),
            },
        )
        .await
        .expect("persist the metadata");

    let channel = b
        .get_public_channel(TOPIC, channel_id.as_str())
        .await
        .expect("get the public channel")
        .expect("found");
    assert_eq!(channel.first_post_id.as_deref(), Some(first.id.as_str()));
    let thread = b
        .list_thread(TOPIC, first.id.as_str(), None, 50)
        .await
        .expect("the thread")
        .items;
    assert!(
        thread
            .iter()
            .any(|post| post.object_id == first.id.as_str() && post.content == "3 日前の最初の投稿"),
        "{thread:?}"
    );
}

// private channel は公開 channel の ID の形を使えない(ID の形で見分けるため)。公開 channel は、ゲーム・Dome を受け付けない
// (Issue の対象外。Live は AC-4 で受け付ける。`public_channel_live.rs`)。
#[tokio::test]
async fn private_channels_cannot_take_the_public_channel_form_and_games_stay_out() {
    let (app, _, _, _) = local_app_with_memory_services();
    app.switch_writer(1);
    let owner = generate_keys();
    let channel = ChannelId::public_channel(1_760_000_000_123, &owner.public_key_hex());
    let token = build_private_channel_invite_token(
        &owner,
        PrivateChannelInviteTokenParams {
            topic: &TopicId::new(TOPIC),
            channel_id: &channel,
            channel_label: "label",
            owner_pubkey: &owner.public_key(),
            epoch_id: "epoch-1-abcdef01",
            namespace_secret_hex: &generate_keys().export_secret_hex(),
            expires_at: None,
        },
    )
    .expect("invite token");
    let error = app
        .import_private_channel_invite(&token)
        .await
        .expect_err("a private channel with the public channel form");
    assert!(
        error.to_string().contains("public channel form"),
        "{error:#}"
    );
    let public_channel = ChannelRef::PublicChannel {
        channel_id: channel,
    };
    assert!(
        app.create_game_room_in_channel(
            TOPIC,
            public_channel,
            CreateGameRoomInput {
                title: "game".into(),
                description: String::new(),
                participants: vec!["a".into(), "b".into()],
            },
        )
        .await
        .is_err()
    );
}

/// 読み先として 1 台だけを返す hint の transport(hint は運ばない)。
#[cfg(feature = "iroh-integration-tests")]
struct ReadFrom(SeedPeer);

#[cfg(feature = "iroh-integration-tests")]
#[async_trait]
impl HintTransport for ReadFrom {
    async fn subscribe_hints(&self, _: &TopicId) -> Result<HintStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
    async fn unsubscribe_hints(&self, _: &TopicId) -> Result<()> {
        Ok(())
    }
    async fn publish_hint(&self, _: &TopicId, _: GossipHint) -> Result<()> {
        Ok(())
    }
    async fn topic_read_candidates(&self, _: &TopicId) -> Result<Vec<SeedPeer>> {
        Ok(vec![self.0.clone()])
    }
}

// 実際の iroh の端末どうしで、招待の無い端末が ID から metadata と channel の投稿を読む(QUIC の docs の読取り。手元の
// namespace は作らない)。
#[cfg(feature = "iroh-integration-tests")]
#[tokio::test]
async fn real_iroh_device_without_an_invite_reads_a_public_channel_by_id() -> Result<()> {
    let publisher_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let client_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let publisher_docs = Arc::new(IrohDocsSync::new(publisher_node.clone()));
    let client_docs = Arc::new(IrohDocsSync::new(client_node.clone()));
    let socket = publisher_node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    let publisher_id = publisher_node.endpoint().addr().id.to_string();
    client_docs
        .import_peer_ticket(&format!("{publisher_id}@{socket}"))
        .await?;
    let blobs = Arc::new(MemoryBlobService::default());
    let app = |docs: Arc<IrohDocsSync>, hints: Arc<dyn HintTransport>| {
        let store = Arc::new(MemoryStore::default());
        let app = app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            hints,
            docs,
            blobs.clone(),
            generate_keys(),
        );
        app.switch_writer(1);
        app
    };
    let a = app(publisher_docs.clone(), Arc::new(NoopHintTransport));
    let b = app(
        client_docs.clone(),
        Arc::new(ReadFrom(SeedPeer {
            endpoint_id: publisher_id,
            addr_hint: Some(socket.to_string()),
        })),
    );

    let channel = a
        .create_public_channel(TOPIC, "雑談", "最初の投稿", Vec::new())
        .await?;
    assert_eq!(
        b.get_public_channel(TOPIC, &channel.channel_id).await?,
        Some(channel.clone())
    );
    let first_post_id = channel.first_post_id.clone().expect("the first post");
    let posts = channel_posts(&b, &channel.channel_id).await;
    let first = posts
        .iter()
        .find(|post| post.object_id == first_post_id)
        .expect("the first post on B");
    assert_eq!(first.content, "最初の投稿");
    assert!(in_channel(first, &channel.channel_id), "{first:?}");
    assert!(b.list_timeline(TOPIC, None, 50).await?.items.is_empty());
    assert_eq!(client_node.docs().list().await?.count().await, 0);

    a.shutdown().await;
    b.shutdown().await;
    client_docs.shutdown().await;
    publisher_docs.shutdown().await;
    client_node.shutdown().await?;
    publisher_node.shutdown().await
}

// #1889: 作成時の最初の投稿に添付を付けられる。本文が空でも添付があれば最初の投稿を作り、ほかの端末が ID から読める。
#[tokio::test]
async fn a_public_channel_is_created_with_an_attachment_only_first_post() {
    let a_docs = MemoryDocsSync::with_docs_author("07".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("08".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);

    let channel = a
        .create_public_channel(
            TOPIC,
            "画像だけ",
            "",
            vec![pending_image_attachment("image/png", &tiny_png_bytes())],
        )
        .await
        .expect("create with an attachment");
    let first_post_id = channel.first_post_id.clone().expect("the first post");
    let thread = b
        .list_thread(TOPIC, &first_post_id, None, 50)
        .await
        .expect("the first post on B")
        .items;
    let first = thread
        .iter()
        .find(|post| post.object_id == first_post_id)
        .expect("the first post resolves by id");
    assert!(in_channel(first, &channel.channel_id), "{first:?}");
    assert_eq!(first.attachments.len(), 1, "{first:?}");
    assert_eq!(first.attachments[0].mime, "image/png");
}

//! 投稿の反映(hydration)が、docs の record の申告値をそのまま信用しないことを固定する。
//!
//! public topic の replica の namespace secret は replica id から決まり(`public_replica_secret`)、
//! topic id を知る誰もが書ける。`objects/<object id>/state` の header は署名を持たないので、
//! 反映は同じ object の署名つき envelope と、record を読んだ replica に照らして確かめる必要がある。

use super::*;

/// docs event で投稿が反映される viewer と、その topic の public replica。
pub(super) struct IntegrityFixture {
    pub(super) docs_sync: Arc<MemoryDocsSync>,
    pub(super) viewer: AppService,
    pub(super) viewer_store: Arc<MemoryStore>,
    pub(super) topic: TopicId,
    pub(super) replica: ReplicaId,
}

/// viewer が先に topic を購読し、docs event で 1 件が反映されることを確かめてから始める。
pub(super) async fn integrity_fixture(name: &str) -> IntegrityFixture {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let viewer_store = Arc::new(MemoryStore::default());
    let viewer = app_service_from_dependencies(
        viewer_store.clone(),
        viewer_store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new(format!("kukuri:topic:integrity-{name}").as_str());
    let replica = topic_replica_id(topic.as_str());
    viewer
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("empty timeline");
    sleep(Duration::from_millis(150)).await;
    let first = persist_test_post(
        docs_sync.as_ref(),
        None,
        &generate_keys(),
        &topic,
        PayloadRef::InlineText {
            text: "first post".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    assert!(
        wait_for_row(&viewer_store, &first.id).await,
        "the first post must be projected by the docs event"
    );
    IntegrityFixture {
        docs_sync,
        viewer,
        viewer_store,
        topic,
        replica,
    }
}

pub(super) async fn wait_for_row(store: &Arc<MemoryStore>, object_id: &EnvelopeId) -> bool {
    timeout(Duration::from_secs(3), async {
        loop {
            if ObjectProjectionStore::get_object_projection(store.as_ref(), object_id)
                .await
                .expect("projection")
                .is_some()
            {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .is_ok()
}

pub(super) fn signed_post(
    keys: &KukuriKeys,
    topic: &TopicId,
    text: &str,
    visibility: ObjectVisibility,
    channel_id: Option<&ChannelId>,
) -> KukuriEnvelope {
    build_post_envelope_with_payload_in_channel(
        keys,
        topic,
        PayloadRef::InlineText { text: text.into() },
        Vec::new(),
        Vec::new(),
        None,
        visibility,
        channel_id,
        Vec::new(),
    )
    .expect("post envelope")
}

/// remote の同期と同じ key の昇順(`objects/<id>/envelope` → `objects/<id>/state`)で entry を書く。
/// `envelope` が `None` なら header だけを書く。
pub(super) async fn write_object_entries(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope: Option<&KukuriEnvelope>,
    header: &CanonicalPostHeader,
) {
    if let Some(envelope) = envelope {
        docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        "objects",
                        &format!("{}/envelope", header.object_id.as_str()),
                    ),
                    value: serde_json::to_value(envelope).expect("envelope json"),
                },
            )
            .await
            .expect("write envelope entry");
    }
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("objects", &format!("{}/state", header.object_id.as_str())),
                value: serde_json::to_value(header).expect("state json"),
            },
        )
        .await
        .expect("write state entry");
}

/// docs event の反映を待ってから、public のタイムラインを返す。
pub(super) async fn public_timeline_after_settle(fixture: &IntegrityFixture) -> TimelineView {
    sleep(Duration::from_millis(400)).await;
    fixture
        .viewer
        .list_timeline(fixture.topic.as_str(), None, 50)
        .await
        .expect("timeline")
}

// (a-1) 攻撃者が自分の鍵で署名した envelope を置き、header の author だけを被害者の pubkey に書き換える。
// 反映は署名つき envelope の author を正とし、header の申告値で著者を決めてはならない。
#[tokio::test]
async fn header_author_that_differs_from_the_signed_envelope_is_not_shown_as_that_author() {
    let fixture = integrity_fixture("forged-author").await;
    let victim_keys = generate_keys();
    let attacker_keys = generate_keys();
    let victim_pubkey = victim_keys.public_key_hex();
    let envelope = signed_post(
        &attacker_keys,
        &fixture.topic,
        "words the victim never wrote",
        ObjectVisibility::Public,
        None,
    );
    let mut header = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    header.author = Pubkey::from(victim_pubkey.as_str());
    write_object_entries(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        Some(&envelope),
        &header,
    )
    .await;

    let view = public_timeline_after_settle(&fixture).await;
    let shown_as_victim = view
        .items
        .iter()
        .filter(|item| item.author_pubkey == victim_pubkey)
        .map(|item| item.content.clone())
        .collect::<Vec<_>>();
    assert!(
        shown_as_victim.is_empty(),
        "a header whose author differs from the signed envelope was shown as the victim's post: {shown_as_victim:?}"
    );
    let row =
        ObjectProjectionStore::get_object_projection(fixture.viewer_store.as_ref(), &envelope.id)
            .await
            .expect("projection");
    assert!(
        row.as_ref()
            .is_none_or(|row| row.author_pubkey != victim_pubkey),
        "the projection row carries the victim's pubkey: {row:?}"
    );
}

// (a-2) envelope を置かず、header だけで被害者の投稿を装う。署名で裏づけられない header は反映しない。
#[tokio::test]
async fn header_without_a_signed_envelope_is_not_projected() {
    let fixture = integrity_fixture("header-only").await;
    let victim_pubkey = generate_keys().public_key_hex();
    // 形を借りるための envelope。replica へは置かない。
    let shape = signed_post(
        &generate_keys(),
        &fixture.topic,
        "header only",
        ObjectVisibility::Public,
        None,
    );
    let mut header = shape
        .to_post_object()
        .expect("post object")
        .expect("post object");
    header.author = Pubkey::from(victim_pubkey.as_str());
    write_object_entries(fixture.docs_sync.as_ref(), &fixture.replica, None, &header).await;

    let view = public_timeline_after_settle(&fixture).await;
    assert!(
        view.items
            .iter()
            .all(|item| item.object_id != header.object_id.as_str()),
        "a header that no signed envelope backs was listed in the timeline"
    );
}

// (a-3) envelope は正しく署名されているが、header の本文が envelope の本文と違う。
// 署名された内容と違う本文を、その著者の投稿として表示してはならない。
#[tokio::test]
async fn header_payload_that_differs_from_the_signed_envelope_is_not_shown() {
    let fixture = integrity_fixture("forged-payload").await;
    let author_keys = generate_keys();
    let envelope = signed_post(
        &author_keys,
        &fixture.topic,
        "what the author signed",
        ObjectVisibility::Public,
        None,
    );
    let mut header = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    header.payload_ref = PayloadRef::InlineText {
        text: "what the author never signed".into(),
    };
    write_object_entries(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        Some(&envelope),
        &header,
    )
    .await;

    let view = public_timeline_after_settle(&fixture).await;
    assert!(
        view.items
            .iter()
            .all(|item| item.content != "what the author never signed"),
        "a body that the author never signed was shown under the author's pubkey"
    );
}

// (b) public replica に置いた投稿が private channel の id を申告する。envelope は攻撃者自身の鍵で正しく署名
// されているので、署名の検証だけでは防げない。private channel のタイムラインは、その channel の epoch の
// replica から読んだ投稿だけを載せなければならない。
#[tokio::test]
async fn post_in_the_public_replica_that_claims_a_private_channel_is_not_listed_in_the_channel() {
    let (app, store, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-channel-claim");
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: topic.clone(),
            label: "members only".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let scope = TimelineScope::Channel {
        channel_id: channel_id.clone(),
    };
    let member_post_id = app
        .create_post_in_channel(
            topic.as_str(),
            ChannelRef::PrivateChannel {
                channel_id: channel_id.clone(),
            },
            "from a member",
            None,
        )
        .await
        .expect("create member post");
    app.list_timeline_scoped(topic.as_str(), scope.clone(), None, 20)
        .await
        .expect("warm the private timeline");
    sleep(Duration::from_millis(150)).await;

    // 攻撃者は channel の参加者ではない。topic id と channel id だけを知っている。
    let attacker_keys = generate_keys();
    let envelope = signed_post(
        &attacker_keys,
        &topic,
        "injected from outside the channel",
        ObjectVisibility::Private,
        Some(&channel_id),
    );
    let header = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    let public_replica = topic_replica_id(topic.as_str());
    write_object_entries(
        docs_sync.as_ref(),
        &public_replica,
        Some(&envelope),
        &header,
    )
    .await;
    sleep(Duration::from_millis(400)).await;
    // docs event を取りこぼした場合に備えて、key 指定の反映も直接通す。
    hydrate_object_in_topic(
        &app.services,
        topic.as_str(),
        &public_replica,
        &envelope.id,
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("hydrate the injected object");

    let view = app
        .list_timeline_scoped(topic.as_str(), scope, None, 20)
        .await
        .expect("private timeline");
    let listed = view
        .items
        .iter()
        .map(|item| item.object_id.clone())
        .collect::<Vec<_>>();
    assert!(
        listed.contains(&member_post_id),
        "the member's post must stay listed: {listed:?}"
    );
    assert!(
        !listed.contains(&envelope.id.as_str().to_string()),
        "a post read from the public replica was listed in the private channel timeline"
    );
    let row = ObjectProjectionStore::get_object_projection(store.as_ref(), &envelope.id)
        .await
        .expect("projection");
    assert!(
        row.as_ref()
            .is_none_or(|row| row.channel_id != channel_id.as_str()),
        "a row whose source is the public replica carries the private channel id: {row:?}"
    );
}

// (c) topic A の replica に置いた投稿が topic B を申告する。読んだ replica の topic と違う投稿は反映しない。
#[tokio::test]
async fn post_that_claims_another_topic_is_not_listed_in_that_topic() {
    let fixture = integrity_fixture("topic-claim").await;
    let other_topic = TopicId::new("kukuri:topic:integrity-topic-claim-other");
    fixture
        .viewer
        .list_timeline(other_topic.as_str(), None, 20)
        .await
        .expect("warm the other topic");
    let envelope = signed_post(
        &generate_keys(),
        &other_topic,
        "written into another topic's replica",
        ObjectVisibility::Public,
        None,
    );
    let header = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    // topic B を申告する投稿を、topic A の replica へ置く。
    write_object_entries(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        Some(&envelope),
        &header,
    )
    .await;
    sleep(Duration::from_millis(400)).await;

    let view = fixture
        .viewer
        .list_timeline(other_topic.as_str(), None, 20)
        .await
        .expect("other timeline");
    assert!(
        view.items
            .iter()
            .all(|item| item.object_id != envelope.id.as_str()),
        "a post read from another topic's replica was listed in the claimed topic"
    );
}

// (d) 通知も header の申告値から作られる。署名と合わない著者の reply で、その著者からの通知を作ってはならない。
#[tokio::test]
async fn forged_reply_header_does_not_create_a_notification_from_the_claimed_author() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-notification");
    let local_object_id = app
        .create_post(topic.as_str(), "local root", None)
        .await
        .expect("create local post");
    let local_envelope = store
        .get_envelope(&EnvelopeId::from(local_object_id.as_str()))
        .await
        .expect("load local envelope")
        .expect("local envelope");
    let victim_pubkey = generate_keys().public_key_hex();
    let attacker_keys = generate_keys();
    let envelope = build_post_envelope_with_payload_in_channel(
        &attacker_keys,
        &topic,
        PayloadRef::InlineText {
            text: "a reply the victim never wrote".into(),
        },
        Vec::new(),
        Vec::new(),
        Some(&local_envelope),
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("reply envelope");
    let mut header = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    header.author = Pubkey::from(victim_pubkey.as_str());
    let replica = topic_replica_id(topic.as_str());
    write_object_entries(docs_sync.as_ref(), &replica, Some(&envelope), &header).await;

    create_remote_object_notification(
        &app,
        store.as_ref(),
        docs_sync.as_ref(),
        blob_service.as_ref(),
        remote_doc_event(
            docs_sync.as_ref(),
            &replica,
            stable_key("objects", &format!("{}/state", envelope.id.as_str())),
        )
        .await,
    )
    .await;

    let from_victim = app
        .list_notifications()
        .await
        .expect("list notifications")
        .into_iter()
        .filter(|item| item.actor_pubkey == victim_pubkey)
        .map(|item| item.preview_text)
        .collect::<Vec<_>>();
    assert!(
        from_victim.is_empty(),
        "a forged reply header created a notification from the victim: {from_victim:?}"
    );
}

// (e) 投稿として読めない record が 1 件あっても、同じ replica の他の投稿の反映と API を止めない
// (ADR 0052 §2 が取り下げについて定めるのと同じ性質)。
#[tokio::test]
async fn unreadable_state_record_does_not_break_the_timeline_of_the_topic() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let topic = TopicId::new("kukuri:topic:integrity-unreadable-state");
    let replica = topic_replica_id(topic.as_str());
    let honest = persist_test_post(
        docs_sync.as_ref(),
        None,
        &generate_keys(),
        &topic,
        PayloadRef::InlineText {
            text: "an honest post next to garbage".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("objects", "0000-garbage/state"),
                value: serde_json::json!({ "not": "a post header" }),
            },
        )
        .await
        .expect("write an unreadable state record");

    // 反映が空の状態から始める viewer(新規の参加者)。
    let store = Arc::new(MemoryStore::default());
    let viewer = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let view = viewer
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("an unreadable record must not fail the timeline of the whole topic");
    assert!(
        view.items
            .iter()
            .any(|item| item.object_id == honest.id.as_str()),
        "an unreadable record hid the honest posts of the topic"
    );
}

// 正しい投稿(署名つき envelope と一致する header を、申告どおりの replica へ置いたもの)は、これまでどおり反映する。
#[tokio::test]
async fn consistent_post_is_still_projected() {
    let fixture = integrity_fixture("consistent").await;
    let author_keys = generate_keys();
    let envelope = signed_post(
        &author_keys,
        &fixture.topic,
        "an honest post",
        ObjectVisibility::Public,
        None,
    );
    let header = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    write_object_entries(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        Some(&envelope),
        &header,
    )
    .await;

    assert!(wait_for_row(&fixture.viewer_store, &envelope.id).await);
    let view = public_timeline_after_settle(&fixture).await;
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == envelope.id.as_str())
        .expect("the honest post is listed");
    assert_eq!(item.author_pubkey, author_keys.public_key_hex());
    assert_eq!(item.content, "an honest post");
}

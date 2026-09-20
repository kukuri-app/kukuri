//! #1239: 取り下げの個別反映と、利用者の操作の読み出しの policy を固定する。
//!
//! 独立監査(PR #1246)の再現 test を恒久化したもの。

use super::*;

/// docs event で投稿が反映される viewer と、その topic の replica。
struct EventFixture {
    docs_sync: Arc<MemoryDocsSync>,
    viewer: AppService,
    viewer_store: Arc<MemoryStore>,
    author_keys: KukuriKeys,
    topic: TopicId,
    replica: ReplicaId,
}

/// viewer が先に topic を購読し、docs event で 1 件が反映されることを確かめてから始める。
async fn event_fixture(name: &str) -> EventFixture {
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
    let topic = TopicId::new(format!("kukuri:topic:event-{name}").as_str());
    let replica = topic_replica_id(topic.as_str());
    viewer
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("empty timeline");
    sleep(Duration::from_millis(150)).await;
    let author_keys = generate_keys();
    let first = persist_test_post(
        docs_sync.as_ref(),
        None,
        &author_keys,
        &topic,
        PayloadRef::InlineText {
            text: "first post".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    assert!(
        wait_for_projection(&viewer_store, &first.id).await,
        "the first post must be projected by the docs event"
    );
    EventFixture {
        docs_sync,
        viewer,
        viewer_store,
        author_keys,
        topic,
        replica,
    }
}

async fn wait_for_projection(store: &Arc<MemoryStore>, object_id: &EnvelopeId) -> bool {
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

fn public_post_envelope(fixture: &EventFixture, text: &str) -> KukuriEnvelope {
    build_post_envelope_with_payload_in_channel(
        &fixture.author_keys,
        &fixture.topic,
        PayloadRef::InlineText { text: text.into() },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("post envelope")
}

/// remote の同期と同じ key の昇順(`objects/<id>/envelope` → `objects/<id>/state`)で投稿を書く。
async fn write_post_in_key_order(fixture: &EventFixture, envelope: &KukuriEnvelope) {
    let object = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    for (suffix, value) in [
        (
            "envelope",
            serde_json::to_value(envelope).expect("envelope json"),
        ),
        ("state", serde_json::to_value(&object).expect("state json")),
    ] {
        fixture
            .docs_sync
            .apply_doc_op(
                &fixture.replica,
                DocOp::SetJson {
                    key: stable_key("objects", &format!("{}/{suffix}", envelope.id.as_str())),
                    value,
                },
            )
            .await
            .expect("write post entry");
    }
}

// ADR 0052 §2: 取り下げとして読めない record は取り下げとして扱わない。public topic の replica は誰でも
// 書けるので、読めない record を置くだけで特定の投稿を隠せないようにする(独立監査の再現 test を恒久化)。
#[tokio::test]
async fn unreadable_withdrawal_record_does_not_hide_the_post() {
    let fixture = event_fixture("unreadable-withdrawal").await;
    let post = public_post_envelope(&fixture, "visible body");
    fixture
        .docs_sync
        .apply_doc_op(
            &fixture.replica,
            DocOp::SetJson {
                key: stable_key("withdrawals", &format!("{}/state", post.id.as_str())),
                value: serde_json::json!({ "not": "a withdrawal envelope" }),
            },
        )
        .await
        .expect("write an unreadable withdrawal record");
    write_post_in_key_order(&fixture, &post).await;

    assert!(
        wait_for_projection(&fixture.viewer_store, &post.id).await,
        "an unreadable withdrawal record must not stop the post from being projected"
    );
    let view = fixture
        .viewer
        .list_timeline(fixture.topic.as_str(), None, 20)
        .await
        .expect("timeline");
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == post.id.as_str())
        .expect("the post is listed");
    assert_eq!(item.content, "visible body");
    assert!(item.withdrawal.is_none());
}

// 取り下げの署名者が対象の著者と違う record も、取り下げとして扱わない。
#[tokio::test]
async fn withdrawal_record_signed_by_another_author_does_not_hide_the_post() {
    let fixture = event_fixture("forged-withdrawal").await;
    let post = public_post_envelope(&fixture, "still visible");
    // 攻撃者は、対象と同じ内容を自分の鍵で作った envelope に対する取り下げを署名し、対象の key に置く。
    let attacker_keys = generate_keys();
    let attacker_copy = build_post_envelope_with_payload_in_channel(
        &attacker_keys,
        &fixture.topic,
        PayloadRef::InlineText {
            text: "still visible".into(),
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("attacker envelope");
    let mut forged = build_post_withdrawal_envelope(
        &attacker_keys,
        &attacker_copy,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("forged withdrawal");
    // 対象の id を指すよう content を書き換える(署名は合わなくなる)。
    forged.content = forged
        .content
        .replace(attacker_copy.id.as_str(), post.id.as_str());
    fixture
        .docs_sync
        .apply_doc_op(
            &fixture.replica,
            DocOp::SetJson {
                key: stable_key("withdrawals", &format!("{}/state", post.id.as_str())),
                value: serde_json::to_value(&forged).expect("forged json"),
            },
        )
        .await
        .expect("write the forged withdrawal");
    write_post_in_key_order(&fixture, &post).await;

    assert!(
        wait_for_projection(&fixture.viewer_store, &post.id).await,
        "a forged withdrawal record must not stop the post from being projected"
    );
    let view = fixture
        .viewer
        .list_timeline(fixture.topic.as_str(), None, 20)
        .await
        .expect("timeline");
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == post.id.as_str())
        .expect("the post is listed");
    assert_eq!(item.content, "still visible");
    assert!(item.withdrawal.is_none());
}

// INVAR-1: 正しい取り下げが対象の entry より先に届いた場合(取り下げの event の時点では検証できない)でも、
// 投稿を反映するときに取り下げを先に確認して本文を伏せる(独立監査の再現 test を恒久化)。
#[tokio::test]
async fn withdrawal_that_arrives_before_the_post_masks_it_when_the_post_is_projected() {
    let fixture = event_fixture("withdrawal-first").await;
    let post = public_post_envelope(&fixture, "body withdrawn before it arrived");
    let withdrawal = build_post_withdrawal_envelope(
        &fixture.author_keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("withdrawal envelope");
    fixture
        .docs_sync
        .apply_doc_op(
            &fixture.replica,
            DocOp::SetJson {
                key: stable_key("withdrawals", &format!("{}/state", post.id.as_str())),
                value: serde_json::to_value(&withdrawal).expect("withdrawal json"),
            },
        )
        .await
        .expect("write the withdrawal first");
    sleep(Duration::from_millis(150)).await;
    write_post_in_key_order(&fixture, &post).await;

    assert!(wait_for_projection(&fixture.viewer_store, &post.id).await);
    let masked = timeout(Duration::from_secs(3), async {
        loop {
            let view = fixture
                .viewer
                .list_timeline(fixture.topic.as_str(), None, 20)
                .await
                .expect("timeline");
            if view
                .items
                .iter()
                .find(|item| item.object_id == post.id.as_str())
                .is_some_and(|item| item.withdrawal.is_some() && item.content.is_empty())
            {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        masked,
        "the body of a post whose withdrawal arrived first stayed visible"
    );
}

// ADR 0052 §4: 利用者の操作は remote 取得を待たない。`HangingRemoteOnMissDocsSync` は、`LocalThenRemote` の
// 読み出しが空振りすると 30 秒待つ(独立監査の再現 test を恒久化)。
#[tokio::test]
async fn user_operations_do_not_wait_for_a_remote_docs_fetch() {
    let app = app_with_hanging_remote_docs(
        Arc::new(MemoryStore::default()),
        Arc::new(HangingRemoteOnMissDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:user-operations-remote-wait";
    let target = app
        .create_post(topic, "target", None)
        .await
        .expect("create target");
    let limit = Duration::from_secs(3);

    timeout(
        limit,
        app.toggle_reaction(
            topic,
            target.as_str(),
            ReactionKeyV1::Emoji {
                emoji: "👍".into()
            },
            Some(ChannelRef::Public),
        ),
    )
    .await
    .expect("reaction must not wait for a remote fetch")
    .expect("reaction");
    timeout(limit, app.bookmark_post(topic, target.as_str()))
        .await
        .expect("bookmark must not wait for a remote fetch")
        .expect("bookmark");
    timeout(
        limit,
        app.create_post(topic, "reply", Some(target.as_str())),
    )
    .await
    .expect("reply must not wait for a remote fetch")
    .expect("reply");
    let missing = "d".repeat(64);
    let missing_bookmark = timeout(limit, app.bookmark_post(topic, missing.as_str()))
        .await
        .expect("a missing target must fail without waiting for a remote fetch");
    assert!(missing_bookmark.is_err());
    app.shutdown().await;
}

// 利用者の操作の key 指定の反映は、取り下げの中の読み出し(対象の envelope と state)も `LocalOnly` で行う。
// 取り下げの record はあるが対象の envelope が手元に無い object でも、remote を待たずに操作が返る
// (独立監査の再現を恒久化。取り下げの中の読み出しを `LocalThenRemote` に固定する mutation を検出する)。
#[tokio::test]
async fn user_operation_does_not_wait_for_a_remote_fetch_inside_the_withdrawal_check() {
    let docs_sync = Arc::new(HangingRemoteOnMissDocsSync::default());
    let app = app_with_hanging_remote_docs(
        Arc::new(MemoryStore::default()),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:user-operation-nested-remote-wait");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let post = build_post_envelope_with_payload_in_channel(
        &author_keys,
        &topic,
        PayloadRef::InlineText {
            text: "withdrawn, envelope not delivered yet".into(),
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("post envelope");
    let withdrawal = build_post_withdrawal_envelope(
        &author_keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal envelope");
    let object = post
        .to_post_object()
        .expect("post object")
        .expect("post object");
    // 対象の envelope は書かない(まだ届いていない)。state と取り下げだけがある。
    for (key, value) in [
        (
            stable_key("objects", &format!("{}/state", post.id.as_str())),
            serde_json::to_value(&object).expect("state json"),
        ),
        (
            stable_key("withdrawals", &format!("{}/state", post.id.as_str())),
            serde_json::to_value(&withdrawal).expect("withdrawal json"),
        ),
    ] {
        docs_sync
            .apply_doc_op(&replica, DocOp::SetJson { key, value })
            .await
            .expect("write entry");
    }

    timeout(
        Duration::from_secs(3),
        app.bookmark_post(topic.as_str(), post.id.as_str()),
    )
    .await
    .expect("the operation must not wait for a remote fetch inside the withdrawal check")
    .expect("bookmark");
    app.shutdown().await;
}

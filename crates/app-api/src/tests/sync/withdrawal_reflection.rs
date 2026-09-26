//! #1239: 取り下げの個別反映と、利用者の操作の読み出しの policy を固定する。
//!
//! 独立監査(PR #1246)の再現 test を恒久化したもの。

use super::*;

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

    // 対象の envelope が無いので、#1248 の後は投稿が反映されず、操作は「対象が無い」で失敗する。
    // ここで固定するのは、取り下げの中の読み出しが remote を待たずに、操作がすぐ返ること。
    let outcome = timeout(
        Duration::from_secs(3),
        app.bookmark_post(topic.as_str(), post.id.as_str()),
    )
    .await
    .expect("the operation must not wait for a remote fetch inside the withdrawal check");
    assert!(
        outcome.is_err(),
        "a post without a signed envelope is not projected"
    );
    app.shutdown().await;
}

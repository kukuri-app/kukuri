//! #1239: 利用者の操作と表示が読む docs の record 数が、replica の大きさに依存しないことを固定する。
//!
//! 判定は所要時間ではなく、docs の query が返した record(または key)の数で行う。

use super::*;

const SMALL: usize = 20;
const LARGE: usize = 400;

struct Fixture {
    app: AppService,
    store: Arc<MemoryStore>,
    docs_sync: Arc<CountingDocsSync>,
    topic: TopicId,
    target: KukuriEnvelope,
}

/// `posts` 件の投稿を持つ topic を作る。docs と projection の両方へ入れる。
async fn fixture(name: &str, posts: usize) -> Fixture {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let remote_keys = generate_keys();
    let topic = TopicId::new(format!("kukuri:topic:scale-{name}-{posts}").as_str());
    let mut target = None;
    for index in 0..posts {
        let envelope = persist_test_post(
            docs_sync.as_ref(),
            Some(store.as_ref()),
            &remote_keys,
            &topic,
            PayloadRef::InlineText {
                text: format!("post {index}"),
            },
            Vec::new(),
            None,
        )
        .await;
        target.get_or_insert(envelope);
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        keys,
    );
    // 購読タスクの起動時の処理を済ませてから数える。
    app.list_timeline(topic.as_str(), None, 5)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(150)).await;
    Fixture {
        app,
        store,
        docs_sync,
        topic,
        target: target.expect("at least one post"),
    }
}

/// `operation` が読んだ docs の record 数を、小さい replica と大きい replica で比べる。
async fn records_read_by<F, Fut>(name: &str, operation: F) -> (usize, usize)
where
    F: Fn(Fixture) -> Fut,
    Fut: std::future::Future<Output = Arc<CountingDocsSync>>,
{
    let mut counts = Vec::new();
    for posts in [SMALL, LARGE] {
        let fixture = fixture(name, posts).await;
        fixture.docs_sync.reset_records_returned();
        let docs_sync = operation(fixture).await;
        counts.push(docs_sync.records_returned());
    }
    (counts[0], counts[1])
}

fn assert_independent_of_replica_size(operation: &str, small: usize, large: usize) {
    assert_eq!(
        small, large,
        "{operation}: docs records read must not depend on the replica size \
         ({SMALL} posts -> {small} records, {LARGE} posts -> {large} records)"
    );
}

// TR-7: 対象の行が projection にあるとき、reaction は replica を走査しない。
#[tokio::test]
async fn reaction_reads_a_constant_number_of_docs_records() {
    let (small, large) = records_read_by("reaction", |fixture| async move {
        fixture
            .app
            .toggle_reaction(
                fixture.topic.as_str(),
                fixture.target.id.as_str(),
                ReactionKeyV1::Emoji {
                    emoji: "👍".into()
                },
                Some(ChannelRef::Public),
            )
            .await
            .expect("toggle reaction");
        fixture.docs_sync
    })
    .await;
    assert_independent_of_replica_size("reaction", small, large);
}

// TR-7: 対象の行が projection に無いときは、対象の key だけを読んで反映する。
#[tokio::test]
async fn reaction_on_an_unprojected_target_reads_only_the_target_keys() {
    let (small, large) = records_read_by("reaction-missing", |fixture| async move {
        ObjectProjectionStore::rebuild_object_projections(fixture.store.as_ref(), Vec::new())
            .await
            .expect("clear projections");
        let state = fixture
            .app
            .toggle_reaction(
                fixture.topic.as_str(),
                fixture.target.id.as_str(),
                ReactionKeyV1::Emoji {
                    emoji: "👍".into()
                },
                Some(ChannelRef::Public),
            )
            .await
            .expect("toggle reaction after key hydration");
        assert_eq!(state.my_reactions.len(), 1);
        fixture.docs_sync
    })
    .await;
    assert_independent_of_replica_size("reaction on an unprojected target", small, large);
}

#[tokio::test]
async fn bookmark_reads_a_constant_number_of_docs_records() {
    let (small, large) = records_read_by("bookmark", |fixture| async move {
        fixture
            .app
            .bookmark_post_in_channel(
                fixture.topic.as_str(),
                fixture.target.id.as_str(),
                ChannelRef::Public,
            )
            .await
            .expect("bookmark");
        fixture.docs_sync
    })
    .await;
    assert_independent_of_replica_size("bookmark", small, large);
}

#[tokio::test]
async fn reply_reads_a_constant_number_of_docs_records() {
    let (small, large) = records_read_by("reply", |fixture| async move {
        fixture
            .app
            .create_post_in_channel(
                fixture.topic.as_str(),
                ChannelRef::Public,
                "reply body",
                Some(fixture.target.id.as_str()),
            )
            .await
            .expect("reply");
        fixture.docs_sync
    })
    .await;
    assert_independent_of_replica_size("reply", small, large);
}

// repost は、既存の repost の検索(以前は topic の全 `objects/` を読んで deserialize していた)を含む。
#[tokio::test]
async fn repost_reads_a_constant_number_of_docs_records() {
    let (small, large) = records_read_by("repost", |fixture| async move {
        let first = fixture
            .app
            .create_repost(
                fixture.topic.as_str(),
                fixture.topic.as_str(),
                fixture.target.id.as_str(),
                None,
            )
            .await
            .expect("repost");
        // 同じ投稿の 2 回目の repost は、既存の repost を返す。
        let second = fixture
            .app
            .create_repost(
                fixture.topic.as_str(),
                fixture.topic.as_str(),
                fixture.target.id.as_str(),
                None,
            )
            .await
            .expect("repost again");
        assert_eq!(first, second, "a simple repost is created once");
        fixture.docs_sync
    })
    .await;
    assert_independent_of_replica_size("repost", small, large);
}

// ADR 0016 §2.3: 取り下げ済みの simple repost は既存の repost として扱わない。
// 以前は docs の state が残るため、取り下げた repost の id を返し続け、同じ投稿を repost し直せなかった。
#[tokio::test]
async fn withdrawn_simple_repost_can_be_reposted_again() {
    let fixture = fixture("repost-after-withdrawal", SMALL).await;
    let first = fixture
        .app
        .create_repost(
            fixture.topic.as_str(),
            fixture.topic.as_str(),
            fixture.target.id.as_str(),
            None,
        )
        .await
        .expect("repost");
    fixture
        .app
        .withdraw_post(
            fixture.topic.as_str(),
            first.as_str(),
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Private,
            None,
        )
        .await
        .expect("withdraw the repost");
    // envelope の id は著者・秒単位の作成時刻・内容から決まる。同じ秒のうちに作り直すと取り下げた
    // repost と同じ id になるので、秒をまたいでから repost し直す。
    sleep(Duration::from_millis(1_100)).await;
    let second = fixture
        .app
        .create_repost(
            fixture.topic.as_str(),
            fixture.topic.as_str(),
            fixture.target.id.as_str(),
            None,
        )
        .await
        .expect("repost again after the withdrawal");
    assert_ne!(
        first, second,
        "a withdrawn simple repost is not reused as the existing repost"
    );
    let third = fixture
        .app
        .create_repost(
            fixture.topic.as_str(),
            fixture.topic.as_str(),
            fixture.target.id.as_str(),
            None,
        )
        .await
        .expect("repost a third time");
    assert_eq!(second, third, "the new simple repost is created once");
}

#[tokio::test]
async fn withdrawal_reads_a_constant_number_of_docs_records() {
    let mut counts = Vec::new();
    for posts in [SMALL, LARGE] {
        let fixture = fixture("withdraw", posts).await;
        let own = fixture
            .app
            .create_post(fixture.topic.as_str(), "to be withdrawn", None)
            .await
            .expect("create own post");
        fixture.docs_sync.reset_records_returned();
        fixture
            .app
            .withdraw_post(
                fixture.topic.as_str(),
                own.as_str(),
                ChannelRef::Public,
                None,
                WithdrawalReasonVisibility::Private,
                None,
            )
            .await
            .expect("withdraw");
        counts.push(fixture.docs_sync.records_returned());
    }
    assert_independent_of_replica_size("withdrawal", counts[0], counts[1]);
}

#[tokio::test]
async fn community_index_resolution_reads_a_constant_number_of_docs_records() {
    let (small, large) = records_read_by("community-index", |fixture| async move {
        let response = fixture
            .app
            .resolve_community_index_posts(vec![CommunityIndexPostResolveInput {
                key: "entry".into(),
                topic: fixture.topic.as_str().to_string(),
                object_id: fixture.target.id.as_str().to_string(),
                author_pubkey: fixture.target.pubkey.as_str().to_string(),
                channel_ref: ChannelRef::Public,
            }])
            .await
            .expect("resolve");
        assert!(response.entries[0].post.is_some());
        fixture.docs_sync
    })
    .await;
    assert_independent_of_replica_size("community index resolution", small, large);
}

// TR-9 / AC-6: view の生成中に docs を読まない。repost を含むページでも、取り下げは projection の表だけで判定する。
// repost 元の取り下げは背景で確認するが、key を 1 つ指定した読み出しだけで、同じ object の確認は間隔を空ける。
#[tokio::test]
async fn timeline_view_generation_does_not_scan_docs() {
    let fixture = fixture("view", SMALL).await;
    // 同じ秒の中の並びは object id で決まる。repost を先頭のページに確実に入れるため、秒をまたいでから作る。
    sleep(Duration::from_millis(1_100)).await;
    fixture
        .app
        .create_repost(
            fixture.topic.as_str(),
            fixture.topic.as_str(),
            fixture.target.id.as_str(),
            None,
        )
        .await
        .expect("repost");
    sleep(Duration::from_millis(150)).await;
    fixture.docs_sync.clear_queries().await;

    // projection が尽きないページを取る(尽きたページは、取得側が範囲を索引と照合する。
    // それは view の生成ではなく、`range_reconcile.rs` が固定する)。
    for _ in 0..3 {
        let view = fixture
            .app
            .list_timeline(fixture.topic.as_str(), None, 5)
            .await
            .expect("timeline with a repost");
        assert!(view.next_cursor.is_some());
        assert!(view.items.iter().any(|item| item.repost_of.is_some()));
    }
    sleep(Duration::from_millis(150)).await;

    // author の replica の読み出し(著者の購読。段階 T6 が扱う)は除き、topic の replica だけを見る。
    let topic_replica = topic_replica_id(fixture.topic.as_str());
    let queries = fixture
        .docs_sync
        .queries()
        .await
        .into_iter()
        .filter(|(replica, _)| replica == topic_replica.as_str())
        .collect::<Vec<_>>();
    let withdrawal_key = stable_key(
        "withdrawals",
        &format!("{}/state", fixture.target.id.as_str()),
    );
    assert!(
        queries
            .iter()
            .all(|(_, query)| *query == DocQuery::Exact(withdrawal_key.clone())),
        "listing a projected page may only check the repost source withdrawal by key, got {queries:?}"
    );
    assert!(
        queries.len() <= 1,
        "the background withdrawal check is spaced per object, got {queries:?}"
    );
}

// INVAR-1: 購読していない topic の投稿を repost した行でも、repost 元が取り下げられていれば本文を表示しない。
// view の生成は docs を読まず、背景の確認(key 指定)が projection の取り下げ表を埋める。
#[tokio::test]
async fn withdrawn_repost_source_in_an_unsubscribed_topic_is_masked_after_the_background_check() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let author_store = Arc::new(MemoryStore::default());
    let author = app_service_from_dependencies(
        author_store.clone(),
        author_store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    let viewer_store = Arc::new(MemoryStore::default());
    let viewer = app_service_from_dependencies(
        viewer_store.clone(),
        viewer_store,
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        generate_keys(),
    );
    let source_topic = "kukuri:topic:withdrawn-repost-source";
    let target_topic = "kukuri:topic:withdrawn-repost-target";
    let source = author
        .create_post(source_topic, "body to be withdrawn", None)
        .await
        .expect("source post");
    let repost = author
        .create_repost(target_topic, source_topic, source.as_str(), None)
        .await
        .expect("repost");
    author
        .withdraw_post(
            source_topic,
            source.as_str(),
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )
        .await
        .expect("withdraw source");

    timeout(Duration::from_secs(5), async {
        loop {
            let view = viewer
                .list_timeline(target_topic, None, 20)
                .await
                .expect("target timeline");
            let item = view
                .items
                .iter()
                .find(|item| item.object_id == repost)
                .expect("repost row");
            let repost_of = item.repost_of.as_ref().expect("repost snapshot");
            if repost_of.content.is_empty() {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the withdrawn repost source must be masked");
}

// 取り下げの docs event は key 単位で反映する。hint が無くても、全件走査を待たずに projection へ入る。
#[tokio::test]
async fn withdrawal_doc_event_is_reflected_by_key_without_scanning() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let author_store = Arc::new(MemoryStore::default());
    let author = app_service_from_dependencies(
        author_store.clone(),
        author_store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    let viewer_store = Arc::new(MemoryStore::default());
    let viewer = app_service_from_dependencies(
        viewer_store.clone(),
        viewer_store.clone(),
        transport,
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service,
        generate_keys(),
    );
    let topic = "kukuri:topic:withdrawal-doc-event";
    let object_id = author
        .create_post(topic, "body to be withdrawn", None)
        .await
        .expect("post");
    let first = viewer
        .list_timeline(topic, None, 20)
        .await
        .expect("viewer timeline");
    assert_eq!(first.items[0].content, "body to be withdrawn");
    sleep(Duration::from_millis(150)).await;
    docs_sync.clear_queries().await;

    author
        .withdraw_post(
            topic,
            object_id.as_str(),
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )
        .await
        .expect("withdraw");

    timeout(Duration::from_secs(5), async {
        loop {
            if kukuri_store::PostWithdrawalStore::get_post_withdrawal(
                viewer_store.as_ref(),
                &EnvelopeId::from(object_id.as_str()),
            )
            .await
            .expect("withdrawal row")
            .is_some()
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the withdrawal doc event must reach the viewer projection");

    let view = viewer
        .list_timeline(topic, None, 20)
        .await
        .expect("viewer timeline after withdrawal");
    assert_eq!(view.items[0].content, "");
    assert!(view.items[0].withdrawal.is_some());

    let queries = docs_sync.queries().await;
    assert!(
        queries
            .iter()
            .all(|(_, query)| !matches!(query, DocQuery::Prefix(prefix) if prefix == "withdrawals/" || prefix == "objects/")),
        "a withdrawal doc event must not trigger a replica scan, got {queries:?}"
    );
}

// INVAR-2: private channel の対象は、参加状態の確認を通らなければ読まない。
#[tokio::test]
async fn private_channel_target_is_not_read_without_membership() {
    let fixture = fixture("private-negative", SMALL).await;
    fixture.docs_sync.clear_queries().await;
    let channel_id = ChannelId::new("channel-the-viewer-never-joined");

    let result = fixture
        .app
        .bookmark_post_in_channel(
            fixture.topic.as_str(),
            fixture.target.id.as_str(),
            ChannelRef::PrivateChannel {
                channel_id: channel_id.clone(),
            },
        )
        .await;
    assert!(
        result.is_err(),
        "a non-member must not bookmark through a private channel scope"
    );

    let missing = EnvelopeId::from("f".repeat(64).as_str());
    let scoped = fixture
        .app
        .ensure_object_projection(
            fixture.topic.as_str(),
            &TimelineScope::Channel { channel_id },
            &missing,
            DocFetchPolicy::LocalOnly,
        )
        .await;
    assert!(
        scoped.is_err(),
        "scope replicas require private channel access"
    );

    let queries = fixture.docs_sync.queries().await;
    assert!(
        queries
            .iter()
            .all(|(replica, _)| !replica.starts_with("channel::")),
        "no private channel replica may be read without membership, got {queries:?}"
    );
}

// 対象が projection にも docs にも無いときは、走査せずに失敗を返す。
#[tokio::test]
async fn missing_target_fails_without_scanning() {
    let fixture = fixture("missing-target", SMALL).await;
    fixture.docs_sync.clear_queries().await;
    let missing = "e".repeat(64);

    let result = fixture
        .app
        .toggle_reaction(
            fixture.topic.as_str(),
            missing.as_str(),
            ReactionKeyV1::Emoji {
                emoji: "👍".into()
            },
            Some(ChannelRef::Public),
        )
        .await;
    assert!(result.is_err());

    let queries = fixture.docs_sync.queries().await;
    assert!(
        queries
            .iter()
            .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
        "a missing target must be resolved by key only, got {queries:?}"
    );
}

fn shared_docs_app(
    docs_sync: &Arc<MemoryDocsSync>,
    blob_service: &Arc<MemoryBlobService>,
    keys: KukuriKeys,
) -> AppService {
    let store = Arc::new(MemoryStore::default());
    app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        keys,
    )
}

// INVAR-1 / ADR 0032 §2(返信先 preview): 購読していない topic の投稿を profile で表示するとき、
// 取り下げ済みの返信先の本文を reply preview に出さない(独立監査の再現 test を恒久化)。
#[tokio::test]
async fn withdrawn_reply_parent_in_an_unsubscribed_topic_is_masked_in_the_profile_reply_preview() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let parent_author = shared_docs_app(&docs_sync, &blob_service, generate_keys());
    let reply_keys = generate_keys();
    let reply_author_pubkey = reply_keys.public_key_hex();
    let reply_author = shared_docs_app(&docs_sync, &blob_service, reply_keys);
    let viewer = shared_docs_app(&docs_sync, &blob_service, generate_keys());
    let topic = "kukuri:topic:unsubscribed-reply-parent";

    let parent = parent_author
        .create_post(topic, "parent body to be withdrawn", None)
        .await
        .expect("parent post");
    let reply = reply_author
        .create_post(topic, "reply body", Some(parent.as_str()))
        .await
        .expect("reply post");
    parent_author
        .withdraw_post(
            topic,
            parent.as_str(),
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )
        .await
        .expect("withdraw parent");

    // viewer は topic を購読しない。返信した著者の profile だけを開く。
    let mut last_preview_content = None;
    let masked = timeout(Duration::from_secs(5), async {
        loop {
            let view = viewer
                .list_profile_timeline(reply_author_pubkey.as_str(), None, 20)
                .await
                .expect("profile timeline");
            let item = view
                .items
                .iter()
                .find(|item| item.object_id == reply)
                .expect("reply row in the profile timeline");
            match item.reply_preview.as_ref() {
                None => break,
                Some(preview) if preview.content.is_empty() => break,
                Some(preview) => last_preview_content = Some(preview.content.clone()),
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        masked,
        "the withdrawn reply parent body stayed visible in the profile reply preview: \
         {last_preview_content:?}"
    );
}

// INVAR-2 / TR-12: 退出した private channel の投稿は、projection に行が残っていても操作できない。
// 参加状態の確認は、projection の早期 return より前に通す(独立監査の再現 test を恒久化)。
#[tokio::test]
async fn left_private_channel_post_cannot_be_bookmarked_from_a_leftover_projection_row() {
    let (app, store, _docs, _blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:bookmark-after-leave";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "leftover".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_ref = ChannelRef::PrivateChannel {
        channel_id: ChannelId::new(channel.channel_id.clone()),
    };
    let object_id = app
        .create_post_in_channel(topic, channel_ref.clone(), "private body", None)
        .await
        .expect("private post");
    app.leave_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("leave private channel");
    assert!(
        ObjectProjectionStore::get_object_projection(
            store.as_ref(),
            &EnvelopeId::from(object_id.as_str()),
        )
        .await
        .expect("projection")
        .is_some(),
        "the projection row is still there after leaving"
    );

    let outcome = app
        .bookmark_post_in_channel(topic, object_id.as_str(), channel_ref)
        .await;
    assert!(
        outcome.is_err(),
        "a left private channel post must not be bookmarked from a leftover projection row"
    );
    assert!(
        app.list_bookmarked_posts()
            .await
            .expect("bookmarks")
            .is_empty(),
        "no bookmark may be stored"
    );
}

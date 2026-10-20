//! #1840 AC-2: topic の公開スレッドの一覧。別の端末が作ったスレッドを 2 つの並び順で一覧から取得し、対象の
//! スレッドだけを開ける。別の端末の新しい投稿があったスレッドは、最後の投稿の順で上へ移る。metadata や投稿を読めない
//! 記録は数を返す。無関係なスレッドと投稿を増やしても、1 ページの読み出しと保持は増えない。
use super::public_channels::{TOPIC, channel_ref, device, device_with_store};
use super::sync::CountingDocsSync;
use super::*;
use kukuri_docs_sync::{BucketReplica, BucketScope, MemoryDocsSync, TimeBucket};
use kukuri_store::{PublicChannelOrder, PublicChannelStore};

/// 一覧の 1 ページの channel の ID。先頭のページの照合は 300 ms を過ぎると背景で続くので、`done` が満たされるまで
/// 取り直す(期限 10 秒)。
async fn listed_until(
    app: &AppService,
    order: PublicChannelOrder,
    done: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ids = app
            .list_public_channels(TOPIC, order, None, 20)
            .await
            .expect("list the public channels")
            .items
            .into_iter()
            .map(|channel| channel.channel_id)
            .collect::<Vec<_>>();
        if done(&ids) || tokio::time::Instant::now() >= deadline {
            return ids;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn today_bucket() -> ReplicaId {
    BucketReplica::new(
        BucketScope::Topic {
            topic_id: TOPIC.into(),
        },
        TimeBucket::from_unix_seconds(Utc::now().timestamp()).expect("bucket"),
    )
    .expect("replica")
    .replica_id()
}

#[tokio::test]
async fn another_peers_public_channels_are_listed_in_both_orders_and_move_up_on_new_posts() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("02".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let (b, b_store) = device_with_store("b", &network, b_docs, a_docs, blobs);

    let older = a
        .create_public_channel(TOPIC, "古いスレッド", "最初の投稿", Vec::new(), Vec::new())
        .await
        .expect("the older channel");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let newer = a
        .create_public_channel(TOPIC, "新しいスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("the newer channel");
    let both = [newer.channel_id.clone(), older.channel_id.clone()];

    // 招待の無い B は、A のスレッドを両方の並び順で一覧から取得する。一覧は channel の投稿を読まない。
    for order in [PublicChannelOrder::Created, PublicChannelOrder::LastPost] {
        assert_eq!(listed_until(&b, order, |ids| ids.len() == 2).await, both);
    }
    let first = older.first_post_id.clone().expect("the first post");
    assert!(
        b_store
            .get_object_projection(&EnvelopeId::from(first.as_str()))
            .await
            .expect("projection")
            .is_none(),
        "listing does not read the posts of the channels"
    );

    // 対象のスレッドだけを開ける(ID から metadata と最初の投稿を読む)。もう一方の投稿は読まない。
    b.get_public_channel(TOPIC, &older.channel_id)
        .await
        .expect("open the older channel")
        .expect("found");
    assert!(
        b.list_thread(TOPIC, &first, None, 50)
            .await
            .expect("the first post on B")
            .items
            .iter()
            .any(|post| post.object_id == first)
    );

    // B が古いスレッドに投稿すると、A の最後の投稿の順でそのスレッドが上へ移る。作成の順は変わらない。
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    b.create_post_in_channel(
        TOPIC,
        channel_ref(&older.channel_id),
        "B の新しい投稿",
        None,
    )
    .await
    .expect("B posts into the older channel");
    let reordered = [older.channel_id.clone(), newer.channel_id.clone()];
    assert_eq!(
        listed_until(&a, PublicChannelOrder::LastPost, |ids| ids == reordered).await,
        reordered
    );
    assert_eq!(
        listed_until(&a, PublicChannelOrder::Created, |ids| ids.len() == 2).await,
        both
    );
}

// metadata を確かめられない作成の記録と、指している投稿を読めない最後の投稿の記録は、行に出さずに数を返す。
#[tokio::test]
async fn unreadable_index_entries_are_counted_and_left_out_of_the_list() {
    let a_docs = MemoryDocsSync::with_docs_author("03".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("04".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs.clone(), blobs);
    let real = a
        .create_public_channel(TOPIC, "読めるスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("a readable channel");

    let bucket = today_bucket();
    let now = Utc::now();
    let forged = ChannelId::public_channel(
        now.timestamp_millis(),
        generate_keys().public_key_hex().as_str(),
    );
    let seconds = now.timestamp();
    let pointed = format!("{}.{}", real.channel_id, "0".repeat(64));
    for (key, value) in [
        (
            format!("channels/list/{seconds:020}-{0}/{0}", forged.as_str()),
            serde_json::json!({ "not": "an envelope" }),
        ),
        (
            format!("channels/activity/{:020}-{pointed}/{pointed}", seconds + 1),
            serde_json::json!({}),
        ),
    ] {
        a_docs
            .apply_doc_op(&bucket, DocOp::SetJson { key, value })
            .await
            .expect("write an unreadable entry");
    }

    let created = loop_until_unavailable(&b, PublicChannelOrder::Created).await;
    assert_eq!(created.0, vec![real.channel_id.clone()]);
    assert_eq!(created.1, 1, "the forged metadata");
    let last_post = loop_until_unavailable(&b, PublicChannelOrder::LastPost).await;
    assert_eq!(last_post.0, vec![real.channel_id.clone()]);
    assert_eq!(last_post.1, 2, "the forged metadata and the missing post");
}

/// 一覧と、取得できなかった数。背景で続く照合が終わるまで取り直す(期限 10 秒)。
async fn loop_until_unavailable(app: &AppService, order: PublicChannelOrder) -> (Vec<String>, u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let page = app
            .list_public_channels(TOPIC, order, None, 20)
            .await
            .expect("list the public channels");
        let ids = page
            .items
            .into_iter()
            .map(|channel| channel.channel_id)
            .collect::<Vec<_>>();
        if (!ids.is_empty() && page.unavailable_count > 0)
            || tokio::time::Instant::now() >= deadline
        {
            return (ids, page.unavailable_count);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// 非表示の作者(mute)のスレッドは一覧に出さない。
#[tokio::test]
async fn hidden_creators_threads_are_left_out_of_the_list() {
    let a_docs = MemoryDocsSync::with_docs_author("05".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("06".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);
    let hidden = a
        .create_public_channel(TOPIC, "mute した作者のスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("a channel");
    let mine = b
        .create_public_channel(TOPIC, "自分のスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("an own channel");
    assert_eq!(
        listed_until(&b, PublicChannelOrder::Created, |ids| ids.len() == 2).await,
        [mine.channel_id.clone(), hidden.channel_id.clone()]
    );
    b.mute_author(a.current_author_pubkey().as_str())
        .await
        .expect("mute A");
    assert_eq!(
        listed_until(&b, PublicChannelOrder::Created, |ids| ids.len() == 1).await,
        [mine.channel_id]
    );
    // ID からも返さない(本文のリンクの card と詳細の見出しに出さない。AC-3)。
    assert_eq!(
        b.get_public_channel(TOPIC, &hidden.channel_id)
            .await
            .expect("get the hidden creator's channel"),
        None
    );
}

// topic の lease を持つ端末には作成の知らせが届き、一覧を開く前に行が置かれる。
#[tokio::test]
async fn a_creation_hint_places_the_row_on_a_device_with_the_topic_lease() {
    let a_docs = MemoryDocsSync::with_docs_author("07".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("08".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b_store = Arc::new(MemoryStore::default());
    let b_transport = Arc::new(FakeTransport::new("b", network.clone()));
    let b = app_service_from_dependencies(
        b_store.clone(),
        b_store.clone(),
        b_transport.clone(),
        b_transport,
        Arc::new(CountingDocsSync::reading_with(b_docs, Arc::new(a_docs))),
        blobs,
        generate_keys(),
    );
    b.switch_writer(1);
    b.set_scope_display(crate::ScopeDisplayRequest {
        observer: "test-topic".into(),
        target: crate::ScopeDisplayTarget::Timeline {
            topic: TOPIC.into(),
            scope: TimelineScope::Public,
        },
        visible: true,
    })
    .await
    .expect("open the topic column");
    let channel = a
        .create_public_channel(TOPIC, "知らせのあるスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("create");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while b_store
        .get_public_channel_row(&channel.channel_id)
        .await
        .expect("row")
        .is_none()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the creation hint places the row"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// 無関係なスレッドと投稿(別の topic のスレッド、topic のタイムラインの投稿)を増やしても、1 ページの照合が provider
// から読む record と key の一覧の数は変わらず、一覧は投稿を保持しない。投稿の作成は hint を間を空けて 3 回送るので、
// 投稿は少なくする。
#[tokio::test]
async fn a_list_page_reads_the_same_amount_however_many_unrelated_threads_and_posts_exist() {
    let mut reads = Vec::new();
    for (unrelated_channels, unrelated_posts_count) in [(5, 1), (20, 4)] {
        let provider_docs = MemoryDocsSync::with_docs_author("09".repeat(32));
        let blobs = Arc::new(MemoryBlobService::default());
        let network = FakeNetwork::default();
        let creator = device(
            "creator",
            &network,
            provider_docs.clone(),
            MemoryDocsSync::default(),
            blobs.clone(),
        );
        for index in 0..3 {
            creator
                .create_public_channel(
                    TOPIC,
                    &format!("スレッド {index}"),
                    "",
                    Vec::new(),
                    Vec::new(),
                )
                .await
                .expect("a listed channel");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        for _ in 0..unrelated_channels {
            creator
                .create_public_channel(
                    "kukuri:topic:unrelated",
                    "別の topic のスレッド",
                    "",
                    Vec::new(),
                    Vec::new(),
                )
                .await
                .expect("an unrelated channel");
        }
        let mut unrelated_posts = Vec::new();
        for index in 0..unrelated_posts_count {
            unrelated_posts.push(
                creator
                    .create_post(TOPIC, &format!("topic の投稿 {index}"), None)
                    .await
                    .expect("an unrelated post"),
            );
        }
        let provider = Arc::new(CountingDocsSync::reading_with(
            provider_docs,
            Arc::new(MemoryDocsSync::default()),
        ));
        let reader_store = Arc::new(MemoryStore::default());
        let reader_transport = Arc::new(FakeTransport::new("reader", network.clone()));
        let reader = app_service_from_dependencies(
            reader_store.clone(),
            reader_store.clone(),
            reader_transport.clone(),
            reader_transport,
            Arc::new(CountingDocsSync::reading_from(provider.clone())),
            blobs,
            generate_keys(),
        );
        reader.switch_writer(1);
        // 先頭のページの照合を、背景へ回さずに 1 回だけ行って数える。
        let mut counts = Vec::new();
        for order in [PublicChannelOrder::Created, PublicChannelOrder::LastPost] {
            provider.reset_records_returned();
            let before = provider.key_queries();
            reader
                .reconcile_public_channel_range(TOPIC, order, None, 20, &Default::default())
                .await
                .expect("reconcile the head page");
            counts.push((provider.records_returned(), provider.key_queries() - before));
        }
        for order in [PublicChannelOrder::Created, PublicChannelOrder::LastPost] {
            assert_eq!(
                listed_until(&reader, order, |ids| ids.len() == 3)
                    .await
                    .len(),
                3,
                "{order:?}"
            );
        }
        for post in &unrelated_posts {
            assert!(
                reader_store
                    .get_object_projection(&EnvelopeId::from(post.as_str()))
                    .await
                    .expect("projection")
                    .is_none(),
                "listing keeps no posts"
            );
        }
        reads.push(counts);
    }
    assert!(
        reads[0]
            .iter()
            .all(|(records, queries)| *records > 0 && *queries > 0),
        "{reads:?}"
    );
    assert_eq!(reads[0], reads[1], "{reads:?}");
}

// 自分の投稿でも、自分の端末の最後の投稿の順でそのスレッドが上へ移る(手元の反映が一覧の行の時刻を進める)。
#[tokio::test]
async fn an_own_post_moves_the_thread_up_on_the_own_device() {
    let a_docs = MemoryDocsSync::with_docs_author("0a".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("0b".repeat(32));
    let (a, a_store) = device_with_store(
        "a",
        &FakeNetwork::default(),
        a_docs,
        b_docs,
        Arc::new(MemoryBlobService::default()),
    );
    let older = a
        .create_public_channel(TOPIC, "古いスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("the older channel");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let newer = a
        .create_public_channel(TOPIC, "新しいスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("the newer channel");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    a.create_post_in_channel(TOPIC, channel_ref(&older.channel_id), "自分の投稿", None)
        .await
        .expect("an own post");
    let ids = a_store
        .list_public_channels(TOPIC, PublicChannelOrder::LastPost, None, 20)
        .await
        .expect("the own list")
        .items
        .into_iter()
        .map(|row| row.channel_id)
        .collect::<Vec<_>>();
    assert_eq!(ids, [older.channel_id, newer.channel_id]);
}

// 回収の期限を過ぎて、まだ回収されていない他人の行は、次の照合が確かめて置き直す(一覧から消えたままにしない)。
#[tokio::test]
async fn an_expired_row_waiting_for_reclaim_is_placed_again_by_the_next_reconcile() {
    let a_docs = MemoryDocsSync::with_docs_author("0c".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("0d".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b_store = Arc::new(
        kukuri_store::SqliteStore::connect_memory()
            .await
            .expect("sqlite"),
    );
    let b_transport = Arc::new(FakeTransport::new("b", network.clone()));
    let b = app_service_from_dependencies(
        b_store.clone(),
        b_store.clone(),
        b_transport.clone(),
        b_transport,
        Arc::new(CountingDocsSync::reading_with(b_docs, Arc::new(a_docs))),
        blobs,
        generate_keys(),
    );
    b.switch_writer(1);
    let channel = a
        .create_public_channel(TOPIC, "期限を過ぎるスレッド", "", Vec::new(), Vec::new())
        .await
        .expect("create");
    let listed = async || {
        b_store
            .list_public_channels(TOPIC, PublicChannelOrder::Created, None, 20)
            .await
            .expect("page")
            .items
            .into_iter()
            .map(|row| row.channel_id)
            .collect::<Vec<_>>()
    };
    let reconcile = async || {
        b.reconcile_public_channel_range(
            TOPIC,
            PublicChannelOrder::Created,
            None,
            20,
            &Default::default(),
        )
        .await
        .expect("reconcile")
    };
    reconcile().await;
    assert_eq!(listed().await, std::slice::from_ref(&channel.channel_id));
    // 7 日使わなかったことにする(回収の前)。
    sqlx::query("UPDATE remote_content_cache SET last_used_at = 0 WHERE kind = 'public_channel'")
        .execute(b_store.pool())
        .await
        .expect("age the row");
    assert!(listed().await.is_empty());
    reconcile().await;
    assert_eq!(listed().await, [channel.channel_id]);
}

// 続きは手元の行の続きから読み、前のページの行を繰り返さない(タイムラインと同じ。2026-10-17 のユーザー判断 13)。
// 照合の範囲(作成は 4 日)より古い行が手元にあっても、同じ行は 1 度だけ出る。
#[tokio::test]
async fn pages_continue_from_the_known_rows_without_repeating_them() {
    let (a, a_store) = device_with_store(
        "a",
        &FakeNetwork::default(),
        MemoryDocsSync::with_docs_author("0e".repeat(32)),
        MemoryDocsSync::with_docs_author("0f".repeat(32)),
        Arc::new(MemoryBlobService::default()),
    );
    let now = Utc::now().timestamp_millis();
    let mut known = Vec::new();
    for days in [0, 1, 8] {
        let created_at = now - days * 86_400_000;
        let channel_id =
            ChannelId::public_channel(created_at, a.current_author_pubkey().as_str()).0;
        a_store
            .put_public_channel(
                kukuri_store::PublicChannelRow {
                    channel_id: channel_id.clone(),
                    topic_id: TOPIC.into(),
                    title: format!("{days} 日前のスレッド"),
                    creator_pubkey: a.current_author_pubkey().as_str().into(),
                    created_at,
                    first_post_id: None,
                    last_post_at: created_at,
                },
                false,
            )
            .await
            .expect("a known row");
        known.push(channel_id);
    }
    for order in [PublicChannelOrder::Created, PublicChannelOrder::LastPost] {
        let mut listed = Vec::new();
        let mut cursor = None;
        for _ in 0..4 {
            let page = a
                .list_public_channels(TOPIC, order, cursor, 2)
                .await
                .expect("a page");
            listed.extend(page.items.into_iter().map(|channel| channel.channel_id));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(listed, known, "{order:?}");
    }
}

//! #1909: 保持する公開 topic を設定すると、画面の列なしに、topic の投稿と、後から作られた公開スレッドの metadata と
//! 投稿を取り込む。公開スレッドの数を増やしても、lease と task の数は変わらない。解除と停止で取り込みが止まる。読めない
//! 記録は取り直しの予定に入れて先へ進む。

use super::public_channels::{TOPIC, channel_ref, device};
use super::sync::CountingDocsSync;
use super::*;
use crate::topic_retention::RetentionState;
use kukuri_docs_sync::{BucketReplica, BucketScope, MemoryDocsSync, TimeBucket};
use kukuri_store::{PublicChannelStore, TopicRetentionStore};

const OTHER_TOPIC: &str = "kukuri:topic:retention-other";

/// 保持の store を持ち、`peer` の docs を provider として読む端末(CLI daemon と同じく、保持だけで取り込む)。
fn retaining_device(
    network: &FakeNetwork,
    own: MemoryDocsSync,
    peer: MemoryDocsSync,
    blobs: Arc<MemoryBlobService>,
) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("k", network.clone()));
    let app = AppService::from_handles(
        ServiceHandles::new(
            store.clone(),
            store.clone(),
            transport.clone(),
            transport,
            Arc::new(CountingDocsSync::reading_with(own, Arc::new(peer))),
            blobs,
            generate_keys(),
        )
        .with_topic_retention(store.clone(), std::env::temp_dir()),
    );
    app.switch_writer(1);
    (app, store)
}

/// 続きが無くなるまで topic を進める(task の間隔を待たない)。前日より前の日は 1 回に 1 日ずつ読むので、既定の期間(30 日)の
/// 日数より多く回す。手で回すので、取り込みの task は止める。
async fn catch_up(app: &AppService, topic: &str) {
    app.shutdown_topic_retention().await;
    let mut state = RetentionState::default();
    for _ in 0..64 {
        if !app
            .step_topic_retention(topic, &mut state)
            .await
            .expect("retention step")
        {
            return;
        }
    }
    panic!("retention did not catch up");
}

async fn lease_keys(app: &AppService) -> Vec<ScopeKey> {
    let mut keys = app.subscription_registry.scope_leases.lock().await.keys();
    keys.sort();
    keys
}

#[tokio::test]
async fn retained_topics_take_topic_and_new_public_channel_posts_without_columns() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let k_docs = MemoryDocsSync::with_docs_author("03".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), k_docs.clone(), blobs.clone());
    let (k, k_store) = retaining_device(&network, k_docs, a_docs, blobs);

    // K は画面の列を開かずに、2 つの topic を保持する。
    for topic in [TOPIC, OTHER_TOPIC] {
        k.set_topic_retention(topic, true)
            .await
            .expect("retain the topic");
    }
    assert_eq!(
        lease_keys(&k).await,
        vec![
            ScopeKey::Topic(TOPIC.into()),
            ScopeKey::Topic(OTHER_TOPIC.into())
        ]
    );
    assert!(
        k.subscription_registry
            .topic_retention_task
            .lock()
            .await
            .as_ref()
            .is_some_and(|task| !task.is_finished()),
        "setting a retained topic starts the retention task"
    );
    // A の書込みの間に task が回ると、同じ秒の後の投稿が続きの位置より前に並んで読まれないことがある(判断 6 で受け入れた
    // 取りこぼし)。前日より前の日を読む間は task が続けて回るので、ここで止めて手で回す。
    k.shutdown_topic_retention().await;

    // A は、K が保持を始めた後に、topic へ投稿し、公開スレッドを作って投稿し、もう 1 つの topic へも投稿する。
    let topic_post = a
        .create_post(TOPIC, "topic の投稿", None)
        .await
        .expect("topic post");
    let channel = a
        .create_public_channel(
            TOPIC,
            "後から作ったスレッド",
            "最初の投稿",
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("public channel");
    let first_post = channel.first_post_id.clone().expect("first post");
    let channel_post = a
        .create_post_in_channel(
            TOPIC,
            channel_ref(&channel.channel_id),
            "スレッドの投稿",
            None,
        )
        .await
        .expect("channel post");
    let other_post = a
        .create_post(OTHER_TOPIC, "別の topic の投稿", None)
        .await
        .expect("other topic post");

    catch_up(&k, TOPIC).await;
    catch_up(&k, OTHER_TOPIC).await;

    // 検証して取り込んだ行がある。公開スレッドの投稿は、そのスレッドの文脈で、topic のタイムラインには入らない。
    for (id, channel_id) in [
        (&topic_post, "public"),
        (&other_post, "public"),
        (&first_post, channel.channel_id.as_str()),
        (&channel_post, channel.channel_id.as_str()),
    ] {
        let row = k_store
            .get_object_projection(&EnvelopeId::from(id.as_str()))
            .await
            .expect("projection")
            .unwrap_or_else(|| panic!("{id} is retained"));
        assert_eq!(row.channel_id, channel_id, "{id}");
    }
    let row = k_store
        .get_public_channel_row(&channel.channel_id)
        .await
        .expect("channel row")
        .expect("the channel created after the setting is retained");
    assert_eq!(row.title, "後から作ったスレッド");
    let timeline = k
        .list_timeline(TOPIC, None, 50)
        .await
        .expect("timeline")
        .items
        .into_iter()
        .map(|post| post.object_id)
        .collect::<Vec<_>>();
    assert!(timeline.contains(&topic_post), "{timeline:?}");
    assert!(
        !timeline.contains(&channel_post) && !timeline.contains(&first_post),
        "{timeline:?}"
    );
    // 進み具合は、索引ごとに読み終えた位置を返す。
    let status = k.topic_retention().await.expect("status");
    let retained = status
        .topics
        .iter()
        .find(|topic| topic.topic == TOPIC)
        .expect("retained topic");
    for index in ["indexes/timeline/", "channels/list/", "channels/activity/"] {
        assert!(
            retained
                .progress
                .iter()
                .any(|progress| progress.index == index),
            "{index}: {:?}",
            retained.progress
        );
    }

    // 公開スレッドを 10 倍に増やしても、lease は保持する topic の 2 つのまま(公開スレッドの lease を取らない)で、
    // 動いている task も lease の 2 つと取り込みの 1 つのまま。
    for index in 0..10 {
        a.create_public_channel(
            TOPIC,
            &format!("thread {index}"),
            "",
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("more public channels");
    }
    catch_up(&k, TOPIC).await;
    assert_eq!(lease_keys(&k).await.len(), 2);
    assert_eq!(
        k.subscription_registry
            .scope_leases
            .lock()
            .await
            .running_tasks(),
        2
    );

    // 解除すると、その topic の lease と続きの位置を外し、取り込みの対象から外す。
    k.set_topic_retention(OTHER_TOPIC, false)
        .await
        .expect("release the topic");
    assert_eq!(lease_keys(&k).await, vec![ScopeKey::Topic(TOPIC.into())]);
    assert_eq!(
        k.topic_retention()
            .await
            .expect("status")
            .topics
            .into_iter()
            .map(|topic| topic.topic)
            .collect::<Vec<_>>(),
        vec![TOPIC.to_string()]
    );
    assert!(
        TopicRetentionStore::topic_retention_cursors(k_store.as_ref(), OTHER_TOPIC, 0)
            .await
            .expect("cursors")
            .is_empty()
    );

    // 停止すると、取り込みの task と lease の task が終わる。
    k.shutdown().await;
    assert!(
        k.subscription_registry
            .topic_retention_task
            .lock()
            .await
            .is_none()
    );
    assert!(lease_keys(&k).await.is_empty());
}

// 起動時に保存した topic の lease を取り直して取り込みを再開し、保持する topic が無ければ task を起こさない。公開でない
// topic の名前は拒否して何も保存しない。
#[tokio::test]
async fn retained_topics_are_restored_at_start_and_private_names_are_rejected() {
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let (k, store) = retaining_device(
        &network,
        MemoryDocsSync::default(),
        MemoryDocsSync::default(),
        blobs.clone(),
    );
    k.start_topic_retention()
        .await
        .expect("start without topics");
    assert!(
        k.subscription_registry
            .topic_retention_task
            .lock()
            .await
            .is_none()
    );
    for topic in [
        "",
        "hint/kukuri:topic:x",
        "private/channel-1",
        "kukuri:dm:abc",
        "a b",
    ] {
        assert!(
            k.set_topic_retention(topic, true).await.is_err(),
            "{topic:?}"
        );
    }
    assert!(
        TopicRetentionStore::retained_topics(store.as_ref(), 10)
            .await
            .expect("topics")
            .is_empty()
    );
    k.set_topic_retention(TOPIC, true).await.expect("retain");
    k.shutdown().await;

    // 同じ保存先で作り直した端末は、保存した topic の lease を取り直して取り込みを始める。
    let transport = Arc::new(FakeTransport::new("k2", network.clone()));
    let restarted = AppService::from_handles(
        ServiceHandles::new(
            store.clone(),
            store.clone(),
            transport.clone(),
            transport,
            Arc::new(CountingDocsSync::default()),
            blobs,
            generate_keys(),
        )
        .with_topic_retention(store.clone(), std::env::temp_dir()),
    );
    restarted
        .start_topic_retention()
        .await
        .expect("restore retained topics");
    assert_eq!(
        lease_keys(&restarted).await,
        vec![ScopeKey::Topic(TOPIC.into())]
    );
    assert!(
        restarted
            .subscription_registry
            .topic_retention_task
            .lock()
            .await
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    );

    // 最後の topic を解除すると、取り込みの task は終わる。また設定すれば、新しい task で再開する。
    restarted
        .set_topic_retention(TOPIC, false)
        .await
        .expect("release the last topic");
    let task_running = || async {
        restarted
            .subscription_registry
            .topic_retention_task
            .lock()
            .await
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    };
    timeout(Duration::from_secs(10), async {
        while task_running().await {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the task ends without retained topics");
    restarted
        .set_topic_retention(TOPIC, true)
        .await
        .expect("retain again");
    assert!(task_running().await);
    restarted.shutdown().await;
}

// 読み先の索引に載っているのに本体を読めない投稿は、取り直しの予定に入れて先へ進む(2026-10-22 ユーザー判断)。後ろの投稿の
// 取り込みは止めない。
#[tokio::test]
async fn unreadable_records_wait_for_a_retry_without_blocking_later_posts() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let k_docs = MemoryDocsSync::with_docs_author("03".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), k_docs.clone(), blobs.clone());
    let (k, k_store) = retaining_device(&network, k_docs, a_docs.clone(), blobs);
    k.set_topic_retention(TOPIC, true)
        .await
        .expect("retain the topic");
    let now = Utc::now().timestamp();
    let missing = EnvelopeId::from("ab".repeat(32));
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: TOPIC.into(),
        },
        TimeBucket::from_unix_seconds(now).expect("bucket"),
    )
    .expect("replica")
    .replica_id();
    a_docs.open_replica(&replica).await.expect("open");
    a_docs
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: format!(
                    "indexes/timeline/{}/{}",
                    timeline_sort_key(now - 1, &missing),
                    missing.as_str()
                ),
                value: serde_json::json!({"object_id": missing, "created_at": now - 1}),
            },
        )
        .await
        .expect("an index entry without its envelope");
    let post = a
        .create_post(TOPIC, "後ろの投稿", None)
        .await
        .expect("later post");

    catch_up(&k, TOPIC).await;

    assert!(
        k_store
            .get_object_projection(&EnvelopeId::from(post.as_str()))
            .await
            .expect("projection")
            .is_some(),
        "the unreadable record does not block the later post"
    );
    let status = k.topic_retention().await.expect("status");
    assert_eq!(
        (status.topics[0].pending_retries, status.topics[0].given_up),
        (1, 0)
    );
    k.shutdown().await;
}

//! #1221 R5-H AC-4・AC-9: 実 Iroh の 2 client。旧 sync を使わず、hint の exact 読取りと lease の開始・再接続での
//! 1 ページの読み直しで新着を取り込み、通知を作る。受け手は書き手の bucket の namespace を取り込まない。

use super::*;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

struct Peer {
    node: Arc<kukuri_iroh_node::IrohDocsNode>,
    docs: Arc<kukuri_docs_sync::IrohDocsSync>,
    app: AppService,
    store: Arc<MemoryStore>,
}

impl Peer {
    async fn new(network: &FakeNetwork, name: &str) -> Result<Self> {
        let node = kukuri_iroh_node::IrohDocsNode::memory().await?;
        let docs = Arc::new(kukuri_docs_sync::IrohDocsSync::new(node.clone()));
        let store = Arc::new(MemoryStore::default());
        let transport = Arc::new(FakeTransport::new(name, network.clone()));
        let app = app_service_from_dependencies(
            store.clone(),
            store.clone(),
            transport.clone(),
            transport,
            docs.clone(),
            Arc::new(MemoryBlobService::default()),
            generate_keys(),
        );
        app.switch_writer(1);
        Ok(Self {
            node,
            docs,
            app,
            store,
        })
    }

    async fn learn(&self, other: &Peer) -> Result<()> {
        let socket = other
            .node
            .endpoint()
            .bound_sockets()
            .into_iter()
            .next()
            .expect("socket");
        self.docs
            .import_peer_ticket(&format!("{}@{socket}", other.node.endpoint().addr().id))
            .await
    }

    async fn projected(&self, object_id: &str) -> bool {
        timeout(Duration::from_secs(20), async {
            while ObjectProjectionStore::get_object_projection(
                self.store.as_ref(),
                &EnvelopeId::from(object_id),
            )
            .await
            .expect("projection")
            .is_none()
            {
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok()
    }

    async fn finish(self) -> Result<()> {
        self.app.shutdown().await;
        self.docs.shutdown().await;
        self.node.shutdown().await
    }
}

async fn hide_topic(app: &AppService, topic: &str) -> Result<()> {
    app.set_scope_display(crate::ScopeDisplayRequest {
        observer: format!("test-topic:{topic}"),
        target: crate::ScopeDisplayTarget::Timeline {
            topic: topic.into(),
            scope: TimelineScope::Public,
        },
        visible: false,
    })
    .await
}

#[tokio::test]
async fn real_iroh_hint_exact_read_and_bounded_rereads_receive_new_posts_without_sync() -> Result<()>
{
    let network = FakeNetwork::default();
    let writer = Peer::new(&network, "writer").await?;
    let reader = Peer::new(&network, "reader").await?;
    reader.learn(&writer).await?;
    let topic = "kukuri:topic:r5h-receive";
    display_topic(&reader.app, topic).await?;

    writer.learn(&reader).await?;
    display_topic(&writer.app, topic).await?;

    // 1. hint を受けた対象だけを exact に読む(双方向)。自分の投稿への返信から通知を作る。
    let root = reader.app.create_post(topic, "root", None).await?;
    assert!(writer.projected(&root).await, "the hinted root arrives");
    let first = writer.app.create_post(topic, "reply", Some(&root)).await?;
    assert!(reader.projected(&first).await, "the hinted reply arrives");
    let replies = || async {
        reader
            .app
            .list_notifications()
            .await
            .expect("notifications")
            .iter()
            .filter(|item| item.object_id.as_deref() == Some(first.as_str()))
            .count()
    };
    assert_eq!(replies().await, 1, "one reply notification");

    // 2. hint を落とした新着は、lease の再開始で現在の bucket を 1 ページ読み直して取り込む。
    hide_topic(&reader.app, topic).await?;
    let missed = writer.app.create_post(topic, "missed hint", None).await?;
    display_topic(&reader.app, topic).await?;
    assert!(reader.projected(&missed).await, "the lease start rereads");

    // 3. 再接続(endpoint の世代の変化)でも読み直す。
    hide_topic(&reader.app, topic).await?;
    let offline = writer.app.create_post(topic, "while offline", None).await?;
    display_topic(&reader.app, topic).await?;
    reader.app.rebuild_scope_subscriptions().await?;
    assert!(reader.projected(&offline).await, "the rebuild rereads");

    // 読み直しは通知を重ねない。受け手は書き手の bucket を取り込まない(旧 sync を始めない)。
    reader.app.rebuild_scope_subscriptions().await?;
    sleep(Duration::from_millis(300)).await;
    assert_eq!(replies().await, 1);
    let bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_unix_seconds(Utc::now().timestamp())?,
    )?
    .replica_id();
    assert!(
        reader
            .docs
            .query_replica_with_policy(
                &bucket,
                DocQuery::Exact(format!("objects/{first}/envelope")),
                DocFetchPolicy::LocalOnly,
            )
            .await?
            .is_empty(),
        "the reader did not import the writer's bucket"
    );
    reader.finish().await?;
    writer.finish().await
}

#[test]
fn the_day_boundary_reread_waits_until_the_next_bucket() {
    use crate::service::scope_receive::until_next_bucket;
    let day = 86_400;
    assert_eq!(until_next_bucket(day * 10).as_secs(), day as u64);
    assert_eq!(until_next_bucket(day * 11 - 1).as_secs(), 1);
    assert_eq!(until_next_bucket(day * 10 + 60).as_secs(), day as u64 - 60);
}

// 同じ task の 2 回目の読み直し(日の境界と同じ台帳の key)も、bucket の先頭を読み直す。1 回目が 1 ページで止めた読み残しの
// 位置を持ち越さない(#1221 R5-H)。
#[tokio::test]
async fn a_second_reread_reads_the_head_again() -> Result<()> {
    let network = FakeNetwork::default();
    let writer = Peer::new(&network, "writer").await?;
    let reader = Peer::new(&network, "reader").await?;
    reader.learn(&writer).await?;
    writer.learn(&reader).await?;
    let topic = "kukuri:topic:r5h-reread-head";
    let mut last = String::new();
    for index in 0..60 {
        last = writer
            .app
            .create_post(topic, &format!("bulk {index}"), None)
            .await?;
    }
    reader.app.reread_scope(topic, &TimelineScope::Public).await;
    assert!(
        reader.projected(&last).await,
        "the first reread reads the newest"
    );
    let late = writer
        .app
        .create_post(topic, "late without a hint", None)
        .await?;
    sleep(Duration::from_millis(31_000)).await;
    reader.app.reread_scope(topic, &TimelineScope::Public).await;
    assert!(
        reader.projected(&late).await,
        "the second reread reads the newest post"
    );
    reader.finish().await?;
    writer.finish().await
}

// 日の境界の読み直しは、直前の日の bucket も先頭から 1 ページ読む。前日に hint を落とした投稿(前日の時刻で署名し、前日の
// bucket に置いた投稿)を、lease の開始の読み直しの直後の境界の読み直しで取り込む(AC-4・AC-9)。
#[tokio::test]
async fn the_day_boundary_reread_takes_in_a_post_whose_hint_was_dropped_the_day_before()
-> Result<()> {
    let network = FakeNetwork::default();
    let writer = Peer::new(&network, "writer").await?;
    let reader = Peer::new(&network, "reader").await?;
    reader.learn(&writer).await?;
    writer.learn(&reader).await?;
    let topic = TopicId::new("kukuri:topic:r5h-day-boundary");
    let yesterday = TimeBucket::from_unix_seconds(Utc::now().timestamp() - 86_400)?;
    let day_start = yesterday.start_seconds() as i64;
    let bucket = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.as_str().into(),
        },
        yesterday,
    )?
    .replica_id();
    let mut last = None;
    for index in 0..60 {
        last = Some(
            super::range_reconcile::put_post_at(
                writer.docs.as_ref(),
                &bucket,
                writer.app.keys(),
                &topic,
                day_start + 100 + index,
                &format!("yesterday {index}"),
                None,
            )
            .await,
        );
    }
    let last = last.expect("posts");
    reader
        .app
        .reread_scope(topic.as_str(), &TimelineScope::Public)
        .await;
    assert!(reader.projected(last.envelope.id.as_str()).await);
    let late = super::range_reconcile::put_post_at(
        writer.docs.as_ref(),
        &bucket,
        writer.app.keys(),
        &topic,
        day_start + 200,
        "yesterday without a hint",
        None,
    )
    .await;
    reader
        .app
        .reread_scope(topic.as_str(), &TimelineScope::Public)
        .await;
    assert!(
        reader.projected(late.envelope.id.as_str()).await,
        "the boundary reread takes in the post from the day before"
    );
    reader.finish().await?;
    writer.finish().await
}

/// `keys` の旧 `author::` へ、`created_at` のプロフィールの行を 1 件置く(手元に残る更新前の行)。
async fn put_legacy_profile_row(docs_sync: &dyn DocsSync, keys: &KukuriKeys, created_at: i64) {
    let author = keys.public_key_hex();
    let envelope = build_profile_post_envelope(
        keys,
        &KukuriProfilePostEnvelopeContentV1 {
            author_pubkey: Pubkey::from(author.as_str()),
            profile_topic_id: author_profile_topic_id(author.as_str()),
            published_topic_id: TopicId::new("kukuri:topic:r5h-author-lease"),
            object_id: EnvelopeId::from(generate_keys().public_key_hex().as_str()),
            created_at,
            object_kind: "post".into(),
            content: format!("legacy {created_at}"),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        },
    )
    .expect("profile post envelope");
    let post = parse_profile_post(&envelope)
        .expect("parse")
        .expect("profile post");
    persist_profile_post_doc(
        docs_sync,
        &author_replica_id(author.as_str()),
        &post,
        &envelope,
    )
    .await
    .expect("persist legacy row");
}

async fn page_one(app: &AppService, author: &str) -> Vec<String> {
    app.list_profile_timeline(author, None, 2)
        .await
        .expect("profile")
        .items
        .into_iter()
        .map(|item| item.object_id)
        .collect()
}

// 手元の旧行が limit 件以上ある他人のプロフィールは、lease を始める前は remote を読まず、切替後の投稿は 1 ページ目に
// 出ない。profile を開いて author の lease が始まると、現在と直前の author bucket を 1 ページ読み直して手元へ置き、
// 1 ページ目に出る(#1221 R5-H)。
#[tokio::test]
async fn the_author_lease_reread_brings_another_authors_switched_posts_to_page_one() -> Result<()> {
    let network = FakeNetwork::default();
    let writer = Peer::new(&network, "writer").await?;
    let reader = Peer::new(&network, "reader").await?;
    reader.learn(&writer).await?;
    writer.learn(&reader).await?;
    let author = writer.app.current_author_pubkey();
    let now = Utc::now().timestamp();
    for offset in 1..=3 {
        put_legacy_profile_row(
            reader.docs.as_ref(),
            writer.app.keys(),
            now - 3_600 - offset,
        )
        .await;
    }
    let switched = writer
        .app
        .create_post("kukuri:topic:r5h-author-lease", "after the switch", None)
        .await?;
    assert_eq!(page_one(&reader.app, &author).await.len(), 2);
    assert!(
        !page_one(&reader.app, &author).await.contains(&switched),
        "without the lease the page does not read the provider"
    );
    display_author(&reader.app, author.as_str()).await?;
    let reached = timeout(Duration::from_secs(20), async {
        while !page_one(&reader.app, &author).await.contains(&switched) {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        reached.is_ok(),
        "the lease reread places the author bucket rows"
    );
    reader.finish().await?;
    writer.finish().await
}

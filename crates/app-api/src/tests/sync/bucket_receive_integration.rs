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

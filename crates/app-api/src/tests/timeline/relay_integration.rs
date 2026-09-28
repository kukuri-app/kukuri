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
        let app = app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            hints,
            docs.clone(),
            blobs,
            generate_keys(),
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

/// 条件が成り立つまで待つ(本文の取り直しは背景で進む)。
async fn eventually<F, Fut>(mut check: F) -> Result<bool>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
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

//! #1221 R5-C: author の現在値とプロフィールを、author 本人の端末から実 QUIC の有界な読取りで得る。
//! client は author の namespace へ sync せず、読んだ record を docs へ取り込まない。

use super::super::*;
use crate::service::profile_timeline_support::persist_profile_index_entry;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};
use kukuri_transport::EndpointAddr;

/// author 本人の端末を、account の宛先として解決する hint transport。gossip は何も届けない。
struct AuthorDestination(EndpointAddr);

#[async_trait]
impl HintTransport for AuthorDestination {
    async fn subscribe_hints(&self, _: &TopicId) -> Result<kukuri_transport::HintStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
    async fn unsubscribe_hints(&self, _: &TopicId) -> Result<()> {
        Ok(())
    }
    async fn publish_hint(&self, _: &TopicId, _: GossipHint) -> Result<()> {
        Ok(())
    }
    async fn resolve_receive_destination(&self, _: &Pubkey) -> Result<Option<EndpointAddr>> {
        Ok(Some(self.0.clone()))
    }
}

fn profile_post(
    keys: &KukuriKeys,
    created_at: i64,
    content: &str,
) -> (ProfilePost, KukuriEnvelope) {
    let author = keys.public_key_hex();
    let envelope = build_profile_post_envelope(
        keys,
        &KukuriProfilePostEnvelopeContentV1 {
            author_pubkey: Pubkey::from(author.as_str()),
            profile_topic_id: author_profile_topic_id(author.as_str()),
            published_topic_id: TopicId::new("kukuri:topic:author-reader"),
            object_id: EnvelopeId::from(generate_keys().public_key_hex().as_str()),
            created_at,
            object_kind: "post".into(),
            content: content.into(),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        },
    )
    .expect("profile post envelope");
    let post = parse_profile_post(&envelope)
        .expect("parse profile post")
        .expect("profile post");
    (post, envelope)
}

/// 新 writer の author bucket に、旧 replica のプロフィールと同じ key で投稿を書く(R5-H の writer の配置)。
async fn put_bucket_profile_post(
    docs: &dyn DocsSync,
    keys: &KukuriKeys,
    created_at: i64,
) -> String {
    let (post, envelope) = profile_post(keys, created_at, "bucket post");
    let replica = BucketReplica::new(
        BucketScope::Author {
            author_pubkey: keys.public_key_hex(),
        },
        TimeBucket::from_unix_seconds(created_at).expect("bucket"),
    )
    .expect("author bucket")
    .replica_id();
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: format!("profile/posts/{}", post.object_id.as_str()),
            value: serde_json::to_value(AuthorProfilePostDocV1 {
                author_pubkey: post.author_pubkey.clone(),
                profile_topic_id: post.profile_topic_id.clone(),
                published_topic_id: post.published_topic_id.clone(),
                object_id: post.object_id.clone(),
                created_at: post.created_at,
                object_kind: post.object_kind.clone(),
                content: post.content.clone(),
                attachments: post.attachments.clone(),
                reply_to_object_id: None,
                root_id: None,
                content_labels: Vec::new(),
                envelope_id: envelope.id.clone(),
            })
            .expect("doc"),
        },
    )
    .await
    .expect("bucket post doc");
    persist_profile_index_entry(docs, &replica, created_at, &post.object_id, "post")
        .await
        .expect("bucket index");
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: format!("envelopes/{}", envelope.id.as_str()),
            value: serde_json::to_value(&envelope).expect("envelope"),
        },
    )
    .await
    .expect("bucket envelope");
    post.object_id.as_str().to_string()
}

#[tokio::test]
async fn real_iroh_author_profile_is_read_from_the_author_device_without_sync() -> Result<()> {
    let author_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let client_node = kukuri_iroh_node::IrohDocsNode::memory().await?;
    let author_docs = kukuri_docs_sync::IrohDocsSync::new(author_node.clone());
    let client_docs = Arc::new(kukuri_docs_sync::IrohDocsSync::new(client_node.clone()));
    let author = generate_keys();
    let local = generate_keys();
    let author_pubkey = author.public_key_hex();

    let profile_envelope = build_profile_envelope_with_docs_author(
        &author,
        &KukuriProfileEnvelopeContentV1 {
            author_pubkey: author.public_key(),
            name: Some("author device".into()),
            display_name: None,
            about: None,
            picture_asset: None,
        },
        None,
    )?;
    let profile = parse_profile(&profile_envelope)?.expect("profile");
    persist_profile_doc(&author_docs, &profile, &profile_envelope).await?;
    let follow = build_follow_edge_envelope(
        &author,
        &Pubkey::from(local.public_key_hex()),
        FollowEdgeStatus::Active,
    )?;
    persist_follow_edge_doc(
        &author_docs,
        &parse_follow_edge(&follow)?.expect("follow"),
        &follow,
    )
    .await?;
    let now = Utc::now().timestamp();
    let mut expected = Vec::new();
    for index in 0..25 {
        let (post, envelope) = profile_post(&author, now - 3_600 - index, "legacy post");
        persist_profile_post_doc(&author_docs, &post, &envelope).await?;
        expected.push(post.object_id.as_str().to_string());
    }
    expected.insert(0, put_bucket_profile_post(&author_docs, &author, now).await);

    let socket = author_node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    let destination = EndpointAddr::new(author_node.endpoint().addr().id).with_ip_addr(socket);
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(AuthorDestination(destination)),
        client_docs.clone(),
        Arc::new(MemoryBlobService::default()),
        local.clone(),
    );

    let first = app
        .list_profile_timeline(author_pubkey.as_str(), None, 20)
        .await?;
    let mut seen = first
        .items
        .iter()
        .map(|item| item.object_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(seen.len(), 20);
    let second = app
        .list_profile_timeline(author_pubkey.as_str(), first.next_cursor, 20)
        .await?;
    seen.extend(second.items.iter().map(|item| item.object_id.clone()));
    assert_eq!(
        seen, expected,
        "the bucket post first, then every legacy post"
    );

    let relationship = timeout(Duration::from_secs(20), async {
        loop {
            if let Some(relationship) = app
                .services
                .projection_store
                .get_author_relationship(local.public_key_hex().as_str(), author_pubkey.as_str())
                .await?
                .filter(|relationship| relationship.followed_by)
            {
                return Ok::<_, anyhow::Error>(relationship);
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await??;
    assert!(relationship.followed_by);
    assert_eq!(
        store
            .get_profile(author_pubkey.as_str())
            .await?
            .and_then(|profile| profile.name)
            .as_deref(),
        Some("author device")
    );
    assert!(
        client_docs
            .query_local_source(
                &author_replica_id(author_pubkey.as_str()),
                "profile/latest",
                None,
                1
            )
            .await?
            .is_empty(),
        "the author's records are not imported into the client's docs"
    );

    app.shutdown().await;
    client_docs.shutdown().await;
    author_docs.shutdown().await;
    client_node.shutdown().await?;
    author_node.shutdown().await
}

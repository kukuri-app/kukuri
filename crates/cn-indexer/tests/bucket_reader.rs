use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use kukuri_cn_core::{
    ChannelSecretCipher, IndexScopeKind, MemoryIndexEntryStore, TestDatabase, add_supported_topic,
    approve_indexing_request, connect_postgres, initialize_database, insert_indexing_request,
    mark_index_demand, register_channel_secret_with_epoch, remove_channel_secret,
    rotate_channel_secret_epoch,
};
use kukuri_cn_indexer::bucket_reader::BucketReader;
use kukuri_cn_indexer::ingest::IngestPipeline;
use kukuri_cn_indexer::maintenance::IndexMaintenance;
use kukuri_cn_indexer::projection::{IndexProjection, MemoryIndexProjection};
use kukuri_cn_indexer::query::{FailClosedIndexQuery, IndexQuery};
use kukuri_cn_indexer::state::IndexerRuntimeState;
use kukuri_cn_indexer::worker::{IndexerWorker, WorkerConfig};
use kukuri_core::{
    ChannelId, KukuriKeys, ObjectVisibility, PayloadRef, TopicId,
    build_post_envelope_with_payload_in_channel, timeline_sort_key,
};
use kukuri_docs_sync::{
    BucketReplica, BucketScope, DocOp, DocsSync, IrohDocsSync, TimeBucket, stable_key,
};
use kukuri_iroh_node::IrohDocsNode;

#[path = "ingest_support/mod.rs"]
mod ingest_support;

async fn publish(
    docs: &IrohDocsSync,
    replica: &kukuri_core::ReplicaId,
    topic: &TopicId,
    body: &str,
    channel: Option<&str>,
) -> Result<String> {
    let channel_id = channel.map(ChannelId::new);
    let envelope = build_post_envelope_with_payload_in_channel(
        &KukuriKeys::generate(),
        topic,
        PayloadRef::InlineText { text: body.into() },
        Vec::new(),
        Vec::new(),
        None,
        if channel.is_some() {
            ObjectVisibility::Private
        } else {
            ObjectVisibility::Public
        },
        channel_id.as_ref(),
        Vec::new(),
    )?;
    let post = envelope.to_post_object()?.expect("post");
    let id = post.object_id.as_str().to_string();
    let sort_key = timeline_sort_key(post.created_at, &post.object_id);
    for (key, value) in [
        (
            stable_key("objects", &format!("{id}/state")),
            serde_json::to_value(&post)?,
        ),
        (
            stable_key("objects", &format!("{id}/envelope")),
            serde_json::to_value(&envelope)?,
        ),
        (
            stable_key("indexes/timeline", &format!("{sort_key}/{id}")),
            serde_json::json!({"object_id": id}),
        ),
    ] {
        docs.apply_doc_op(replica, DocOp::SetJson { key, value })
            .await?;
    }
    Ok(id)
}

/// 端末を CN の bootstrap 登録へ載せる（bucket reader が提供元として選ぶ）。
async fn register_provider(pool: &sqlx::PgPool, pubkey: &str, node: &IrohDocsNode) -> Result<()> {
    sqlx::query("INSERT INTO cn_user.subscriber_accounts(subscriber_pubkey) VALUES ($1)")
        .bind(pubkey)
        .execute(pool)
        .await?;
    let socket = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    sqlx::query(
        "INSERT INTO cn_bootstrap.peer_registrations(subscriber_pubkey, endpoint_id, addr_hint, expires_at)
         VALUES ($1, $2, $3, NOW() + INTERVAL '1 hour')",
    )
    .bind(pubkey)
    .bind(node.endpoint().addr().id.to_string())
    .bind(socket.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

fn topic_bucket(topic: &str, now: i64) -> Result<kukuri_core::ReplicaId> {
    Ok(BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_unix_seconds(now)?,
    )?
    .replica_id())
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_feed_one_cn_through_bounded_bucket_reader() -> Result<()> {
    let Some(admin) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        return Ok(());
    };
    let database = TestDatabase::create(&admin, "cn_remote_bucket_reader").await?;
    let pool = connect_postgres(&database.database_url).await?;
    initialize_database(&pool).await?;
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    mark_index_demand(&pool, IndexScopeKind::PublicTopic, "rust").await?;

    let node_a = IrohDocsNode::memory().await?;
    let node_b = IrohDocsNode::memory().await?;
    let cn_node = IrohDocsNode::memory().await?;
    let docs_a = Arc::new(IrohDocsSync::new(node_a.clone()));
    let docs_b = Arc::new(IrohDocsSync::new(node_b.clone()));
    let cn_docs = Arc::new(IrohDocsSync::new(cn_node.clone()));
    let now = chrono::Utc::now().timestamp();
    let replica = topic_bucket("rust", now)?;
    let topic = TopicId::new("rust");
    let a = publish(&docs_a, &replica, &topic, "from client a", None).await?;
    let b = publish(&docs_b, &replica, &topic, "from client b", None).await?;
    register_provider(&pool, &"a".repeat(64), &node_a).await?;
    register_provider(&pool, &"b".repeat(64), &node_b).await?;

    let (safety, artifacts) = ingest_support::allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifacts));
    let projection = Arc::new(MemoryIndexProjection::default());
    let pipeline = IngestPipeline::new(
        cn_docs.clone(),
        safety.clone(),
        entries.clone(),
        projection.clone(),
    );
    let cipher =
        ChannelSecretCipher::from_key_material("public-bucket-test-cipher-key-0123456789")?;
    let reader = Arc::new(BucketReader::new(
        pool.clone(),
        cn_docs.clone(),
        entries.clone(),
        pipeline,
        cipher.clone(),
    ));
    assert_eq!(reader.poll_once(now, true).await?.0.indexed, 2);
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &a)
            .await?
    );
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &b)
            .await?
    );
    // 日の境界（時刻の注入）: 翌日の巡回は直前の bucket として同じ投稿を読み直すが、索引は 1 件のまま。
    // 2 日後の巡回は、その bucket を読まない（休止した過去 bucket を読みに行かない）。
    let late = publish(&docs_a, &replica, &topic, "late in the day", None).await?;
    assert_eq!(reader.poll_once(now + 2 * 86_400, true).await?.0.indexed, 0);
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &late)
            .await?
    );
    reader.poll_once(now + 86_400, true).await?;
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &late)
            .await?
    );
    for id in [&a, &b, &late] {
        assert_eq!(
            entries
                .entries_snapshot()
                .iter()
                .filter(|entry| &entry.object_id == id)
                .count(),
            1,
            "読み直しで二重に反映しない"
        );
    }
    let mut local_namespaces = cn_node.docs().list().await?;
    assert!(
        local_namespaces.next().await.is_none(),
        "bounded remote read must not import or sync the bucket namespace"
    );

    let worker = IndexerWorker::new(
        reader,
        IndexMaintenance::new(pool.clone(), entries, projection.clone(), cipher),
        Arc::new(IndexerRuntimeState::default()),
        WorkerConfig {
            poll_interval: Duration::from_millis(250),
            demand_interval: Duration::from_millis(250),
        },
    )
    .spawn();
    let after_start = publish(&docs_a, &replica, &topic, "after worker start", None).await?;
    let mut indexed = false;
    for _ in 0..50 {
        if projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &after_start)
            .await?
        {
            indexed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        indexed,
        "owned background reader must index a new bucket post"
    );
    worker.shutdown().await;
    let after_stop = publish(&docs_a, &replica, &topic, "after worker stop", None).await?;
    tokio::time::sleep(Duration::from_millis(750)).await;
    assert!(
        !projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &after_stop)
            .await?,
        "shutdown must stop the background reader"
    );

    cn_docs.shutdown().await;
    docs_a.shutdown().await;
    docs_b.shutdown().await;
    cn_node.shutdown().await?;
    node_a.shutdown().await?;
    node_b.shutdown().await?;
    pool.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn registered_private_epoch_reads_only_the_disclosing_provider() -> Result<()> {
    let Some(admin) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        return Ok(());
    };
    let database = TestDatabase::create(&admin, "cn_private_bucket_reader").await?;
    let pool = connect_postgres(&database.database_url).await?;
    initialize_database(&pool).await?;
    let cipher =
        ChannelSecretCipher::from_key_material("private-bucket-test-cipher-key-0123456789")?;
    let epoch_secret = [7u8; 32];
    register_channel_secret_with_epoch(
        &pool,
        &cipher,
        "private-room",
        "epoch-1",
        &hex::encode(epoch_secret),
    )
    .await?;
    let subscriber = "a".repeat(64);
    sqlx::query("INSERT INTO cn_user.subscriber_accounts(subscriber_pubkey) VALUES ($1)")
        .bind(&subscriber)
        .execute(&pool)
        .await?;
    let request = insert_indexing_request(
        &pool,
        &subscriber,
        IndexScopeKind::PrivateChannel,
        "private-room",
    )
    .await?;
    approve_indexing_request(&pool, &request.id).await?;
    mark_index_demand(&pool, IndexScopeKind::PrivateChannel, "private-room").await?;

    let provider = IrohDocsNode::memory().await?;
    let unrelated = IrohDocsNode::memory().await?;
    let cn_node = IrohDocsNode::memory().await?;
    let docs_provider = Arc::new(IrohDocsSync::new(provider.clone()));
    let docs_unrelated = Arc::new(IrohDocsSync::new(unrelated.clone()));
    let cn_docs = Arc::new(IrohDocsSync::new(cn_node.clone()));
    let now = chrono::Utc::now().timestamp();
    let bucket = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: "private-room".into(),
            epoch_id: "epoch-1".into(),
        },
        TimeBucket::from_unix_seconds(now)?,
    )?;
    let replica = bucket.replica_id();
    let derived = hex::encode(bucket.derive_private_secret(&epoch_secret)?);
    for docs in [&docs_provider, &docs_unrelated] {
        docs.register_private_replica_secret(&replica, &derived)
            .await?;
    }
    let topic = TopicId::new("private-topic");
    let wanted = publish(
        &docs_provider,
        &replica,
        &topic,
        "authorized source",
        Some("private-room"),
    )
    .await?;
    let excluded = publish(
        &docs_unrelated,
        &replica,
        &topic,
        "unregistered source",
        Some("private-room"),
    )
    .await?;
    let provider_socket = provider
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("provider socket");
    sqlx::query(
        "INSERT INTO cn_bootstrap.peer_registrations(subscriber_pubkey, endpoint_id, addr_hint)
         VALUES ($1, $2, $3)",
    )
    .bind(&subscriber)
    .bind(provider.endpoint().addr().id.to_string())
    .bind(provider_socket.to_string())
    .execute(&pool)
    .await?;

    let (safety, artifacts) = ingest_support::allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifacts));
    let projection = Arc::new(MemoryIndexProjection::default());
    let pipeline =
        IngestPipeline::new(cn_docs.clone(), safety, entries.clone(), projection.clone());
    let reader = BucketReader::new(
        pool.clone(),
        cn_docs.clone(),
        entries.clone(),
        pipeline,
        cipher.clone(),
    );
    assert_eq!(reader.poll_once(now, true).await?.0.indexed, 1);
    assert!(
        projection
            .contains_object(IndexScopeKind::PrivateChannel, "private-room", &wanted)
            .await?
    );
    assert!(
        !projection
            .contains_object(IndexScopeKind::PrivateChannel, "private-room", &excluded)
            .await?
    );
    assert_eq!(
        entries
            .entries_snapshot()
            .into_iter()
            .find(|entry| entry.object_id == wanted)
            .expect("indexed private entry")
            .source_replica_id,
        replica.as_str(),
    );
    let query = FailClosedIndexQuery::new(projection.clone(), entries.clone());
    let hits = query
        .search_scope(
            IndexScopeKind::PrivateChannel,
            "private-room",
            "authorized",
            10,
        )
        .await?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source_replica_id, replica.as_str());
    assert!(cn_node.docs().list().await?.next().await.is_none());

    // epoch の境界: 回転後は新 epoch の bucket を読み、旧 epoch の投稿を二重に取り込まない。
    let next_secret = [8u8; 32];
    rotate_channel_secret_epoch(
        &pool,
        &cipher,
        &subscriber,
        "private-room",
        ("epoch-1", &hex::encode(epoch_secret)),
        ("epoch-2", &hex::encode(next_secret)),
    )
    .await?;
    let next_bucket = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: "private-room".into(),
            epoch_id: "epoch-2".into(),
        },
        TimeBucket::from_unix_seconds(now)?,
    )?;
    let replica = next_bucket.replica_id();
    docs_provider
        .register_private_replica_secret(
            &replica,
            &hex::encode(next_bucket.derive_private_secret(&next_secret)?),
        )
        .await?;
    let after_rotation = publish(
        &docs_provider,
        &replica,
        &topic,
        "after rotation",
        Some("private-room"),
    )
    .await?;
    assert_eq!(reader.poll_once(now, true).await?.0.indexed, 1);
    assert!(
        projection
            .contains_object(
                IndexScopeKind::PrivateChannel,
                "private-room",
                &after_rotation
            )
            .await?
    );
    assert_eq!(
        entries
            .entries_snapshot()
            .iter()
            .filter(|entry| entry.object_id == wanted)
            .count(),
        1
    );
    sqlx::query(
        "UPDATE cn_user.subscriber_accounts SET status = 'inactive' WHERE subscriber_pubkey = $1",
    )
    .bind(&subscriber)
    .execute(&pool)
    .await?;
    let after_consent_loss = publish(
        &docs_provider,
        &replica,
        &topic,
        "after consent loss",
        Some("private-room"),
    )
    .await?;
    assert_eq!(reader.poll_once(now, true).await?.0.indexed, 0);
    assert!(
        !projection
            .contains_object(
                IndexScopeKind::PrivateChannel,
                "private-room",
                &after_consent_loss
            )
            .await?
    );
    sqlx::query(
        "UPDATE cn_user.subscriber_accounts SET status = 'active' WHERE subscriber_pubkey = $1",
    )
    .bind(&subscriber)
    .execute(&pool)
    .await?;
    remove_channel_secret(&pool, "private-room").await?;
    let after_revoke = publish(
        &docs_provider,
        &replica,
        &topic,
        "after revoke",
        Some("private-room"),
    )
    .await?;
    assert_eq!(reader.poll_once(now, true).await?.0.indexed, 0);
    assert!(
        !projection
            .contains_object(
                IndexScopeKind::PrivateChannel,
                "private-room",
                &after_revoke
            )
            .await?
    );

    cn_docs.shutdown().await;
    docs_provider.shutdown().await;
    docs_unrelated.shutdown().await;
    cn_node.shutdown().await?;
    provider.shutdown().await?;
    unrelated.shutdown().await?;
    pool.close().await;
    database.cleanup().await
}

/// 最近の検索・表示の需要がある scope は `demand_interval`（既定 30 秒）で、それ以外は `poll_interval` の巡回で
/// 索引される（#1221 R5-H）。変更通知の queue は無く、どちらも bucket reader の巡回だけで取り込む。
#[tokio::test(flavor = "multi_thread")]
async fn demanded_scope_is_read_each_demand_interval_and_others_each_poll() -> Result<()> {
    assert_eq!(
        WorkerConfig::default().demand_interval,
        Duration::from_secs(30)
    );
    let Some(admin) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        return Ok(());
    };
    let database = TestDatabase::create(&admin, "cn_bucket_demand_interval").await?;
    let pool = connect_postgres(&database.database_url).await?;
    initialize_database(&pool).await?;
    for topic in ["hot", "cold"] {
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, topic).await?;
    }
    // topic の追加も需要を登録する（#1221 R5-E）。ここでは検索・表示の需要だけを置く。
    sqlx::query("UPDATE cn_index.supported_topics SET last_index_demand_at = NULL")
        .execute(&pool)
        .await?;
    mark_index_demand(&pool, IndexScopeKind::PublicTopic, "hot").await?;

    let author = IrohDocsNode::memory().await?;
    let cn_node = IrohDocsNode::memory().await?;
    let author_docs = Arc::new(IrohDocsSync::new(author.clone()));
    let cn_docs = Arc::new(IrohDocsSync::new(cn_node.clone()));
    register_provider(&pool, &"a".repeat(64), &author).await?;
    let (safety, artifacts) = ingest_support::allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifacts));
    let projection = Arc::new(MemoryIndexProjection::default());
    let pipeline =
        IngestPipeline::new(cn_docs.clone(), safety, entries.clone(), projection.clone());
    let cipher =
        ChannelSecretCipher::from_key_material("demand-bucket-test-cipher-key-0123456789")?;
    let reader = BucketReader::new(
        pool.clone(),
        cn_docs.clone(),
        entries.clone(),
        pipeline,
        cipher.clone(),
    );
    let state = Arc::new(IndexerRuntimeState::default());
    let worker = IndexerWorker::new(
        Arc::new(reader),
        IndexMaintenance::new(pool.clone(), entries, projection.clone(), cipher),
        state.clone(),
        WorkerConfig {
            poll_interval: Duration::from_secs(12),
            demand_interval: Duration::from_millis(200),
        },
    )
    .spawn();
    let result = async {
        // 最初の全 scope の巡回が済んでから書く。次の全 scope の巡回は 12 秒後。
        tokio::time::timeout(Duration::from_secs(30), async {
            while state.snapshot().last_sync_at.is_none() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        let now = chrono::Utc::now().timestamp();
        let hot = publish(
            &author_docs,
            &topic_bucket("hot", now)?,
            &TopicId::new("hot"),
            "demanded",
            None,
        )
        .await?;
        let cold = publish(
            &author_docs,
            &topic_bucket("cold", now)?,
            &TopicId::new("cold"),
            "not demanded",
            None,
        )
        .await?;
        let contains = |topic: &'static str, id: String| {
            let projection = projection.clone();
            async move {
                projection
                    .contains_object(IndexScopeKind::PublicTopic, topic, &id)
                    .await
            }
        };
        let hot_indexed = tokio::time::timeout(Duration::from_secs(4), async {
            while !contains("hot", hot.clone()).await? {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            anyhow::Ok(())
        })
        .await;
        assert!(
            hot_indexed.is_ok(),
            "需要のある scope は需要の周期で索引される"
        );
        assert!(
            !contains("cold", cold.clone()).await?,
            "需要の無い scope は需要の周期では読まない"
        );
        let cold_indexed = tokio::time::timeout(Duration::from_secs(30), async {
            while !contains("cold", cold.clone()).await? {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            anyhow::Ok(())
        })
        .await;
        assert!(
            cold_indexed.is_ok(),
            "需要の無い scope も全 scope の巡回で索引される"
        );
        anyhow::Ok(())
    }
    .await;
    worker.shutdown().await;
    cn_docs.shutdown().await;
    author_docs.shutdown().await;
    cn_node.shutdown().await?;
    author.shutdown().await?;
    pool.close().await;
    database.cleanup().await?;
    result
}

/// #1221 R5-H 判断 6: 旧 sync の peer だった運用者指定の seed(`COMMUNITY_NODE_INDEXER_SEED_PEERS`。bootstrap 登録の
/// 無い端末)も、公開 bucket の提供元の候補になる。
#[tokio::test(flavor = "multi_thread")]
async fn configured_seed_peer_is_a_public_bucket_provider() -> Result<()> {
    let Some(admin) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        return Ok(());
    };
    let database = TestDatabase::create(&admin, "cn_bucket_seed_provider").await?;
    let pool = connect_postgres(&database.database_url).await?;
    initialize_database(&pool).await?;
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    let seed = IrohDocsNode::memory().await?;
    let cn_node = IrohDocsNode::memory().await?;
    let seed_docs = Arc::new(IrohDocsSync::new(seed.clone()));
    let cn_docs = Arc::new(IrohDocsSync::new(cn_node.clone()));
    let now = chrono::Utc::now().timestamp();
    let id = publish(
        &seed_docs,
        &topic_bucket("rust", now)?,
        &TopicId::new("rust"),
        "from the configured seed",
        None,
    )
    .await?;
    let (safety, artifacts) = ingest_support::allow_service();
    let entries = Arc::new(MemoryIndexEntryStore::new(artifacts));
    let projection = Arc::new(MemoryIndexProjection::default());
    let pipeline =
        IngestPipeline::new(cn_docs.clone(), safety, entries.clone(), projection.clone());
    let cipher = ChannelSecretCipher::from_key_material("seed-bucket-test-cipher-key-0123456789")?;
    let socket = seed
        .endpoint()
        .bound_sockets()
        .into_iter()
        .next()
        .expect("socket");
    let reader = BucketReader::new(pool.clone(), cn_docs, entries, pipeline, cipher)
        .with_blob_seeds(
            Arc::new(kukuri_blob_service::IrohBlobService::new(cn_node.clone())),
            vec![kukuri_transport::SeedPeer {
                endpoint_id: seed.endpoint().addr().id.to_string(),
                addr_hint: Some(socket.to_string()),
            }],
        );
    assert_eq!(reader.poll_once(now, true).await?.0.indexed, 1);
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "rust", &id)
            .await?
    );
    seed.shutdown().await?;
    cn_node.shutdown().await?;
    pool.close().await;
    database.cleanup().await
}

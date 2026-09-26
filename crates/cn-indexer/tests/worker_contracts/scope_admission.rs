use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FailingPrivateRegistrationDocs {
    inner: Arc<MemoryDocsSync>,
    open: std::sync::Mutex<std::collections::HashSet<String>>,
    max_open: AtomicUsize,
}

#[async_trait]
impl DocsSync for FailingPrivateRegistrationDocs {
    async fn open_replica(&self, replica: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica).await?;
        let mut open = self.open.lock().expect("open set poisoned");
        open.insert(replica.as_str().to_string());
        self.max_open.fetch_max(open.len(), Ordering::SeqCst);
        Ok(())
    }

    async fn close_replica(&self, replica: &ReplicaId) -> Result<()> {
        self.inner.close_replica(replica).await?;
        self.open
            .lock()
            .expect("open set poisoned")
            .remove(replica.as_str());
        Ok(())
    }

    async fn register_private_replica_secret(&self, _: &ReplicaId, _: &str) -> Result<()> {
        anyhow::bail!("injected private registration failure")
    }

    async fn apply_doc_op(&self, replica: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.inner
            .query_replica_with_policy(replica, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica, query).await
    }

    async fn subscribe_replica(
        &self,
        replica: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica).await
    }

    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn worker_admits_at_most_32_legacy_scopes_from_a_larger_supported_set() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_legacy_scope_admission").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    for index in 0..80 {
        add_supported_topic(
            &pool,
            IndexScopeKind::PublicTopic,
            &format!("topic-{index:02}"),
        )
        .await?;
    }
    add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?;
    register_channel_secret(&pool, &cipher(), "secret-room", TEST_NAMESPACE_SECRET).await?;
    add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "no-secret").await?;
    clear_added_demand(&pool).await?;
    let docs = Arc::new(MemoryDocsSync::default());
    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, _) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let selector = participant.clone();
    let handle = IndexerWorker::new(
        participant,
        docs,
        state.clone(),
        fast_config(Duration::from_secs(120)),
    )
    .spawn();
    wait_until("first scope-admission pass", || {
        let state = state.clone();
        async move { state.snapshot().last_pass_duration_ms.is_some() }
    })
    .await;
    assert!(
        state.snapshot().opened_scopes <= 32,
        "legacy worker must leave room for the 32-scope public bucket reader"
    );
    mark_index_demand(&pool, IndexScopeKind::PublicTopic, "topic-79").await?;
    mark_index_demand(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?;
    mark_index_demand(&pool, IndexScopeKind::PrivateChannel, "no-secret").await?;
    let mut visited = std::collections::HashSet::new();
    for _ in 0..3 {
        let selected = selector
            .selected_scopes_at(chrono::Utc::now().timestamp())
            .await?;
        assert!(selected.len() <= 32);
        assert!(selected.iter().any(|scope| scope.id == "topic-79"));
        assert!(selected.iter().any(|scope| scope.id == "secret-room"));
        assert!(!selected.iter().any(|scope| scope.id == "no-secret"));
        visited.extend(selected.into_iter().map(|scope| scope.id));
    }
    assert_eq!(
        visited.len(),
        81,
        "fair cursor must reach all eligible scopes"
    );
    handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn rotating_out_a_supported_scope_keeps_its_indexed_post() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database =
        TestDatabase::create(admin_url.as_str(), "cn_scope_rotation_keeps_index").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    for index in 0..80 {
        add_supported_topic(
            &pool,
            IndexScopeKind::PublicTopic,
            &format!("topic-{index:02}"),
        )
        .await?;
    }
    clear_added_demand(&pool).await?;
    let docs = Arc::new(MemoryDocsSync::default());
    let post = persist_post(
        docs.as_ref(),
        &kukuri_docs_sync::topic_replica_id("topic-00"),
        &TopicId::new("topic-00"),
        "still indexed after rotation",
    )
    .await;
    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, entries) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let handle = IndexerWorker::new(
        participant,
        docs,
        state.clone(),
        fast_config(Duration::from_secs(1)),
    )
    .spawn();
    wait_until("initial topic-00 index", || {
        let projection = projection.clone();
        let post = post.clone();
        async move {
            projection
                .contains_object(IndexScopeKind::PublicTopic, "topic-00", &post)
                .await
                .unwrap_or(false)
        }
    })
    .await;
    wait_until("topic-00 rotated out", || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT last_scope_id FROM cn_index.legacy_scope_cursor WHERE id = TRUE",
            )
            .fetch_one(&pool)
            .await
            .is_ok_and(|cursor| cursor == "topic-63")
        }
    })
    .await;
    assert!(entries.contains(IndexScopeKind::PublicTopic, "topic-00", &post));
    assert!(
        projection
            .contains_object(IndexScopeKind::PublicTopic, "topic-00", &post)
            .await?
    );
    assert!(state.snapshot().opened_scopes <= 32);
    handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn partial_open_error_keeps_reservations_within_the_legacy_limit() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_partial_open_reservation").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    for index in 0..80 {
        add_supported_topic(
            &pool,
            IndexScopeKind::PublicTopic,
            &format!("topic-{index:02}"),
        )
        .await?;
    }
    add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?;
    register_channel_secret(&pool, &cipher(), "secret-room", TEST_NAMESPACE_SECRET).await?;
    clear_added_demand(&pool).await?;
    mark_index_demand(&pool, IndexScopeKind::PublicTopic, "topic-00").await?;
    let docs = Arc::new(FailingPrivateRegistrationDocs {
        inner: Arc::new(MemoryDocsSync::default()),
        open: std::sync::Mutex::new(std::collections::HashSet::new()),
        max_open: AtomicUsize::new(0),
    });
    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, _) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let handle = IndexerWorker::new(
        participant,
        docs.clone(),
        state.clone(),
        fast_config(Duration::from_secs(1)),
    )
    .spawn();
    wait_until("failed initial pass", || {
        let state = state.clone();
        async move { state.snapshot().last_pass_duration_ms.is_some() }
    })
    .await;
    assert_eq!(docs.max_open.load(Ordering::SeqCst), 1);
    sqlx::query(
        "UPDATE cn_index.supported_topics SET last_index_demand_at = NULL
         WHERE kind = 'public_topic' AND id = 'topic-00'",
    )
    .execute(&pool)
    .await?;
    wait_until("next fair pass", || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT last_scope_id FROM cn_index.legacy_scope_cursor WHERE id = TRUE",
            )
            .fetch_one(&pool)
            .await
            .is_ok_and(|cursor| cursor == "topic-62")
        }
    })
    .await;
    assert!(docs.max_open.load(Ordering::SeqCst) <= 32);
    assert!(state.snapshot().opened_scopes <= 32);
    handle.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn unsupported_indexed_scopes_are_deindexed_in_bounded_pages() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_indexed_scope_cursor").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    let docs = Arc::new(MemoryDocsSync::default());
    let state = Arc::new(IndexerRuntimeState::default());
    let projection = Arc::new(MemoryIndexProjection::default());
    let (participant, entries) = participant_with_docs(&pool, docs.clone(), &projection, &state);
    let mut posts = Vec::new();
    for index in 0..80 {
        let topic = format!("topic-{index:02}");
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, &topic).await?;
        let replica = kukuri_docs_sync::topic_replica_id(&topic);
        let post = persist_post(docs.as_ref(), &replica, &TopicId::new(&topic), "indexed").await;
        posts.push((topic.clone(), post));
        participant
            .ingest_recent_scope(&ScopeReplica::from_scope(
                IndexScopeKind::PublicTopic,
                &topic,
            ))
            .await?;
    }
    assert!(posts.iter().all(|(topic, post)| entries.contains(
        IndexScopeKind::PublicTopic,
        topic,
        post
    )));
    let posts = Arc::new(posts);
    for index in 1..80 {
        remove_supported_topic(
            &pool,
            IndexScopeKind::PublicTopic,
            &format!("topic-{index:02}"),
        )
        .await?;
    }
    let handle = IndexerWorker::new(
        participant.clone(),
        docs.clone(),
        state.clone(),
        fast_config(Duration::from_secs(120)),
    )
    .spawn();
    wait_until("first removal pass", || {
        let state = state.clone();
        async move { state.snapshot().last_pass_duration_ms.is_some() }
    })
    .await;
    assert!(
        state.snapshot().deindexed <= 32,
        "one pass must not load and de-index every historical scope"
    );
    let first_cursor: String = sqlx::query_scalar(
        "SELECT last_scope_id FROM cn_index.indexed_scope_cursor WHERE id = TRUE",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(first_cursor, "topic-31");
    handle.shutdown().await;

    let resumed = Arc::new(IndexerRuntimeState::default());
    let resumed_handle = IndexerWorker::new(
        participant,
        docs,
        resumed.clone(),
        fast_config(Duration::from_millis(250)),
    )
    .spawn();
    wait_until("first resumed removal pass", || {
        let resumed = resumed.clone();
        async move { resumed.snapshot().last_pass_duration_ms.is_some() }
    })
    .await;
    let resumed_cursor: String = sqlx::query_scalar(
        "SELECT last_scope_id FROM cn_index.indexed_scope_cursor WHERE id = TRUE",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(resumed_cursor, "topic-63");
    wait_until("remaining removals", || {
        let entries = entries.clone();
        let posts = posts.clone();
        async move {
            posts
                .iter()
                .skip(1)
                .all(|(topic, post)| !entries.contains(IndexScopeKind::PublicTopic, topic, post))
        }
    })
    .await;
    assert!(entries.contains(IndexScopeKind::PublicTopic, "topic-00", &posts[0].1));
    resumed_handle.shutdown().await;
    Ok(())
}

/// topic の追加は需要を登録する（#1221 R5-E）。公平な巡回だけを確かめる test では、追加による需要を外す。
async fn clear_added_demand(pool: &sqlx::PgPool) -> Result<()> {
    sqlx::query("UPDATE cn_index.supported_topics SET last_index_demand_at = NULL")
        .execute(pool)
        .await?;
    Ok(())
}

use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct ObservedDocs {
    inner: MemoryDocsSync,
    opened: Mutex<Vec<kukuri_core::ReplicaId>>,
    reads: Mutex<Vec<(kukuri_core::ReplicaId, usize)>>,
    close_attempts: Mutex<std::collections::HashMap<String, usize>>,
    closed: Mutex<Vec<kukuri_core::ReplicaId>>,
    fail_first_close: bool,
}

#[async_trait::async_trait]
impl DocsSync for ObservedDocs {
    async fn close_replica(&self, replica: &kukuri_core::ReplicaId) -> Result<()> {
        {
            let mut attempts = self.close_attempts.lock().expect("close attempts mutex");
            let count = attempts.entry(replica.as_str().to_string()).or_default();
            *count += 1;
            if self.fail_first_close && *count == 1 {
                anyhow::bail!("injected close failure");
            }
        }
        self.inner.close_replica(replica).await?;
        self.closed
            .lock()
            .expect("closed replicas mutex")
            .push(replica.clone());
        Ok(())
    }
    async fn open_replica(&self, replica: &kukuri_core::ReplicaId) -> Result<()> {
        self.opened
            .lock()
            .expect("opened replicas mutex")
            .push(replica.clone());
        self.inner.open_replica(replica).await
    }
    async fn apply_doc_op(&self, replica: &kukuri_core::ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica, op).await
    }
    async fn query_replica_with_policy(
        &self,
        replica: &kukuri_core::ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        let result = self
            .inner
            .query_replica_with_policy(replica, query, policy)
            .await?;
        self.reads
            .lock()
            .expect("reads mutex")
            .push((replica.clone(), result.len()));
        Ok(result)
    }
    async fn query_replica_keys(
        &self,
        replica: &kukuri_core::ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica, query).await
    }
    async fn subscribe_replica(
        &self,
        replica: &kukuri_core::ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica).await
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[tokio::test]
async fn worker_retries_failed_close_for_empty_buckets_without_stopping_another_scope() -> Result<()>
{
    let Some(admin) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        return Ok(());
    };
    let database = TestDatabase::create(&admin, "cn_bucket_close_retry").await?;
    let pool = connect_postgres(&database.database_url).await?;
    initialize_database(&pool).await?;
    for topic in ["removed", "kept"] {
        add_supported_topic(&pool, IndexScopeKind::PublicTopic, topic).await?;
    }
    let docs = Arc::new(ObservedDocs {
        fail_first_close: true,
        ..Default::default()
    });
    let (participant, projection) = participant(docs.clone(), pool.clone())?;
    let participant = participant.with_public_replica_mode(PublicReplicaReadMode::TimeBucketV1);
    let now = chrono::Utc::now().timestamp();
    let kept = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "kept".into(),
        },
        TimeBucket::from_unix_seconds(now)?,
    )?
    .replica_id();
    let post_id = post(&docs.inner, &kept, "kept", "keep this result", now).await?;
    let state = Arc::new(IndexerRuntimeState::default());
    let handle = IndexerWorker::new(
        Arc::new(participant),
        docs.clone(),
        state.clone(),
        WorkerConfig {
            poll_interval: Duration::from_millis(30),
            ..Default::default()
        },
    )
    .spawn();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        while state.snapshot().last_sync_at.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        kukuri_cn_core::remove_supported_topic(&pool, IndexScopeKind::PublicTopic, "removed")
            .await?;
        while docs.closed.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    handle.shutdown().await;
    let kept_index = projection
        .contains_object(IndexScopeKind::PublicTopic, "kept", &post_id)
        .await?;
    pool.close().await;
    database.cleanup().await?;
    result??;
    let closed = docs.closed.lock().unwrap();
    assert_eq!(closed.len(), 2);
    assert!(closed.iter().all(|id| matches!(BucketReplica::parse(id).unwrap().scope(), BucketScope::Topic { topic_id } if topic_id == "removed")));
    assert!(
        docs.close_attempts
            .lock()
            .unwrap()
            .values()
            .all(|count| *count == 2)
    );
    assert!(kept_index);
    Ok(())
}

#[tokio::test]
async fn ten_times_more_history_does_not_increase_public_startup_reads() -> Result<()> {
    let Some(admin) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        return Ok(());
    };
    let database = TestDatabase::create(&admin, "cn_bucket_scale").await?;
    let pool = connect_postgres(&database.database_url).await?;
    initialize_database(&pool).await?;
    let topic = "history-scale";
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, topic).await?;
    let mut measurements = Vec::new();
    for history in [10, 100] {
        let docs = Arc::new(ObservedDocs::default());
        for day in (1..=history).chain([999, 1000]) {
            let replica = BucketReplica::new(
                BucketScope::Topic {
                    topic_id: topic.into(),
                },
                TimeBucket::from_index(day)?,
            )?
            .replica_id();
            post(
                &docs.inner,
                &replica,
                topic,
                "a signed post",
                day as i64 * 86_400,
            )
            .await?;
        }
        let (participant, projection) = participant(docs.clone(), pool.clone())?;
        let participant = participant.with_public_replica_mode(PublicReplicaReadMode::TimeBucketV1);
        let scopes = participant
            .restore_selected_scopes(&participant.selected_scopes_at(1000 * 86_400).await?)
            .await?;
        assert_eq!(scopes.len(), 2);
        for scope in &scopes {
            participant.ingest_recent_scope(scope).await?;
        }
        assert_eq!(
            projection
                .count_scope(IndexScopeKind::PublicTopic, topic)
                .await?,
            2
        );
        let opened = docs.opened.lock().unwrap();
        let reads = docs.reads.lock().unwrap();
        assert!(
            opened
                .iter()
                .chain(reads.iter().map(|(id, _)| id))
                .all(|id| BucketReplica::parse(id)
                    .is_ok_and(|bucket| [999, 1000].contains(&bucket.bucket().index())))
        );
        measurements.push((
            opened.len(),
            reads.len(),
            reads.iter().map(|(_, count)| count).sum::<usize>(),
        ));
    }
    pool.close().await;
    database.cleanup().await?;
    assert_eq!(measurements[0], measurements[1]);
    assert_eq!(measurements[0].0, 4);
    assert!(measurements[0].2 > 0);
    Ok(())
}

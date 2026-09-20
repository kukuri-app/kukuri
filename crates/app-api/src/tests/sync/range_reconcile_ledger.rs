//! #1239: ページの範囲の照合の、次の照合までの間隔と、失敗の扱いを固定する。
//!
//! 独立監査(PR #1247)の再現 test と、mutation で固定されていなかった主張を含む。

use super::range_reconcile::{BASE_TIME, cursor_at, is_projected, range_fixture};
use super::*;
use crate::service::replica_window::{RANGE_CHECK_INTERVAL_MS, RANGE_CHECK_RETRY_INTERVAL_MS};

async fn put_json(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    key: String,
    value: serde_json::Value,
) {
    docs_sync
        .apply_doc_op(replica, DocOp::SetJson { key, value })
        .await
        .expect("write entry");
}

fn timeline_index_key(created_at: i64, object_id: &str) -> String {
    stable_key(
        "indexes/timeline",
        &format!("{created_at:020}-{object_id}/{object_id}"),
    )
}

/// 台帳にある範囲の、次の照合までの最長の待ち時間。
async fn longest_wait_ms(app: &AppService) -> i64 {
    app.services
        .range_checks
        .latest_next_check_at_ms_for_test()
        .await
        .expect("a recorded range")
        - Utc::now().timestamp_millis()
}

// 索引にあるが本体の届いていない entry が残った範囲は、短い間隔で照合し直す(30 秒の間隔に入れない)。
#[tokio::test]
async fn a_range_with_an_unresolved_entry_is_checked_again_after_the_retry_interval() {
    let fixture = range_fixture("ledger-unresolved", 3, |_| true).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    put_json(
        fixture.docs_sync.as_ref(),
        &replica,
        timeline_index_key(BASE_TIME + 10, "a".repeat(64).as_str()),
        serde_json::json!({}),
    )
    .await;
    fixture
        .app
        .reconcile_timeline_range(fixture.topic.as_str(), &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile");
    let wait = longest_wait_ms(&fixture.app).await;
    fixture.app.shutdown().await;
    assert!(
        wait <= RANGE_CHECK_RETRY_INTERVAL_MS,
        "an unresolved entry keeps the range on the retry interval, got {wait} ms"
    );
}

// 欠けの無い範囲は、長い間隔に入る。
#[tokio::test]
async fn a_complete_range_waits_for_the_full_interval() {
    let fixture = range_fixture("ledger-complete", 3, |_| true).await;
    fixture
        .app
        .reconcile_timeline_range(fixture.topic.as_str(), &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile");
    let wait = longest_wait_ms(&fixture.app).await;
    fixture.app.shutdown().await;
    assert!(
        wait > RANGE_CHECK_RETRY_INTERVAL_MS && wait <= RANGE_CHECK_INTERVAL_MS,
        "a complete range waits for the full interval, got {wait} ms"
    );
}

// 取り下げの record はあるが、対象の envelope がまだ届いていない行は「まだ反映できない」として、
// 短い間隔で照合し直す。envelope が届いた後の照合で、本文が伏せられる。
#[tokio::test]
async fn a_withdrawal_whose_target_envelope_is_missing_keeps_the_range_on_the_retry_interval() {
    let (app, store, docs_sync, _blobs) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:range-ledger-target-missing");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let envelope = build_post_envelope_with_payload_in_channel(
        &author_keys,
        &topic,
        PayloadRef::InlineText {
            text: "withdrawn".into(),
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("envelope");
    let object = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    // 対象の envelope は書かない。state・索引・取り下げだけを書き、行は projection に入れておく。
    put_json(
        docs_sync.as_ref(),
        &replica,
        stable_key("objects", &format!("{}/state", envelope.id.as_str())),
        serde_json::to_value(&object).expect("state json"),
    )
    .await;
    put_json(
        docs_sync.as_ref(),
        &replica,
        timeline_index_key(object.created_at, envelope.id.as_str()),
        serde_json::json!({}),
    )
    .await;
    let withdrawal = build_post_withdrawal_envelope(
        &author_keys,
        &envelope,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal");
    put_json(
        docs_sync.as_ref(),
        &replica,
        stable_key("withdrawals", &format!("{}/state", envelope.id.as_str())),
        serde_json::to_value(&withdrawal).expect("withdrawal json"),
    )
    .await;
    ObjectProjectionStore::put_object_projection(
        store.as_ref(),
        projection_row_from_header(&object, Some("withdrawn".into()), &replica),
    )
    .await
    .expect("put projection");

    let hydrated = app
        .reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile");
    assert_eq!(hydrated, 0);
    let wait = longest_wait_ms(&app).await;
    assert!(
        wait <= RANGE_CHECK_RETRY_INTERVAL_MS,
        "a withdrawal that cannot be verified yet keeps the range on the retry interval, got {wait} ms"
    );

    // 対象の envelope が届いた後の照合で、取り下げが反映される。
    put_json(
        docs_sync.as_ref(),
        &replica,
        stable_key("objects", &format!("{}/envelope", envelope.id.as_str())),
        serde_json::to_value(&envelope).expect("envelope json"),
    )
    .await;
    app.services.range_checks.expire_all_for_test().await;
    let hydrated = app
        .reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile after the envelope arrived");
    app.shutdown().await;
    assert_eq!(hydrated, 1);
    let projection_store: &dyn ProjectionStore = store.as_ref();
    assert!(
        projection_store
            .get_post_withdrawal(&envelope.id)
            .await
            .expect("withdrawal row")
            .is_some()
    );
}

// 投稿として読めない entry は「反映済み」に数えない。読めない entry だけが 1 ページぶんを超えて続いても、
// その先の投稿へ遡れる。
#[tokio::test]
async fn a_run_of_unreadable_entries_is_not_counted_as_a_resolved_page() {
    let posts = 20usize;
    let fixture = range_fixture("ledger-invalid-run", posts, |index| index >= posts - 2).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    let oldest_projected = &fixture.posts[posts - 2];
    // 読めない state を指す entry を 60 件、projection にある最古の投稿のすぐ古い側へ置く。
    for index in 0..60usize {
        let id = format!("{index:064x}");
        put_json(
            fixture.docs_sync.as_ref(),
            &replica,
            stable_key("objects", &format!("{id}/state")),
            serde_json::json!({ "not": "a canonical post header" }),
        )
        .await;
        put_json(
            fixture.docs_sync.as_ref(),
            &replica,
            timeline_index_key(oldest_projected.created_at, id.as_str()),
            serde_json::json!({}),
        )
        .await;
    }
    let hydrated = fixture
        .app
        .reconcile_timeline_range(
            fixture.topic.as_str(),
            &TimelineScope::Public,
            Some(&cursor_at(oldest_projected)),
            10,
        )
        .await
        .expect("reconcile");
    let reached = is_projected(fixture.store.as_ref(), &fixture.posts[posts - 3]).await;
    fixture.app.shutdown().await;
    assert!(
        reached,
        "the check must read past the unreadable entries (hydrated={hydrated})"
    );
    // 読む件数を 4 倍ずつ増やすので、最後の読み出しはページに要る件数より多くの投稿を反映しうる。
    assert!(hydrated >= 10, "hydrated={hydrated}");
}

/// key 指定の読み出しを失敗させる docs。docs の読み書きの失敗(I/O)を模す。
#[derive(Clone, Default)]
struct FailingExactReadsDocsSync {
    inner: MemoryDocsSync,
    fail: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl DocsSync for FailingExactReadsDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            anyhow::bail!("injected docs read failure");
        }
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<Vec<kukuri_docs_sync::DocKeyEntry>> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// ADR 0052 §2: 読めない record は読み飛ばすが、docs の読み出しそのものの失敗は握りつぶさず、エラーとして返す。
#[tokio::test]
async fn a_docs_read_failure_is_returned_as_an_error() {
    let docs_sync = Arc::new(FailingExactReadsDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:range-ledger-io-failure");
    let replica = topic_replica_id(topic.as_str());
    persist_test_post(
        docs_sync.as_ref(),
        None,
        &generate_keys(),
        &topic,
        PayloadRef::InlineText {
            text: "post".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    assert!(docs_sync.inner.open_replica(&replica).await.is_ok());
    docs_sync
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let outcome = app
        .reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, None, 20)
        .await;
    app.shutdown().await;
    let error = outcome.expect_err("a docs read failure must not be swallowed");
    assert!(format!("{error:#}").contains("injected docs read failure"));
}

// ADR 0052 §2: 署名の正しい取り下げでも、projection に保存できない generation のものは取り下げとして
// 扱わない。1 件置くだけで、その object を含む範囲の取得を失敗させられない(独立監査の再現)。
#[tokio::test]
async fn withdrawal_with_an_oversized_generation_does_not_fail_a_non_empty_head_page() {
    let dir = tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteStore::connect_file(dir.path().join("oversized-generation.sqlite"))
            .await
            .expect("sqlite"),
    );
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-ledger-oversized-generation");
    let replica = topic_replica_id(topic.as_str());
    let writer_keys = generate_keys();
    let mut envelopes = Vec::new();
    for index in 0..3usize {
        envelopes.push(
            persist_test_post(
                docs_sync.as_ref(),
                Some(store.as_ref()),
                &writer_keys,
                &topic,
                PayloadRef::InlineText {
                    text: format!("post {index}"),
                },
                Vec::new(),
                None,
            )
            .await,
        );
    }
    let withdrawal = build_post_withdrawal_envelope(
        &writer_keys,
        &envelopes[0],
        u64::MAX,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("a correctly signed withdrawal");
    persist_post_withdrawal(docs_sync.as_ref(), &replica, &envelopes[0].id, &withdrawal)
        .await
        .expect("persist withdrawal");
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let mut outcomes = Vec::new();
    for _ in 0..3 {
        outcomes.push(
            app.list_timeline(topic.as_str(), None, 20)
                .await
                .map(|view| view.items.len())
                .map_err(|error| format!("{error:#}")),
        );
    }
    app.shutdown().await;
    assert!(
        outcomes.iter().all(|outcome| *outcome == Ok(3)),
        "{outcomes:?}"
    );
}

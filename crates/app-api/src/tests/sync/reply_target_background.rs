//! #1239(AC-6、#1277): view の生成は docs を読まない。projection に無い返信先は、背景で key 指定で反映し、
//! この回の preview は出さない(反映できれば次の取得で出る)。背景の反映は、確認先ごとに間隔を空ける。

use super::hydration_integrity::{signed_post, write_object_entries};
use super::shadowing_docs::honest_header;
use super::*;
use kukuri_store::PostWithdrawalStore;

#[tokio::test]
async fn a_missing_reply_target_is_reflected_in_the_background_without_docs_reads_in_the_view() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-background");
    let replica = topic_replica_id(topic.as_str());
    let parent = signed_post(
        &generate_keys(),
        &topic,
        "the parent",
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&parent),
        &honest_header(&parent),
    )
    .await;
    let profiles = HashMap::new();
    let source = Some((&replica, topic.as_str()));

    // 背景の反映が走り出す前に、view の生成が docs を読まないことを確かめる(permit を取らせない)。
    let permits = app.services.reply_target_checks.permits();
    let held = permits
        .acquire_many(crate::service::hydration_limits::BACKGROUND_CHECK_MAX_CONCURRENT as u32)
        .await
        .expect("hold the permits");
    docs_sync.clear_queries().await;
    docs_sync.reset_records_returned();
    let first = app
        .reply_preview_for_object_id(Some(&parent.id), source, &profiles)
        .await
        .expect("reply preview");
    assert!(first.is_none(), "the preview waits for the background");
    assert!(
        docs_sync.queries().await.is_empty(),
        "the view generation does not read docs"
    );
    assert_eq!(docs_sync.records_returned(), 0);
    drop(held);
    let _ = app
        .reply_preview_for_object_id(Some(&parent.id), source, &profiles)
        .await
        .expect("retry after capacity returns");

    timeout(Duration::from_secs(10), async {
        while store
            .get_object_projection(&parent.id)
            .await
            .expect("projection")
            .is_none()
        {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the background reflects the reply target");
    let second = app
        .reply_preview_for_object_id(Some(&parent.id), source, &profiles)
        .await
        .expect("reply preview")
        .expect("the preview after the background");
    assert_eq!(second.content, "the parent");
}

// 反映できない返信先(手元に本体が無い)を表示し続けても、背景の読み出しは確認先ごとに間隔を空けて 1 回。
#[tokio::test]
async fn background_reflection_of_a_reply_target_is_spaced_per_target() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-spaced");
    let replica = topic_replica_id(topic.as_str());
    let missing = EnvelopeId::from("f".repeat(64).as_str());
    let profiles = HashMap::new();
    docs_sync.clear_queries().await;

    for _ in 0..5 {
        assert!(
            app.reply_preview_for_object_id(
                Some(&missing),
                Some((&replica, topic.as_str())),
                &profiles
            )
            .await
            .expect("reply preview")
            .is_none()
        );
    }
    sleep(Duration::from_millis(300)).await;

    let envelope_key = stable_key("objects", &format!("{}/envelope", missing.as_str()));
    let reads = docs_sync
        .queries()
        .await
        .into_iter()
        .filter(|(_, query)| *query == DocQuery::Exact(envelope_key.clone()))
        .count();
    assert_eq!(reads, 1, "one background read per target and interval");
    assert_eq!(app.services.reply_target_checks.len(), 1);
}

// 取得側の反映(#1277): 遡ったページの行でも、返信先が手元の docs にあれば、その取得で preview が出る
// (view の生成は docs を読まず、取得が view の生成の前に key 指定で反映する)。
#[tokio::test]
async fn an_older_page_reflects_the_reply_target_before_building_the_view() {
    // 購読タスクへは test が流した通知だけが届く(購読の窓の追いつきが返信先を先に反映しないように)。
    let docs_sync = Arc::new(super::subscription_catch_up::InjectedNoticesDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-older-page");
    let replica = topic_replica_id(topic.as_str());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe the topic");
    sleep(Duration::from_millis(200)).await;
    let keys = generate_keys();
    let parent = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_000,
        "the parent",
        None,
    )
    .await;
    let reply = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_010,
        "the reply",
        Some(&parent.envelope),
    )
    .await;
    // 返信だけが projection にある(返信先は、索引の照合の範囲の外)。
    super::range_reconcile::project(store.as_ref(), &reply, &replica).await;

    let page = app
        .list_timeline(
            topic.as_str(),
            Some(TimelineCursor {
                created_at: 1_700_000_011,
                object_id: EnvelopeId::from("f".repeat(64).as_str()),
            }),
            1,
        )
        .await
        .expect("older page");

    let item = page
        .items
        .iter()
        .find(|item| item.object_id == reply.object_id.as_str())
        .expect("the reply is listed");
    assert_eq!(
        item.reply_preview
            .as_ref()
            .map(|preview| preview.content.as_str()),
        Some("the parent"),
        "the preview is shown by the same listing"
    );
}

// thread の取得も、view の生成の前に返信先を反映する(返信先は、照合したページの範囲の外)。
#[tokio::test]
async fn a_thread_page_reflects_the_reply_target_before_building_the_view() {
    // 購読タスクへは test が流した通知だけが届く(購読の窓の追いつきが返信先を先に反映しないように)。
    let docs_sync = Arc::new(super::subscription_catch_up::InjectedNoticesDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-thread-page");
    let replica = topic_replica_id(topic.as_str());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe the topic");
    sleep(Duration::from_millis(200)).await;
    let keys = generate_keys();
    let root = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_000,
        "the root",
        None,
    )
    .await;
    let parent = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_010,
        "the parent",
        Some(&root.envelope),
    )
    .await;
    let reply = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_020,
        "the reply",
        Some(&parent.envelope),
    )
    .await;
    super::range_reconcile::project(store.as_ref(), &root, &replica).await;
    super::range_reconcile::project(store.as_ref(), &reply, &replica).await;

    // 返信先(parent)より後ろのページ。照合は reply だけで 1 件に届き、parent を反映しない。
    let page = app
        .list_thread(
            topic.as_str(),
            root.object_id.as_str(),
            Some(super::range_reconcile::cursor_at(&parent)),
            1,
        )
        .await
        .expect("thread page");

    let item = page
        .items
        .iter()
        .find(|item| item.object_id == reply.object_id.as_str())
        .expect("the reply is listed");
    assert_eq!(
        item.reply_preview
            .as_ref()
            .map(|preview| preview.content.as_str()),
        Some("the parent")
    );
}

// #1284 AC-1: boundedなProfileも、表示する行が参照する直前の返信先だけを
// Timeline / Threadと同じ有限本文回復へ渡す。Bookmarksはviewport上のPostCardから起動する。
#[tokio::test]
async fn profile_view_recovers_the_visible_reply_target_body() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:reply-target-secondary-views";
    let parent_id = app
        .create_post(topic, "parent visible through a preview", None)
        .await
        .expect("create parent");
    let reply_id = app
        .create_post(topic, "reply in secondary views", Some(parent_id.as_str()))
        .await
        .expect("create reply");
    let parent_object_id = EnvelopeId::from(parent_id.as_str());
    let mut parent_row = store
        .get_object_projection(&parent_object_id)
        .await
        .expect("projection")
        .expect("parent row");
    parent_row.content = None;
    store
        .put_object_projection(parent_row.clone())
        .await
        .expect("store missing parent");

    let profile = app
        .list_profile_timeline(app.current_author_pubkey().as_str(), None, 20)
        .await
        .expect("profile timeline");
    let profile_reply = profile
        .items
        .iter()
        .find(|post| post.object_id == reply_id)
        .expect("profile reply");
    assert_eq!(
        profile_reply
            .reply_preview
            .as_ref()
            .map(|preview| preview.content.as_str()),
        Some("parent visible through a preview")
    );
}

/// 本文 blob が「手元には無いが、remote からは取れる」状態を表す blob service。`fetch_blob` の回数を数える。
#[derive(Clone, Default)]
struct RemoteBodyBlobService {
    inner: MemoryBlobService,
    fetches: Arc<std::sync::atomic::AtomicUsize>,
}

/// 本文の提供元を test から復旧できる blob service。返信先の projection が `content: None` で
/// 先に保存された後も、表示中の preview が既存の有限 retry に入ることを確認する。
#[derive(Default)]
struct GatedRemoteBodyBlobService {
    inner: MemoryBlobService,
    open: std::sync::atomic::AtomicBool,
    fetches: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl BlobService for GatedRemoteBodyBlobService {
    async fn fetch_local_blob(
        &self,
        hash: &kukuri_core::BlobHash,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        self.inner.fetch_local_blob(hash).await
    }

    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        self.inner.fetch_blob(hash).await
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.local_blob_status(hash).await
    }

    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(BlobStatus::Missing);
        }
        self.inner.local_blob_status(hash).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// #1284 AC-1: 返信先としてだけ表示される行が `content: None` で projection に残っていても、
// その返信を含む範囲の取得を契機に `MissingBodyLedger` の範囲で本文を取り直す。
#[tokio::test]
async fn a_visible_reply_target_with_a_missing_body_recovers_after_its_provider_returns() {
    let docs_sync = Arc::new(super::subscription_catch_up::InjectedNoticesDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let blobs = Arc::new(GatedRemoteBodyBlobService::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        blobs.clone(),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-missing-body");
    let replica = topic_replica_id(topic.as_str());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe the topic");
    sleep(Duration::from_millis(200)).await;
    let keys = generate_keys();
    let stored = blobs
        .put_blob(b"recovered reply parent".to_vec(), "text/plain")
        .await
        .expect("put blob");
    let parent_envelope = build_post_envelope_with_payload_in_channel(
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: stored.hash.clone(),
            mime: "text/plain".into(),
            bytes: stored.bytes,
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("parent envelope");
    let parent_header = parent_envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    persist_post_object(
        docs_sync.as_ref(),
        &replica,
        parent_header.clone(),
        parent_envelope.clone(),
    )
    .await
    .expect("persist parent");
    let reply = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        parent_header.created_at + 10,
        "the reply",
        Some(&parent_envelope),
    )
    .await;
    super::range_reconcile::project(store.as_ref(), &reply, &replica).await;

    let cursor = TimelineCursor {
        created_at: reply.created_at + 1,
        object_id: EnvelopeId::from("f".repeat(64).as_str()),
    };
    app.list_timeline(topic.as_str(), Some(cursor.clone()), 1)
        .await
        .expect("timeline while the body is unavailable");
    timeout(Duration::from_secs(10), async {
        loop {
            if store
                .get_object_projection(&parent_header.object_id)
                .await
                .expect("projection")
                .is_some_and(|row| row.content.is_none())
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the reply target is projected without its body");

    let mut target_row = store
        .get_object_projection(&parent_header.object_id)
        .await
        .expect("projection")
        .expect("reply target row");
    let original_channel = target_row.channel_id.clone();
    target_row.channel_id = "different-scope".into();
    store
        .put_object_projection(target_row.clone())
        .await
        .expect("store mismatched scope");
    blobs.open.store(true, std::sync::atomic::Ordering::SeqCst);
    let fetches_before_scope_mismatch = blobs.fetches.load(std::sync::atomic::Ordering::SeqCst);
    let mismatched = app
        .list_timeline(topic.as_str(), Some(cursor.clone()), 1)
        .await
        .expect("timeline with a mismatched reply target scope");
    assert_eq!(
        mismatched.items[0]
            .reply_preview
            .as_ref()
            .map(|preview| preview.content.as_str()),
        Some("[blob pending]")
    );
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        fetches_before_scope_mismatch,
        "a target from another scope must not start blob I/O"
    );
    target_row.channel_id = original_channel;
    store
        .put_object_projection(target_row)
        .await
        .expect("restore target scope");

    let recovered = app
        .list_timeline(topic.as_str(), Some(cursor.clone()), 1)
        .await
        .expect("timeline after the provider returns");
    let preview = recovered.items[0]
        .reply_preview
        .as_ref()
        .expect("reply preview");
    assert_eq!(preview.content, "recovered reply parent");
    assert_eq!(
        app.services.missing_body_ledger.attempts(&stored.hash),
        0,
        "a successful retry clears the finite retry ledger"
    );

    let mut adult_row = store
        .get_object_projection(&parent_header.object_id)
        .await
        .expect("projection")
        .expect("recovered target");
    adult_row.content = None;
    adult_row.content_labels = vec!["adult".into()];
    store
        .put_object_projection(adult_row.clone())
        .await
        .expect("store adult missing target");
    let fetches_before_adult_gate = blobs.fetches.load(std::sync::atomic::Ordering::SeqCst);
    app.list_timeline(topic.as_str(), Some(cursor.clone()), 1)
        .await
        .expect("timeline with a gated missing target");
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        fetches_before_adult_gate,
        "an adult-gated reply target must be rejected before blob I/O"
    );
    app.set_adult_content_display_enabled(true);
    app.list_timeline(topic.as_str(), Some(cursor.clone()), 1)
        .await
        .expect("timeline after enabling adult content");

    let mut withdrawn_row = store
        .get_object_projection(&parent_header.object_id)
        .await
        .expect("projection")
        .expect("recovered target after enabling adult content");
    withdrawn_row.content = None;
    store
        .put_object_projection(withdrawn_row.clone())
        .await
        .expect("store missing withdrawn target");
    store
        .put_post_withdrawal(PostWithdrawalRow {
            target_object_id: parent_header.object_id.clone(),
            target_author_pubkey: withdrawn_row.author_pubkey.clone(),
            source_replica_id: withdrawn_row.source_replica_id.clone(),
            withdrawal_envelope_id: EnvelopeId::from("withdrawal-for-reply-target"),
            withdrawn_at: parent_header.created_at + 20,
            generation: 1,
            replacement_object_id: None,
            reason_visibility: WithdrawalReasonVisibility::Public,
            reason: Some(PostWithdrawalReason::AuthorRequest),
        })
        .await
        .expect("store withdrawal");
    let fetches_before_withdrawn = blobs.fetches.load(std::sync::atomic::Ordering::SeqCst);
    app.list_timeline(topic.as_str(), Some(cursor), 1)
        .await
        .expect("timeline with a withdrawn missing target");
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        fetches_before_withdrawn,
        "a withdrawn reply target must be rejected before blob I/O"
    );
}

#[async_trait]
impl BlobService for RemoteBodyBlobService {
    async fn fetch_local_blob(
        &self,
        _hash: &kukuri_core::BlobHash,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.fetch_blob(hash).await
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.inner.blob_status(hash).await
    }

    async fn local_blob_status(&self, _hash: &BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// 取得の経路は remote の本文を待たない。本文が手元に無い返信先は、取得では反映せず、remote から本文を取る背景の反映へ回す。
#[tokio::test]
async fn the_listing_does_not_wait_for_a_remote_body_of_the_reply_target() {
    // 購読タスクへは test が流した通知だけが届く(購読の窓の追いつきが返信先を先に反映しないように)。
    let docs_sync = Arc::new(super::subscription_catch_up::InjectedNoticesDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let blobs = Arc::new(RemoteBodyBlobService::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        blobs.clone(),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-remote-body");
    let replica = topic_replica_id(topic.as_str());
    app.ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe the topic");
    sleep(Duration::from_millis(200)).await;
    let keys = generate_keys();
    let stored = blobs
        .put_blob(b"the remote parent".to_vec(), "text/plain")
        .await
        .expect("put blob");
    let parent_envelope = build_post_envelope_with_payload_in_channel(
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: stored.hash.clone(),
            mime: "text/plain".into(),
            bytes: stored.bytes,
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
    )
    .expect("parent envelope");
    let parent_header = parent_envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    persist_post_object(
        docs_sync.as_ref(),
        &replica,
        parent_header.clone(),
        parent_envelope.clone(),
    )
    .await
    .expect("persist parent");
    let reply = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        parent_header.created_at + 10,
        "the reply",
        Some(&parent_envelope),
    )
    .await;
    super::range_reconcile::project(store.as_ref(), &reply, &replica).await;

    // 背景の反映を止めておき、取得の経路の blob の取得を数える。
    let permits = app.services.reply_target_checks.permits();
    let held = permits
        .acquire_many(crate::service::hydration_limits::BACKGROUND_CHECK_MAX_CONCURRENT as u32)
        .await
        .expect("hold the permits");
    let page = app
        .list_timeline(
            topic.as_str(),
            Some(TimelineCursor {
                created_at: reply.created_at + 1,
                object_id: EnvelopeId::from("f".repeat(64).as_str()),
            }),
            1,
        )
        .await
        .expect("older page");
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the listing does not fetch a remote body"
    );
    assert!(page.items.iter().all(|item| item.reply_preview.is_none()));
    drop(held);

    // 容量が戻った後の次の需要だけが、remote 本文の背景取得を開始する。
    let _ = app
        .list_timeline(
            topic.as_str(),
            Some(TimelineCursor {
                created_at: reply.created_at + 1,
                object_id: EnvelopeId::from("f".repeat(64).as_str()),
            }),
            1,
        )
        .await
        .expect("older page after capacity returns");
    timeout(Duration::from_secs(10), async {
        loop {
            let row = store
                .get_object_projection(&parent_header.object_id)
                .await
                .expect("projection");
            if row.is_some_and(|row| row.content.as_deref() == Some("the remote parent")) {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the background reflects the reply target with its remote body");
    assert!(blobs.fetches.load(std::sync::atomic::Ordering::SeqCst) >= 1);
}

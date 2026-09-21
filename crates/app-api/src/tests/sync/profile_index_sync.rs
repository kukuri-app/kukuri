//! #1239(T6-2 の独立監査 B-2'): 自分の replica に同期で届いた、索引の無い投稿の本体は、key の event より後に届く。
//! 購読タスクは、読めなかった key を覚え、本体がそろった通知で試し直して索引を足す。
//! docs author を知らない閲覧者の読み出し量は、投稿の数が旧 record の一覧の上限を超えると増えない。

use super::*;
use kukuri_docs_sync::{ReplicaNotice, ReplicaNoticeStream};
use std::collections::HashSet;

const OWNER: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const LEGACY: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const BASE_TIME: i64 = 1_700_000_000;

async fn profile_index_keys(docs_sync: &dyn DocsSync, pubkey: &str) -> Vec<String> {
    docs_sync
        .query_replica_keys(
            &author_replica_id(pubkey),
            kukuri_docs_sync::DocKeyQuery {
                prefix: "indexes/profile/".into(),
                order: kukuri_docs_sync::DocKeyOrder::Ascending,
                limit: 16,
            },
        )
        .await
        .expect("index keys")
        .entries
        .into_iter()
        .map(|entry| entry.key)
        .collect()
}

/// 本体を隠しておける docs。隠した key の record は、手元に本体が無いものとして読めない。
/// 購読には、test から流した通知だけが届く。
#[derive(Clone)]
struct DelayedContentDocsSync {
    inner: MemoryDocsSync,
    hidden: Arc<TokioMutex<HashSet<String>>>,
    notices: tokio::sync::broadcast::Sender<ReplicaNotice>,
}

impl Default for DelayedContentDocsSync {
    fn default() -> Self {
        Self {
            inner: MemoryDocsSync::with_docs_author(OWNER),
            hidden: Arc::default(),
            notices: tokio::sync::broadcast::channel(16).0,
        }
    }
}

#[async_trait]
impl DocsSync for DelayedContentDocsSync {
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
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        let hidden = self.hidden.lock().await.clone();
        let mut records = self
            .inner
            .query_replica_with_policy(replica_id, query, policy)
            .await?;
        records.retain(|record| !hidden.contains(&record.key));
        Ok(records)
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn query_replica_keys_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner
            .query_replica_keys_by_author(replica_id, docs_author, query)
            .await
    }

    async fn local_docs_author(&self) -> Result<Option<String>> {
        self.inner.local_docs_author().await
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        if self.hidden.lock().await.contains(key) {
            return Ok(None);
        }
        self.inner
            .query_replica_by_author(replica_id, docs_author, key, policy)
            .await
    }

    async fn subscribe_replica(
        &self,
        _replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        Ok(Box::pin(futures_util::stream::pending()))
    }

    async fn subscribe_replica_notices(
        &self,
        _replica_id: &ReplicaId,
    ) -> Result<ReplicaNoticeStream> {
        let stream = tokio_stream::wrappers::BroadcastStream::new(self.notices.subscribe());
        Ok(Box::pin(futures_util::StreamExt::filter_map(
            stream,
            |item| async move { item.ok().map(Ok) },
        )))
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// 補完を読み終えた後に、旧版の端末の投稿が同期で届く。key の event の時点では本体が無く、本体がそろった通知で索引が足される。
#[tokio::test]
async fn an_own_legacy_post_whose_content_arrives_later_is_indexed() {
    let docs_sync = Arc::new(DelayedContentDocsSync::default());
    let keys = generate_keys();
    let pubkey = keys.public_key_hex();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    );
    app.spawn_author_subscription(pubkey.as_str())
        .await
        .expect("subscribe myself");
    let checkpoint = format!("profile-index-backfill/{pubkey}/profile/posts/");
    timeout(Duration::from_secs(10), async {
        while store
            .get_sync_checkpoint(&checkpoint)
            .await
            .expect("checkpoint")
            .as_deref()
            != Some("done")
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the backfill over the empty replica finishes");

    // 旧版の端末の投稿(索引なし)が同期で届いた。key は届いたが、本体はまだ無い。
    let object_id = EnvelopeId::from(generate_keys().public_key_hex().as_str());
    let envelope = build_profile_post_envelope(
        &keys,
        &KukuriProfilePostEnvelopeContentV1 {
            author_pubkey: Pubkey::from(pubkey.as_str()),
            profile_topic_id: author_profile_topic_id(pubkey.as_str()),
            published_topic_id: TopicId::new("kukuri:topic:profile-sync"),
            object_id: object_id.clone(),
            created_at: BASE_TIME,
            object_kind: "post".into(),
            content: "legacy".into(),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        },
    )
    .expect("envelope");
    let post = parse_profile_post(&envelope).expect("parse").expect("post");
    let post_key = stable_key("profile/posts", object_id.as_str());
    {
        let mut hidden = docs_sync.hidden.lock().await;
        hidden.insert(post_key.clone());
        hidden.insert(stable_key("envelopes", envelope.id.as_str()));
    }
    persist_profile_post_doc(docs_sync.as_ref(), &post, &envelope)
        .await
        .expect("persist");
    docs_sync
        .apply_doc_op(
            &author_replica_id(pubkey.as_str()),
            DocOp::DeletePrefix {
                prefix: "indexes/profile/".into(),
            },
        )
        .await
        .expect("remove the index (a post by an older client)");
    docs_sync
        .notices
        .send(ReplicaNotice::Entry(kukuri_docs_sync::DocEvent {
            replica_id: author_replica_id(pubkey.as_str()),
            key: post_key,
            content_hash: String::new(),
            source_peer: Some("remote-peer".into()),
            // 旧版の端末(端末ごとの旧名義)が書いた entry。
            docs_author: Some(LEGACY.into()),
        }))
        .expect("send the entry");
    sleep(Duration::from_millis(300)).await;
    // ここでは一覧を取得しない(空の結果は購読の張り直しを起こし、本体の到着の通知による試し直しを確かめられない)。
    assert!(
        profile_index_keys(docs_sync.as_ref(), pubkey.as_str())
            .await
            .is_empty(),
        "the post cannot be indexed before its content arrives"
    );

    // 本体がそろった。
    docs_sync.hidden.lock().await.clear();
    docs_sync
        .notices
        .send(ReplicaNotice::ContentReady)
        .expect("content ready");
    timeout(Duration::from_secs(10), async {
        while profile_index_keys(docs_sync.as_ref(), pubkey.as_str())
            .await
            .is_empty()
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the content-ready notice indexes the post");
    timeout(Duration::from_secs(10), async {
        loop {
            let page = app
                .list_profile_timeline(pubkey.as_str(), None, 20)
                .await
                .expect("profile timeline");
            if page
                .items
                .iter()
                .any(|item| item.object_id == object_id.as_str())
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the post is indexed after its content arrives");
    app.shutdown().await;
}

// docs author を知らない閲覧者は、索引を名義を問わずにたどり、旧 record の上限つきの一覧も合わせる。読む量は、投稿の数が
// その上限(128 件)と索引の 1 回の一覧の上限を超えると増えない。
#[tokio::test]
async fn a_stranger_reads_a_bounded_amount_regardless_of_the_post_count() {
    let mut counts = Vec::new();
    for posts in [200usize, 1_000] {
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
        let keys = generate_keys();
        let pubkey = keys.public_key_hex();
        for index in 0..posts {
            let object_id = EnvelopeId::from(generate_keys().public_key_hex().as_str());
            let envelope = build_profile_post_envelope(
                &keys,
                &KukuriProfilePostEnvelopeContentV1 {
                    author_pubkey: Pubkey::from(pubkey.as_str()),
                    profile_topic_id: author_profile_topic_id(pubkey.as_str()),
                    published_topic_id: TopicId::new("kukuri:topic:profile-stranger"),
                    object_id,
                    created_at: BASE_TIME + index as i64,
                    object_kind: "post".into(),
                    content: format!("post {index}"),
                    attachments: Vec::new(),
                    reply_to_object_id: None,
                    root_id: None,
                    content_labels: Vec::new(),
                },
            )
            .expect("envelope");
            let post = parse_profile_post(&envelope).expect("parse").expect("post");
            persist_profile_post_doc(docs_sync.as_ref(), &post, &envelope)
                .await
                .expect("persist");
        }
        app.list_profile_timeline(pubkey.as_str(), None, 20)
            .await
            .expect("first page");
        sleep(Duration::from_millis(150)).await;
        docs_sync.reset_records_returned();

        let page = app
            .list_profile_timeline(pubkey.as_str(), None, 20)
            .await
            .expect("profile timeline");

        assert_eq!(page.items.len(), 20);
        counts.push(docs_sync.records_returned());
    }
    assert_eq!(
        counts[0], counts[1],
        "a stranger's reads must stop growing past the bounds"
    );
}

fn own_app(docs_sync: Arc<MemoryDocsSync>, keys: KukuriKeys) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        keys,
    );
    (app, store)
}

async fn wait_checkpoint(store: &MemoryStore, key: &str, value: &str) {
    timeout(Duration::from_secs(10), async {
        while store
            .get_sync_checkpoint(key)
            .await
            .expect("checkpoint")
            .as_deref()
            != Some(value)
        {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{key} becomes {value}"));
}

// やり直しの依頼は store に残り、購読を張り直しても(仕事を作り直しても)失われない(独立監査 B-5)。
#[tokio::test]
async fn a_restart_request_survives_a_resubscription() {
    let docs_sync = Arc::new(MemoryDocsSync::with_docs_author(OWNER));
    let keys = generate_keys();
    let pubkey = keys.public_key_hex();
    let (app, store) = own_app(docs_sync.clone(), keys.clone());
    let backfill = format!("profile-index-backfill/{pubkey}/profile/posts/");
    let mut work = OwnReplicaWork::start(&app.services, pubkey.as_str());
    wait_checkpoint(store.as_ref(), &backfill, "done").await;
    // 補完を読み終えた後に、索引の無い投稿が届き、その event を取りこぼした。
    let object_id = EnvelopeId::from(generate_keys().public_key_hex().as_str());
    let envelope = build_profile_post_envelope(
        &keys,
        &KukuriProfilePostEnvelopeContentV1 {
            author_pubkey: Pubkey::from(pubkey.as_str()),
            profile_topic_id: author_profile_topic_id(pubkey.as_str()),
            published_topic_id: TopicId::new("kukuri:topic:profile-restart"),
            object_id,
            created_at: BASE_TIME,
            object_kind: "post".into(),
            content: "legacy".into(),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        },
    )
    .expect("envelope");
    let post = parse_profile_post(&envelope).expect("parse").expect("post");
    persist_profile_post_doc(docs_sync.as_ref(), &post, &envelope)
        .await
        .expect("persist");
    docs_sync
        .apply_doc_op(
            &author_replica_id(pubkey.as_str()),
            DocOp::DeletePrefix {
                prefix: "indexes/profile/".into(),
            },
        )
        .await
        .expect("remove the index");
    work.request_restart(&app.services).await;
    // tick の前に購読が張り直された。
    drop(work);
    let mut work = OwnReplicaWork::start(&app.services, pubkey.as_str());

    timeout(Duration::from_secs(10), async {
        while profile_index_keys(docs_sync.as_ref(), pubkey.as_str())
            .await
            .is_empty()
        {
            work.on_tick(&app.services).await;
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the restart request is carried over and the backfill indexes the post");
    wait_checkpoint(
        store.as_ref(),
        &format!("own-replica-restart/{pubkey}"),
        "done",
    )
    .await;
}

// 覚えきれない数の読めない key が届いたら、覚えた分を捨てる前に、やり直しの依頼を store に残す。
// 自分の docs author が書いた entry(今の版の投稿)は、読めなくても覚えない。
#[tokio::test]
async fn overflowing_the_pending_keys_leaves_a_restart_request() {
    let docs_sync = Arc::new(MemoryDocsSync::with_docs_author(OWNER));
    let keys = generate_keys();
    let pubkey = keys.public_key_hex();
    let (app, store) = own_app(docs_sync, keys);
    let mut work = OwnReplicaWork::start(&app.services, pubkey.as_str());
    let pending_key = format!("own-profile-pending/{pubkey}");
    work.on_entry(
        &app.services,
        stable_key("profile/posts", &format!("{:064x}", 9_999)).as_str(),
        Some(OWNER),
    )
    .await;
    assert_eq!(
        store
            .get_sync_checkpoint(&pending_key)
            .await
            .expect("pending"),
        None,
        "an entry by the own docs author is not remembered"
    );
    for index in 0..513 {
        work.on_entry(
            &app.services,
            stable_key("profile/posts", &format!("{index:064x}")).as_str(),
            Some(LEGACY),
        )
        .await;
    }
    assert_eq!(
        store
            .get_sync_checkpoint(&format!("own-replica-restart/{pubkey}"))
            .await
            .expect("restart request")
            .as_deref(),
        Some("restart")
    );
}

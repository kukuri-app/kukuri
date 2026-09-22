//! #1239: ページの範囲の照合が、権限の無い replica を読まないことと、空の索引からの復旧を固定する。
//!
//! 独立監査(PR #1247)の再現 test を恒久化したものを含む。

use super::range_reconcile::{BASE_TIME, put_post_at};
use super::*;

/// replica ごとの読み出しを記録する docs。`silent_events` のときは docs の event を配らない。
#[derive(Clone, Default)]
pub(super) struct RecordingDocsSync {
    inner: MemoryDocsSync,
    pub(super) reads: Arc<TokioMutex<Vec<String>>>,
    silent_events: bool,
}

#[async_trait]
impl DocsSync for RecordingDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn register_private_replica_secret(
        &self,
        replica_id: &ReplicaId,
        namespace_secret_hex: &str,
    ) -> Result<()> {
        self.inner
            .register_private_replica_secret(replica_id, namespace_secret_hex)
            .await
    }

    async fn remove_private_replica_secret(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.remove_private_replica_secret(replica_id).await
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
        self.reads
            .lock()
            .await
            .push(replica_id.as_str().to_string());
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.reads
            .lock()
            .await
            .push(replica_id.as_str().to_string());
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        if self.silent_events {
            return Ok(Box::pin(futures_util::stream::pending()));
        }
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

fn recording_app(docs_sync: Arc<RecordingDocsSync>) -> AppService {
    recording_app_with_blobs(docs_sync, Arc::new(MemoryBlobService::default())).0
}

pub(super) fn recording_app_with_blobs(
    docs_sync: Arc<RecordingDocsSync>,
    blob_service: Arc<MemoryBlobService>,
) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        generate_keys(),
    );
    (app, store)
}

// INVAR-2 / TR-12: 退出した private channel の thread と scope を取得しても、その channel の replica を読まない。
// thread の表示は以前と同じ(projection に残った行だけで組み立てる)で、取得は失敗しない。
#[tokio::test]
async fn left_private_channel_is_not_read_by_the_thread_or_the_timeline_reconcile() {
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let app = recording_app(docs_sync.clone());
    let topic = "kukuri:topic:range-left-channel";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "left".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let channel_ref = ChannelRef::PrivateChannel {
        channel_id: channel_id.clone(),
    };
    let root = app
        .create_post_in_channel(topic, channel_ref.clone(), "private root", None)
        .await
        .expect("private root");
    app.create_post_in_channel(topic, channel_ref, "private reply", Some(root.as_str()))
        .await
        .expect("private reply");
    app.leave_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("leave");
    sleep(Duration::from_millis(200)).await;
    docs_sync.reads.lock().await.clear();

    let thread = app.list_thread(topic, root.as_str(), None, 20).await;
    let channel_timeline = app
        .list_timeline_scoped(topic, TimelineScope::Channel { channel_id }, None, 20)
        .await;
    let public = app
        .list_timeline_scoped(topic, TimelineScope::Public, None, 20)
        .await
        .expect("public");
    sleep(Duration::from_millis(200)).await;
    let channel_reads = docs_sync
        .reads
        .lock()
        .await
        .iter()
        .filter(|replica| replica.starts_with("channel::"))
        .cloned()
        .collect::<Vec<_>>();
    app.shutdown().await;
    assert!(
        channel_reads.is_empty(),
        "a left private channel replica must not be read: {channel_reads:?}"
    );
    assert!(thread.is_ok(), "the thread listing itself must not fail");
    assert!(
        channel_timeline.is_err(),
        "the channel scope must be refused"
    );
    assert!(public.items.is_empty());
}

// 空の索引を 1 回見た先頭の範囲は、間隔を空けずに次の取得でも読む。投稿が docs に入った直後のページを、
// docs の event に頼らずに組み立てられる(以前は、空ページの取得のたびに走査して復旧していた)。
#[tokio::test]
async fn an_empty_head_range_is_checked_again_on_the_next_listing() {
    let docs_sync = Arc::new(RecordingDocsSync {
        silent_events: true,
        ..RecordingDocsSync::default()
    });
    let app = recording_app(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:range-empty-head");
    let replica = topic_replica_id(topic.as_str());
    let first = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("first listing");
    assert!(first.items.is_empty());
    let author_keys = generate_keys();
    for index in 0..3usize {
        put_post_at(
            docs_sync.as_ref(),
            &replica,
            &author_keys,
            &topic,
            BASE_TIME + index as i64,
            format!("post {index}").as_str(),
            None,
        )
        .await;
    }
    let second = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("second listing");
    app.shutdown().await;
    assert_eq!(second.items.len(), 3);
}

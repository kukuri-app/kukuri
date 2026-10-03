//! #1218 AC-5b: 本人の別の端末からの差分の取得・変更の窓・送り直し（ADR 0061 §10）。
//! 固定の sequence: 通知の欠落、offline からの復帰、再起動、保存の失敗、account の切替。窓を超えた相手の周回と再開、
//! 休止した履歴を 10 倍にしても 1 回の取得の仕事が増えないこと。

use super::*;
use kukuri_core::{
    AccountSyncItem, AccountSyncItemKey, ChannelAudienceKind, ChannelId, CreatePrivateChannelInput,
    TopicId,
};
use kukuri_store::SqliteStore;

use crate::AccountSyncStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 試験の端末の docs。書込み・読取りを止められ、読取りの回数を数え、`remote` を本人の別の端末の reader として返す。
#[derive(Clone)]
pub(super) struct DeviceDocs {
    inner: MemoryDocsSync,
    writes_down: Arc<AtomicBool>,
    /// 0 になったら読取りが失敗する（相手が離れた）。
    reads_left: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
    /// 読んだ record の値の bytes と、書込みの回数。
    bytes: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    remote: Arc<std::sync::Mutex<Option<Arc<dyn DocsSync>>>>,
}

impl DeviceDocs {
    fn new() -> Self {
        Self {
            inner: MemoryDocsSync::with_docs_author(ACCOUNT_DOCS_AUTHOR),
            writes_down: Arc::default(),
            reads_left: Arc::new(AtomicUsize::new(usize::MAX)),
            reads: Arc::default(),
            bytes: Arc::default(),
            writes: Arc::default(),
            remote: Arc::default(),
        }
    }

    fn read(&self) -> Result<()> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.reads_left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("the device is unreachable"))
    }

    fn reads(&self) -> usize {
        self.reads.swap(0, Ordering::SeqCst)
    }

    /// (読取り、読んだ bytes、書込み) を数え直す。
    fn work(&self) -> (usize, usize, usize) {
        (
            self.reads.swap(0, Ordering::SeqCst),
            self.bytes.swap(0, Ordering::SeqCst),
            self.writes.swap(0, Ordering::SeqCst),
        )
    }

    fn counted(&self, records: &[DocRecord]) {
        let bytes = records.iter().map(|record| record.value.len()).sum();
        self.bytes.fetch_add(bytes, Ordering::SeqCst);
    }

    fn reachable(&self, reads: usize) {
        self.reads_left.store(reads, Ordering::SeqCst);
    }

    fn reading_from(&self, other: &DeviceDocs) {
        *self.remote.lock().expect("remote") = Some(Arc::new(other.clone()));
    }
}

#[async_trait]
impl DocsSync for DeviceDocs {
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
    async fn install_private_epoch_secrets(
        &self,
        source: Arc<dyn kukuri_docs_sync::PrivateEpochSecrets>,
    ) -> Result<()> {
        self.inner.install_private_epoch_secrets(source).await
    }
    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        anyhow::ensure!(
            !self.writes_down.load(Ordering::SeqCst),
            "the device storage is failing"
        );
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.apply_doc_op(replica_id, op).await
    }
    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }
    async fn query_replica_exact_bounded(
        &self,
        replica_id: &ReplicaId,
        key: &str,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.read()?;
        let records = self
            .inner
            .query_replica_exact_bounded(replica_id, key, limit, policy)
            .await?;
        self.counted(&records);
        Ok(records)
    }
    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.read()?;
        self.inner.query_replica_keys(replica_id, query).await
    }
    async fn query_replica_keys_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.read()?;
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
    ) -> Result<Option<DocRecord>> {
        self.read()?;
        let record = self
            .inner
            .query_replica_by_author(replica_id, docs_author, key, policy)
            .await?;
        self.counted(record.as_slice());
        Ok(record)
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
    async fn remote_readers(
        &self,
        _replica: &ReplicaId,
        _private_secret: Option<[u8; 32]>,
        _scope_peers: Vec<kukuri_transport::SeedPeer>,
    ) -> Result<Vec<Arc<dyn DocsSync>>> {
        Ok(self
            .remote
            .lock()
            .expect("remote")
            .iter()
            .cloned()
            .collect())
    }
}

/// 同じ account の端末（FakeTransport の ID が端末 ID）。差分の取得・送り直しの task は起こさない（試験が契機を呼ぶ）。
async fn own_device(
    keys: &KukuriKeys,
    id: &str,
    docs: &DeviceDocs,
    store: Arc<dyn ProjectionStore>,
    base: Arc<dyn Store>,
) -> AppService {
    let transport = Arc::new(FakeTransport::new(id, FakeNetwork::default()));
    let app = app_service_from_dependencies(
        base,
        store,
        transport.clone(),
        transport,
        Arc::new(docs.clone()),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    );
    register_account_replica(&app).await;
    app
}

pub(super) async fn memory_device(keys: &KukuriKeys, id: &str) -> (AppService, DeviceDocs) {
    let docs = DeviceDocs::new();
    let store = Arc::new(MemoryStore::default());
    (
        own_device(keys, id, &docs, store.clone(), store).await,
        docs,
    )
}

pub(super) async fn trusted(app: &AppService) -> Vec<String> {
    app.list_trust_always_visible().await.expect("trusted")
}

pub(super) async fn trust(app: &AppService, count: usize) -> Vec<String> {
    let mut authors = Vec::new();
    for _ in 0..count {
        let author = generate_keys().public_key_hex();
        app.set_trust_always_visible(&author, true)
            .await
            .expect("trust");
        authors.push(author);
    }
    authors.sort();
    authors
}

/// 通知の欠落と offline からの復帰: hint が届かず、相手に届かない間の取得は失敗して未同期を示し、相手に届いたら cursor
/// から読み残しを読む。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missed_hints_and_offline_devices_are_recovered_from_the_cursor() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    let authors = trust(&a, 3).await;

    a_docs.reachable(0);
    assert!(b.fetch_account_sync_from("device-a").await.is_err());
    assert!(trusted(&b).await.is_empty());
    let status = b.account_sync_status().await.expect("status");
    assert!(status.fetch_failed);

    a_docs.reachable(usize::MAX);
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert_eq!(trusted(&b).await, authors);
    assert!(!b.account_sync_status().await.expect("status").fetch_failed);
}

/// 相手 1 台から、その相手が別の端末から採った変更も読める（変更の窓は採った変更をすべて足す）。往復は同じ版で止まる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changes_relay_through_another_device_and_stop_at_the_same_version() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    let (c, c_docs) = memory_device(&keys, "device-c").await;
    b_docs.reading_from(&a_docs);
    c_docs.reading_from(&b_docs);
    let authors = trust(&a, 2).await;
    b.fetch_account_sync_from("device-a").await.expect("b");
    c.fetch_account_sync_from("device-b").await.expect("c");
    assert_eq!(trusted(&c).await, authors);

    // A が B から読み返しても、同じ版は採らず、A の窓は増えない。
    a_docs.reading_from(&b_docs);
    let head = async |app: &AppService| {
        app.read_account_sync_docs_key("changes/device-a/head", DocFetchPolicy::LocalOnly)
            .await
            .expect("head")
            .map(|item| item.value)
    };
    let before = head(&a).await;
    a.fetch_account_sync_from("device-b").await.expect("a");
    assert_eq!(head(&a).await, before);
}

/// 窓（256 件）を超えて離れていた相手は周回で追い付き、周回が途中で止まっても cursor の位置から再開する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_past_the_window_catches_up_by_a_cycle_that_resumes() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    let mut authors = trust(&a, 1).await;
    b.fetch_account_sync_from("device-a").await.expect("first");
    authors.extend(trust(&a, 300).await);
    authors.sort();

    a_docs.reachable(100);
    assert!(b.fetch_account_sync_from("device-a").await.is_err());
    let status = b.account_sync_status().await.expect("status");
    assert!(status.behind && status.fetch_failed);
    let partial = trusted(&b).await.len();
    assert!(partial > 1 && partial < authors.len(), "{partial}");

    a_docs.reachable(usize::MAX);
    b.fetch_account_sync_from("device-a").await.expect("resume");
    assert_eq!(trusted(&b).await, authors);
    let status = b.account_sync_status().await.expect("status");
    assert!(!status.behind && !status.fetch_failed);
}

/// 保存の失敗: 書けなかった編集もこの端末では反映し、新しい編集を保留にせず、送信待ちの間は未同期を示す。書けるように
/// なったら送り直し、別の端末に届く。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unwritten_edits_are_unsynced_until_resent_and_never_hold_new_edits() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);

    a_docs.writes_down.store(true, Ordering::SeqCst);
    let authors = trust(&a, 2).await;
    assert_eq!(trusted(&a).await, authors);
    assert!(
        a.account_sync_status()
            .await
            .expect("status")
            .pending_writes
    );

    a_docs.writes_down.store(false, Ordering::SeqCst);
    a.resend_account_sync_items().await.expect("resend");
    assert!(
        !a.account_sync_status()
            .await
            .expect("status")
            .pending_writes
    );
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert_eq!(trusted(&b).await, authors);
}

/// 再起動: cursor は store に残り、再起動した端末は新しい変更だけを読む。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_device_resumes_from_its_durable_cursor() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let b_docs = DeviceDocs::new();
    b_docs.reading_from(&a_docs);
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("device-b.db");
    let connect = || async { Arc::new(SqliteStore::connect_file(&path).await.expect("sqlite")) };
    let store = connect().await;
    let b = own_device(&keys, "device-b", &b_docs, store.clone(), store).await;
    let mut authors = trust(&a, 5).await;
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    drop(b);

    authors.extend(trust(&a, 1).await);
    authors.sort();
    let store = connect().await;
    let b = own_device(&keys, "device-b", &b_docs, store.clone(), store).await;
    a_docs.reads();
    b.fetch_account_sync_from("device-a").await.expect("resume");
    // head、1 件の slot、その item だけ。
    assert_eq!(a_docs.reads(), 3);
    assert_eq!(trusted(&b).await, authors);
}

/// account の切替: 別の account の端末は、同じ相手からも自分の account の replica しか読めず（replica の id と item の
/// 封が account の鍵から決まる）、何も採らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_account_takes_nothing_from_the_device() {
    let (a, a_docs) = memory_device(&generate_keys(), "device-a").await;
    let (other, other_docs) = memory_device(&generate_keys(), "device-o").await;
    other_docs.reading_from(&a_docs);
    trust(&a, 2).await;
    assert!(other.fetch_account_sync_from("device-a").await.is_err());
    assert!(trusted(&other).await.is_empty());
}

/// 別の端末で参加した private channel に、取得で参加する（参加の版の後に鍵が届く）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_channel_joined_on_one_device_is_joined_on_the_other() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    for app in [&a, &b] {
        app.restore_joined_private_channels()
            .await
            .expect("restore");
    }
    let topic = "kukuri:topic:account-sync-fetch";
    let _ = a.list_timeline(topic, None, 20).await;
    let channel = a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "synced".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create")
        .channel_id;
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    let state = b
        .joined_private_channel_state(topic, channel.as_str())
        .await
        .expect("joined on the other device");
    assert_eq!(
        state.current_epoch_id,
        a.joined_private_channel_state(topic, channel.as_str())
            .await
            .expect("joined")
            .current_epoch_id
    );
}

/// 休止した channel・履歴を 10 倍にしても、新しい 1 件の取得の読取りの回数と bytes は増えない（変更の窓の seq の
/// 桁がそろう規模で比べる。seq は 100 と 991）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_change_fetch_does_not_grow_with_idle_channels_or_history() {
    let mut work = Vec::new();
    for idle in [33, 330] {
        let keys = generate_keys();
        let (a, a_docs) = memory_device(&keys, "device-a").await;
        let (b, b_docs) = memory_device(&keys, "device-b").await;
        b_docs.reading_from(&a_docs);
        trust(&a, idle).await;
        // 休止した channel（参加の版と世代の鍵）。
        for index in 0..idle {
            let channel = ChannelId::new(format!("idle-{index:04}"));
            a.write_account_sync_item(&AccountSyncItem::edit(
                AccountSyncItemKey::ChannelMembership {
                    channel_id: channel.clone(),
                },
                1,
                None,
            ))
            .await
            .expect("idle membership");
            a.write_account_sync_item(
                &AccountSyncItem::channel_epoch(
                    &channel,
                    "epoch-1-x",
                    1,
                    serde_json::to_value(PrivateChannelEpochCapability {
                        epoch_id: "epoch-1-x".into(),
                        namespace_secret_hex: hex::encode([1_u8; 32]),
                    })
                    .expect("epoch"),
                )
                .expect("epoch item"),
            )
            .await
            .expect("idle epoch");
        }
        b.fetch_account_sync_from("device-a")
            .await
            .expect("catch up");
        trust(&a, 1).await;
        a_docs.work();
        b.fetch_account_sync_from("device-a").await.expect("fetch");
        let (reads, bytes, _) = a_docs.work();
        work.push((reads, bytes));
    }
    assert_eq!(work[0], work[1]);
}

/// 送り直し: 書けた行を 10 倍にしても、1 件の送り直しの読み書きは増えない。1 page（64 件）を超える送信待ちは、
/// 索引が空になるまで続けて送る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resend_work_does_not_grow_and_continues_past_a_page() {
    let mut work = Vec::new();
    for written in [10, 100] {
        let (a, a_docs) = memory_device(&generate_keys(), "device-a").await;
        trust(&a, written).await;
        a_docs.writes_down.store(true, Ordering::SeqCst);
        trust(&a, 1).await;
        a_docs.writes_down.store(false, Ordering::SeqCst);
        a_docs.work();
        a.resend_account_sync_items().await.expect("resend");
        let (reads, _, writes) = a_docs.work();
        work.push((reads, writes));
    }
    assert_eq!(work[0], work[1]);

    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    a_docs.writes_down.store(true, Ordering::SeqCst);
    let authors = trust(&a, 70).await;
    a_docs.writes_down.store(false, Ordering::SeqCst);
    a.resend_account_sync_items().await.expect("resend");
    assert!(
        !a.account_sync_status()
            .await
            .expect("status")
            .pending_writes
    );
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert_eq!(trusted(&b).await, authors);
}

/// cursor の上限（自分を除いて 16 相手）で cursor を消された相手とは、周回で追い付く。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_whose_cursor_was_evicted_catches_up_by_a_cycle() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    let mut authors = trust(&a, 2).await;
    b.fetch_account_sync_from("device-a").await.expect("first");
    let store = &b.services.projection_store;
    for index in 0..kukuri_store::ACCOUNT_SYNC_CURSOR_LIMIT {
        store
            .put_account_sync_cursor(
                &kukuri_store::AccountSyncCursor {
                    device_id: format!("device-{index:02}"),
                    seq: 1,
                    head: 1,
                    cycle_prefix: None,
                    cycle_head: 0,
                    updated_at: Utc::now().timestamp_millis(),
                },
                "device-b",
            )
            .await
            .expect("newer cursor");
    }
    assert!(
        store
            .get_account_sync_cursor("device-a")
            .await
            .expect("cursor")
            .is_none()
    );
    authors.extend(trust(&a, 1).await);
    authors.sort();
    b.fetch_account_sync_from("device-a").await.expect("cycle");
    assert_eq!(trusted(&b).await, authors);
}

/// 契機の取得: 本人の端末の候補が無ければ未同期を示し、作り直し（自分の replica の周回）は 1 回で終わる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trigger_without_own_devices_is_unsynced() {
    let (a, _) = memory_device(&generate_keys(), "device-a").await;
    assert!(a.account_sync_status().await.expect("status").rebuilding);
    a.catch_up_account_sync().await;
    let status = a.account_sync_status().await.expect("status");
    assert!(status.no_peers && !status.rebuilding);
}

/// hint の契機: 書いた端末の hint を受けた lease の task が、その端末から取得する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hint_makes_the_other_device_fetch() {
    let keys = generate_keys();
    let network = FakeNetwork::default();
    let mut devices = Vec::new();
    for id in ["device-a", "device-b"] {
        let docs = DeviceDocs::new();
        let store = Arc::new(MemoryStore::default());
        let transport = Arc::new(FakeTransport::new(id, network.clone()));
        let app = app_service_from_dependencies(
            store.clone(),
            store,
            transport.clone(),
            transport,
            Arc::new(docs.clone()),
            Arc::new(MemoryBlobService::default()),
            keys.clone(),
        );
        app.start_account_sync().await.expect("account sync");
        devices.push((app, docs));
    }
    let [(a, a_docs), (b, b_docs)] = <[_; 2]>::try_from(devices).ok().expect("two devices");
    b_docs.reading_from(&a_docs);
    let authors = trust(&a, 1).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while trusted(&b).await != authors {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the hint makes the other device fetch");
}

/// 本人の端末の候補を返す hint の transport（rendezvous の応答で入る候補を、試験が後から足す）。
#[derive(Clone)]
struct CandidateHints {
    inner: Arc<FakeTransport>,
    peers: Arc<std::sync::Mutex<Vec<kukuri_transport::SeedPeer>>>,
}

#[async_trait]
impl HintTransport for CandidateHints {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<kukuri_transport::HintStream> {
        self.inner.subscribe_hints(topic).await
    }
    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        self.inner.unsubscribe_hints(topic).await
    }
    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        self.inner.publish_hint(topic, hint).await
    }
    async fn topic_read_candidates(&self, _: &TopicId) -> Result<Vec<kukuri_transport::SeedPeer>> {
        Ok(self.peers.lock().expect("peers").clone())
    }
}

/// 起動・復帰の時点では gossip の候補がまだ無い。rendezvous の応答に新しく現れた本人の端末からは、gossip の合流を
/// 待たずに取得し、未同期の「候補なし」を下ろす。同じ端末が続く応答（更新ごと）では読み直さない（#1218 AC-5b 監査
/// B-1・B-3）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_devices_appearing_in_rendezvous_are_fetched_from_once() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let b_docs = DeviceDocs::new();
    b_docs.reading_from(&a_docs);
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("device-b", FakeNetwork::default()));
    let hints = CandidateHints {
        inner: transport.clone(),
        peers: Arc::default(),
    };
    let b = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        Arc::new(hints),
        Arc::new(b_docs.clone()),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    );
    b.start_account_sync().await.expect("account sync");
    let until = |done: fn(&AccountSyncStatus) -> bool| {
        let b = &b;
        async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while !done(&b.account_sync_status().await.expect("status")) {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("status");
        }
    };
    until(|status| status.no_peers).await;
    let authors = trust(&a, 2).await;
    // gossip の候補（`topic_read_candidates`）は空のまま、rendezvous の候補だけで読む。
    let topic = kukuri_core::wire::hint_topic_id(keys.derive_account_sync().hint_topic());
    let peers = [kukuri_transport::SeedPeer {
        endpoint_id: "device-a".into(),
        addr_hint: None,
    }];
    b.account_sync_peers_joined("cn-1", topic.as_str(), &peers)
        .await;
    until(|status| !status.no_peers).await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while trusted(&b).await != authors {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the appeared device is fetched from");

    // 同じ端末が続く応答と、その端末の居ない別の CN の応答では読み直さない（CN ごとの更新で繰り返さない）。
    a_docs.work();
    for _ in 0..2 {
        b.account_sync_peers_joined("cn-1", topic.as_str(), &peers)
            .await;
        b.account_sync_peers_joined("cn-2", topic.as_str(), &[])
            .await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(a_docs.work(), (0, 0, 0));

    // 復帰（task の作り直し）の後は、次の応答に居る端末から 1 回読む。
    let more = trust(&a, 1).await;
    let mut authors = authors;
    authors.extend(more);
    authors.sort();
    b.restart_scope_subscription(&ScopeKey::AccountSync(
        keys.derive_account_sync().hint_topic().as_str().to_string(),
    ))
    .await;
    b.account_sync_peers_joined("cn-1", topic.as_str(), &peers)
        .await;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while trusted(&b).await != authors {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("after the task is rebuilt, the device is fetched from again");
}

/// 本人の別の端末で参加した channel の、参加の版と世代の鍵を `channels` 件、手元の replica に書く。
async fn write_channel_items(app: &AppService, channels: usize) {
    for index in 0..channels {
        let channel = ChannelId::new(format!("kept-{index:04}"));
        app.write_account_sync_item(
            &AccountSyncItem::channel_epoch(
                &channel,
                "epoch-86400000-x",
                10,
                serde_json::to_value(PrivateChannelEpochCapability {
                    epoch_id: "epoch-86400000-x".into(),
                    namespace_secret_hex: hex::encode([7_u8; 32]),
                })
                .expect("epoch"),
            )
            .expect("epoch item"),
        )
        .await
        .expect("epoch item");
        app.write_account_sync_item(&AccountSyncItem::edit(
            AccountSyncItemKey::ChannelMembership {
                channel_id: channel.clone(),
            },
            10,
            Some(
                serde_json::to_value(kukuri_core::ChannelMembershipV1 {
                    topic_id: "kukuri:topic:rebuild".into(),
                    label: channel.as_str().into(),
                    creator_pubkey: "0".repeat(63) + "1",
                    owner_pubkey: "0".repeat(63) + "1",
                    joined_via_pubkey: None,
                    audience_kind: ChannelAudienceKind::InviteOnly,
                    current_epoch_id: "epoch-86400000-x".into(),
                })
                .expect("membership"),
            ),
        ))
        .await
        .expect("membership item");
    }
}

/// #1218 AC-5c: DB を失った端末は、手元の replica を周回して参加と鍵を作り直す。周回の 1 回（1 page）の読取りは、
/// 参加・世代の数を 10 倍にしても増えない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebuild_page_does_not_grow_with_channels() {
    let mut largest = Vec::new();
    for channels in [10, 100] {
        let keys = generate_keys();
        let docs = DeviceDocs::new();
        let before = MemoryStore::default();
        let old = own_device(
            &keys,
            "device-a",
            &docs,
            Arc::new(before.clone()),
            Arc::new(before),
        )
        .await;
        write_channel_items(&old, channels).await;
        drop(old);

        // DB を失った: 同じ docs（replica）と、空の store。
        let store = Arc::new(MemoryStore::default());
        let app = own_device(&keys, "device-a", &docs, store.clone(), store).await;
        app.restore_joined_private_channels()
            .await
            .expect("restore");
        let mut prefix = Some("profile".to_string());
        let mut most = 0;
        while let Some(current) = prefix {
            docs.work();
            prefix = app
                .account_sync_cycle_step(&docs, &current, true)
                .await
                .expect("rebuild step");
            most = most.max(docs.work().0);
        }
        let mut cursor = None;
        let mut joined = 0;
        loop {
            let page = app
                .list_joined_private_channels("kukuri:topic:rebuild", cursor.as_deref())
                .await
                .expect("list");
            joined += page.items.len();
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(joined, channels);
        largest.push(most);
    }
    assert_eq!(largest[0], largest[1]);
}

/// 印を待ち始めてから `step` を行い、印が付いた後の通信状態の account 同期の状態を返す。
async fn marked(
    app: &AppService,
    changes: &kukuri_transport::StatusChanges,
    step: impl std::future::Future<Output = ()>,
) -> AccountSyncStatus {
    let (keys, ()) = tokio::join!(
        tokio::time::timeout(std::time::Duration::from_secs(10), changes.changed()),
        step
    );
    assert!(
        keys.expect("a mark")
            .contains(&kukuri_transport::StatusKey::Summary)
    );
    app.get_sync_status()
        .await
        .expect("sync status")
        .account_sync
}

/// W5 AC-6a: account 同期の状態は通信状態（`get_sync_status`・差分の通知）に載り、候補の有無・取得の失敗と成功・送信待ちの
/// 発生と解消のたびに、通信状態の印が付く（差分の通知がそれを読み直す）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_account_sync_status_rides_on_the_sync_status_and_marks_each_change() {
    let keys = generate_keys();
    let (_a, a_docs) = memory_device(&keys, "device-a").await;
    let changes = kukuri_transport::StatusChanges::default();
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    let b = b.with_status_changes(changes.clone());
    b_docs.reading_from(&a_docs);

    assert!(
        marked(&b, &changes, b.catch_up_account_sync())
            .await
            .no_peers
    );
    a_docs.reachable(0);
    let failed = marked(&b, &changes, async {
        assert!(b.fetch_account_sync_from("device-a").await.is_err());
    });
    assert!(failed.await.fetch_failed);
    a_docs.reachable(usize::MAX);
    let fetched = marked(&b, &changes, async {
        b.fetch_account_sync_from("device-a").await.expect("fetch");
    })
    .await;
    assert!(!fetched.no_peers && !fetched.fetch_failed);

    b_docs.writes_down.store(true, Ordering::SeqCst);
    let unwritten = marked(&b, &changes, async {
        trust(&b, 1).await;
    });
    assert!(unwritten.await.pending_writes);
    b_docs.writes_down.store(false, Ordering::SeqCst);
    let resent = marked(&b, &changes, async {
        b.resend_account_sync_items().await.expect("resend");
    });
    assert_eq!(resent.await, AccountSyncStatus::default());
}

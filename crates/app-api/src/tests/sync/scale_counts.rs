//! #1239(T7・AC-7、ADR 0052 の完了条件): replica と projection の件数を 1,000 / 10,000 / 100,000 にしても、定期処理・
//! 利用者の操作・表示・新着の受信の各操作が読む docs の entry 数と、projection の読み書きの量が増えないことを確かめる。
//! docs は返した record と key の数で、projection は SQLite の仮想機械が実行した命令の数で数える(命令の数は読み書きした
//! 行の数とともに増え、B-tree の深さには依存しない)。所要時間の閾値は使わない。
//!
//! 大部分の entry は、実際の key と同じ形の中身の無い entry(時系列の索引・object・follow の edge・プロフィールの索引)で埋める。
//! projection にも、同じ数の行(時系列の索引の埋め草と同じ object)を置く。表示や窓に入る新しい側だけに、署名つきの投稿を置く。
//! 操作が埋め草の側を読むと、読んだ量が件数とともに増える。
//! docs は key の範囲を索引で読む double(iroh-docs と同じく、読んだ entry の数だけ手間がかかる)を使う。

use super::range_reconcile::{BASE_TIME, TestPost, put_post_at};
use super::*;
use crate::service::catch_up_replica_window;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};

const SIZES: [usize; 3] = [1_000, 10_000, 100_000];
/// 新しい側に置く、署名つきの投稿の数。
const REAL_POSTS: usize = 30;
/// double の書き込みの名義。
const DOCS_AUTHOR: &str = "4444444444444444444444444444444444444444444444444444444444444444";

/// replica ごとの、key の順の record。
type Replicas = HashMap<String, BTreeMap<String, Vec<u8>>>;

/// key の範囲を索引で読む docs。record は replica ごとの key の順の map に置き、prefix の読み出しは範囲で読む。
/// `emit` が立っている間の書き込みは、相手の peer から届いた entry として覚え、`flush` で購読へ通知する(新着の受信。
/// 同期では entry と本体がそろってから通知が届くので、書き終えてからまとめて通知する)。
#[derive(Clone)]
struct IndexedDocsSync {
    replicas: Arc<TokioMutex<Replicas>>,
    returned: Arc<AtomicUsize>,
    emit: Arc<AtomicBool>,
    pending: Arc<std::sync::Mutex<Vec<kukuri_docs_sync::DocEvent>>>,
    events: tokio::sync::broadcast::Sender<kukuri_docs_sync::DocEvent>,
}

impl Default for IndexedDocsSync {
    fn default() -> Self {
        Self {
            replicas: Arc::default(),
            returned: Arc::default(),
            emit: Arc::default(),
            pending: Arc::default(),
            events: tokio::sync::broadcast::channel(64).0,
        }
    }
}

impl IndexedDocsSync {
    fn returned(&self) -> usize {
        self.returned.load(AtomicOrdering::SeqCst)
    }

    fn reset(&self) {
        self.returned.store(0, AtomicOrdering::SeqCst);
    }

    fn notify(&self, replica_id: &ReplicaId, key: &str, value: &[u8]) {
        if !self.emit.load(AtomicOrdering::SeqCst) {
            return;
        }
        self.pending
            .lock()
            .expect("pending events")
            .push(kukuri_docs_sync::DocEvent {
                replica_id: replica_id.clone(),
                key: key.to_string(),
                content_hash: kukuri_docs_sync::value_hash(value),
                source_peer: Some("remote-peer".into()),
                docs_author: Some(DOCS_AUTHOR.to_string()),
            });
    }

    /// 覚えた entry を、書いた順に購読へ通知する。
    fn flush(&self) {
        for event in std::mem::take(&mut *self.pending.lock().expect("pending events")) {
            // 購読が無いときの送信の失敗は無視する。
            let _ = self.events.send(event);
        }
    }

    fn record(key: &str, value: &[u8]) -> kukuri_docs_sync::DocRecord {
        kukuri_docs_sync::DocRecord {
            key: key.to_string(),
            value: value.to_vec(),
            content_hash: kukuri_docs_sync::value_hash(value),
            content_len: value.len() as u64,
            docs_author: Some(DOCS_AUTHOR.to_string()),
        }
    }

    /// `prefix` で始まる key の範囲(昇順)。
    fn prefix_range<'a>(
        map: &'a BTreeMap<String, Vec<u8>>,
        prefix: &'a str,
    ) -> impl DoubleEndedIterator<Item = (&'a String, &'a Vec<u8>)> + 'a {
        // key は ASCII なので、prefix の後ろに最大の文字を付けた値までが prefix の範囲になる。
        map.range(prefix.to_string()..format!("{prefix}{}", char::MAX))
    }
}

#[async_trait]
impl DocsSync for IndexedDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.replicas
            .lock()
            .await
            .entry(replica_id.as_str().to_string())
            .or_default();
        Ok(())
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        let mut replicas = self.replicas.lock().await;
        let map = replicas.entry(replica_id.as_str().to_string()).or_default();
        match op {
            DocOp::SetJson { key, value } => {
                let value = serde_json::to_vec(&value)?;
                self.notify(replica_id, key.as_str(), value.as_slice());
                map.insert(key, value);
            }
            DocOp::SetBytes { key, value } => {
                self.notify(replica_id, key.as_str(), value.as_slice());
                map.insert(key, value);
            }
            DocOp::DeletePrefix { prefix } => {
                let keys = Self::prefix_range(map, prefix.as_str())
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                for key in keys {
                    map.remove(&key);
                }
            }
        }
        Ok(())
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        _policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        let replicas = self.replicas.lock().await;
        let Some(map) = replicas.get(replica_id.as_str()) else {
            return Ok(Vec::new());
        };
        let records = match query {
            DocQuery::Exact(key) => map
                .get(&key)
                .map(|value| vec![Self::record(&key, value)])
                .unwrap_or_default(),
            DocQuery::Prefix(prefix) => Self::prefix_range(map, prefix.as_str())
                .map(|(key, value)| Self::record(key, value))
                .collect(),
            DocQuery::All => map
                .iter()
                .map(|(key, value)| Self::record(key, value))
                .collect(),
        };
        self.returned
            .fetch_add(records.len(), AtomicOrdering::SeqCst);
        Ok(records)
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        let replicas = self.replicas.lock().await;
        let Some(map) = replicas.get(replica_id.as_str()) else {
            return Ok(kukuri_docs_sync::DocKeyPage::default());
        };
        let entry = |(key, value): (&String, &Vec<u8>)| kukuri_docs_sync::DocKeyEntry {
            key: key.clone(),
            content_hash: kukuri_docs_sync::value_hash(value),
            content_len: value.len() as u64,
            docs_author: Some(DOCS_AUTHOR.to_string()),
        };
        let range = Self::prefix_range(map, query.prefix.as_str());
        let entries: Vec<_> = match query.order {
            kukuri_docs_sync::DocKeyOrder::Ascending => {
                range.take(query.limit).map(entry).collect()
            }
            kukuri_docs_sync::DocKeyOrder::Descending => {
                range.rev().take(query.limit).map(entry).collect()
            }
        };
        self.returned
            .fetch_add(entries.len(), AtomicOrdering::SeqCst);
        Ok(kukuri_docs_sync::DocKeyPage {
            reached_limit: query.limit > 0 && entries.len() >= query.limit,
            entries,
        })
    }

    async fn query_replica_keys_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        if docs_author != DOCS_AUTHOR {
            return Ok(kukuri_docs_sync::DocKeyPage::default());
        }
        self.query_replica_keys(replica_id, query).await
    }

    async fn local_docs_author(&self) -> Result<Option<String>> {
        Ok(Some(DOCS_AUTHOR.to_string()))
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        if docs_author != DOCS_AUTHOR {
            return Ok(None);
        }
        Ok(self
            .query_replica_with_policy(replica_id, DocQuery::Exact(key.to_string()), policy)
            .await?
            .into_iter()
            .next())
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        let replica_id = replica_id.clone();
        let stream = tokio_stream::wrappers::BroadcastStream::new(self.events.subscribe());
        Ok(Box::pin(futures_util::StreamExt::filter_map(
            stream,
            move |item| {
                let replica_id = replica_id.clone();
                async move {
                    item.ok()
                        .filter(|event| event.replica_id == replica_id)
                        .map(Ok)
                }
            },
        )))
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }
}

fn filler_id(index: usize) -> String {
    format!("{index:064x}")
}

/// 埋め草の時刻。署名つきの投稿より古い側に、1 秒ずつ並べる。
fn filler_time(index: usize) -> i64 {
    BASE_TIME - 1 - index as i64
}

struct Fixture {
    app: AppService,
    docs_sync: Arc<IndexedDocsSync>,
    /// projection(SQLite)の仮想機械が実行した命令の数。
    steps: Arc<AtomicU64>,
    topic: TopicId,
    posts: Vec<TestPost>,
    remote_pubkey: String,
    author_keys: KukuriKeys,
}

/// 1 つの操作で数えた量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reads {
    /// docs が返した record と key の数。
    docs: usize,
    /// projection の読み書きで SQLite が実行した命令の数。
    projection: u64,
}

impl Fixture {
    fn counted(&self) -> Reads {
        Reads {
            docs: self.docs_sync.returned(),
            projection: self.steps.load(AtomicOrdering::SeqCst),
        }
    }

    /// 背景の仕事(購読タスクの起動時の追いつきなど)が落ち着くまで待つ。読んだ量が 300ms 変わらなければ落ち着いたとみなす。
    /// 300ms より遅れて始まる背景の読み出し(最小間隔を空けた追いつき、tick ごとの処理)は数えない。double が docs の通知を
    /// 出すのは新着の受信の操作の間だけで、その通知は追いつきを依頼しない(反映済みの object を指す entry だけが続く)ので、
    /// 測定の間にそうした読み出しは起きない。
    async fn settle(&self) {
        let mut last = self.counted();
        for _ in 0..200 {
            sleep(Duration::from_millis(300)).await;
            let now = self.counted();
            if now == last {
                return;
            }
            last = now;
        }
        panic!("the background work did not settle");
    }

    /// 落ち着いた後に `operation` を実行し、背景の続きが落ち着くまでに docs から読んだ record と key の数と、projection の
    /// 読み書きの命令の数を返す。
    async fn reads_of<F, Fut>(&self, operation: F) -> Reads
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        self.settle().await;
        self.docs_sync.reset();
        self.steps.store(0, AtomicOrdering::SeqCst);
        operation().await;
        // 操作が背景で起こす読み出し(対象の key の確認など)も含めて数える。数え終わる前に終わるかどうかで揺れないように。
        self.settle().await;
        self.counted()
    }
}

/// projection の埋め草の行。時系列の索引の埋め草と同じ object を指す。
fn filler_row(
    topic: &TopicId,
    replica: &ReplicaId,
    author: &str,
    index: usize,
) -> ObjectProjectionRow {
    let id = filler_id(index);
    ObjectProjectionRow {
        object_id: EnvelopeId::from(id.as_str()),
        topic_id: topic.as_str().to_string(),
        channel_id: "public".into(),
        author_pubkey: author.to_string(),
        created_at: filler_time(index),
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::InlineText {
            text: "filler".into(),
        },
        content: Some("filler".into()),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: replica.clone(),
        source_key: stable_key("objects", &format!("{id}/state")),
        source_envelope_id: EnvelopeId::from(id.as_str()),
        source_blob_hash: None,
        source_docs_author: None,
        derived_at: 0,
        projection_version: kukuri_store::VERIFIED_OBJECT_PROJECTION_VERSION,
    }
}

/// topic の replica と、ある著者の author replica に、`size` 件ずつの埋め草を置く。projection にも `size` 行を置く。
async fn fixture(size: usize) -> Fixture {
    let docs_sync = Arc::new(IndexedDocsSync::default());
    let steps = Arc::new(AtomicU64::new(0));
    let store = Arc::new(
        kukuri_store::SqliteStore::connect_memory_counting_vm_steps(steps.clone())
            .await
            .expect("counting sqlite store"),
    );
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let topic = TopicId::new(format!("kukuri:topic:scale-counts-{size}").as_str());
    let replica = topic_replica_id(topic.as_str());
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let author_replica = author_replica_id(remote_pubkey.as_str());
    let local_keys = generate_keys();
    {
        let mut replicas = docs_sync.replicas.lock().await;
        let topic_map = replicas.entry(replica.as_str().to_string()).or_default();
        for index in 0..size {
            let id = filler_id(index);
            let object_id = EnvelopeId::from(id.as_str());
            topic_map.insert(
                stable_key(
                    "indexes/timeline",
                    &format!("{}/{id}", timeline_sort_key(filler_time(index), &object_id)),
                ),
                b"{}".to_vec(),
            );
            topic_map.insert(
                stable_key("objects", &format!("{id}/state")),
                b"{}".to_vec(),
            );
            topic_map.insert(
                stable_key("objects", &format!("{id}/envelope")),
                b"{}".to_vec(),
            );
        }
        let author_map = replicas
            .entry(author_replica.as_str().to_string())
            .or_default();
        for index in 0..size {
            let id = filler_id(index);
            let object_id = EnvelopeId::from(id.as_str());
            author_map.insert(stable_key("graph/follows", id.as_str()), b"{}".to_vec());
            author_map.insert(
                stable_key(
                    "indexes/profile",
                    &format!("{}/{id}", timeline_sort_key(filler_time(index), &object_id)),
                ),
                b"{}".to_vec(),
            );
        }
        // 閲覧者を指す follow の key。author の追いつきは、この key(と block の key)だけを読む。
        author_map.insert(
            stable_key("graph/follows", local_keys.public_key_hex().as_str()),
            b"{}".to_vec(),
        );
    }
    let author_keys = generate_keys();
    let filler_author = generate_keys().public_key_hex();
    store
        .put_object_projections(
            (0..size)
                .map(|index| filler_row(&topic, &replica, filler_author.as_str(), index))
                .collect(),
        )
        .await
        .expect("filler projection rows");
    let mut posts = Vec::new();
    for index in 0..REAL_POSTS {
        posts.push(
            put_post_at(
                docs_sync.as_ref(),
                &replica,
                &author_keys,
                &topic,
                BASE_TIME + index as i64,
                format!("post {index}").as_str(),
                None,
            )
            .await,
        );
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        local_keys,
    );
    // 閲覧者は著者の docs author を知っている(profile の tag から覚えた状態)。
    store
        .put_author_docs_author(remote_pubkey.as_str(), DOCS_AUTHOR)
        .await
        .expect("learn the docs author");
    // 購読はまだ起動しない。topic とプロフィールを初めて開く操作(購読タスクと author 購読の起動と、その背景の続き)も測る。
    Fixture {
        app,
        docs_sync,
        steps,
        topic,
        posts,
        remote_pubkey,
        author_keys,
    }
}

/// 各操作が 3 つの件数で読んだ量。操作の名前ごとに、件数の順に並ぶ。
async fn reads_by_size() -> BTreeMap<&'static str, Vec<Reads>> {
    let mut reads: BTreeMap<&'static str, Vec<Reads>> = BTreeMap::new();
    for size in SIZES {
        let fixture = fixture(size).await;
        let app = &fixture.app;
        let topic = fixture.topic.as_str();
        let replica = topic_replica_id(topic);
        let deep = 500;
        let measured = [
            (
                // 画面の取得と同時に走らせると、どちらが先に同じ object を反映するかで読む量が 1 件揺れる。起動だけを測る。
                "topic subscription start",
                fixture
                    .reads_of(|| async {
                        display_topic(app, topic)
                            .await
                            .expect("subscribe the topic");
                    })
                    .await,
            ),
            (
                "profile first open (author subscription start)",
                fixture
                    .reads_of(|| async {
                        app.list_profile_timeline(fixture.remote_pubkey.as_str(), None, 20)
                            .await
                            .expect("first profile timeline");
                    })
                    .await,
            ),
            (
                "subscription catch-up",
                fixture
                    .reads_of(|| async {
                        catch_up_replica_window(
                            &app.services,
                            topic,
                            &replica,
                            DocFetchPolicy::LocalOnly,
                            true,
                        )
                        .await
                        .expect("catch up");
                    })
                    .await,
            ),
            (
                "timeline head page",
                fixture
                    .reads_of(|| async {
                        app.list_timeline(topic, None, 20).await.expect("head page");
                    })
                    .await,
            ),
            (
                // 相手の peer から、新しい投稿 1 件の entry(object・envelope・時系列の索引)が届く。
                "receive a new post",
                fixture
                    .reads_of(|| async {
                        fixture.docs_sync.emit.store(true, AtomicOrdering::SeqCst);
                        put_post_at(
                            fixture.docs_sync.as_ref(),
                            &replica,
                            &fixture.author_keys,
                            &fixture.topic,
                            BASE_TIME + REAL_POSTS as i64,
                            "a new post",
                            None,
                        )
                        .await;
                        fixture.docs_sync.emit.store(false, AtomicOrdering::SeqCst);
                        fixture.docs_sync.flush();
                    })
                    .await,
            ),
            (
                "timeline older page",
                fixture
                    .reads_of(|| async {
                        app.list_timeline(
                            topic,
                            Some(TimelineCursor {
                                created_at: filler_time(deep),
                                object_id: EnvelopeId::from(filler_id(deep).as_str()),
                            }),
                            20,
                        )
                        .await
                        .expect("older page");
                    })
                    .await,
            ),
            (
                "reaction",
                fixture
                    .reads_of(|| async {
                        let target = fixture.posts.last().expect("a real post");
                        app.toggle_reaction(
                            topic,
                            target.object_id.as_str(),
                            ReactionKeyV1::Emoji {
                                emoji: "👍".into()
                            },
                            Some(ChannelRef::Public),
                        )
                        .await
                        .expect("toggle reaction");
                    })
                    .await,
            ),
            (
                "author catch-up",
                fixture
                    .reads_of(|| async {
                        catch_up_author_state(
                            &app.services,
                            app.current_author_pubkey().as_str(),
                            fixture.remote_pubkey.as_str(),
                            DocFetchPolicy::LocalOnly,
                        )
                        .await
                        .expect("author catch-up");
                    })
                    .await,
            ),
            (
                "profile timeline",
                fixture
                    .reads_of(|| async {
                        app.list_profile_timeline(fixture.remote_pubkey.as_str(), None, 20)
                            .await
                            .expect("profile timeline");
                    })
                    .await,
            ),
        ];
        for (name, count) in measured {
            reads.entry(name).or_default().push(count);
        }
        fixture.app.shutdown().await;
    }
    reads
}

// 定期処理(購読タスクの窓の追いつき、author 購読の追いつき)、表示(タイムラインの新しい側・遡ったページ、
// プロフィールのタイムライン)、利用者の操作(reaction)、新着の受信が読む docs の量と、projection の読み書きの量は、
// replica と projection の件数を 1,000 / 10,000 / 100,000 にしても増えない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_do_not_grow_from_one_thousand_to_one_hundred_thousand_entries() {
    let reads = reads_by_size().await;
    // タイムラインの新しい側のページは projection から読み、docs を読まない(0 件)。ほかの操作は docs を読む。
    // どの操作も projection を読み書きする。
    assert!(
        reads
            .iter()
            .filter(|(operation, _)| **operation != "timeline head page")
            .all(|(_, counts)| counts[0].docs > 0),
        "{reads:?}"
    );
    assert!(
        reads.values().all(|counts| counts[0].projection > 0),
        "{reads:?}"
    );
    for (operation, counts) in &reads {
        assert!(
            counts.windows(2).all(|pair| pair[0] == pair[1]),
            "{operation}: docs records read and projection steps must not grow with the size ({SIZES:?} -> {counts:?})"
        );
    }
}

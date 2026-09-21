//! #1239(T6-2 の独立監査 B-1〜B-4): 誰でも書ける author replica に他の名義が置いた key で、プロフィールの投稿を隠せないこと。
//! 自分の replica に後から届いた、索引の無い投稿が読めること。

use super::*;
use crate::service::profile_timeline_support::{
    backfill_own_profile_index_with, index_own_profile_key, restart_own_profile_index_backfill,
};

const OWNER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER: &str = "9999999999999999999999999999999999999999999999999999999999999999";
const BASE_TIME: i64 = 1_700_000_000;

/// 著者の名義(`OWNER`)の docs と、他の名義(`OTHER`)の docs を 1 つの replica として見せる docs。
/// 書き込みは著者の名義に入る。他の名義の entry は `other` へ直接書く。
#[derive(Clone)]
struct TwoAuthorDocsSync {
    owner: MemoryDocsSync,
    other: MemoryDocsSync,
}

impl Default for TwoAuthorDocsSync {
    fn default() -> Self {
        Self {
            owner: MemoryDocsSync::with_docs_author(OWNER),
            other: MemoryDocsSync::with_docs_author(OTHER),
        }
    }
}

impl TwoAuthorDocsSync {
    fn by(&self, docs_author: &str) -> Option<&MemoryDocsSync> {
        match docs_author {
            OWNER => Some(&self.owner),
            OTHER => Some(&self.other),
            _ => None,
        }
    }
}

#[async_trait]
impl DocsSync for TwoAuthorDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.owner.open_replica(replica_id).await?;
        self.other.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.owner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        // iroh-docs と同じく、同じ key の entry は docs author の昇順(他の名義が先)。
        let mut records = self
            .other
            .query_replica_with_policy(replica_id, query.clone(), policy)
            .await?;
        records.extend(
            self.owner
                .query_replica_with_policy(replica_id, query, policy)
                .await?,
        );
        Ok(records)
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        let mut entries = self
            .other
            .query_replica_keys(replica_id, query.clone())
            .await?
            .entries;
        entries.extend(
            self.owner
                .query_replica_keys(replica_id, query.clone())
                .await?
                .entries,
        );
        entries.sort_by(|left, right| match query.order {
            kukuri_docs_sync::DocKeyOrder::Ascending => left.key.cmp(&right.key),
            kukuri_docs_sync::DocKeyOrder::Descending => right.key.cmp(&left.key),
        });
        let reached_limit = query.limit > 0 && entries.len() >= query.limit;
        entries.truncate(query.limit);
        Ok(kukuri_docs_sync::DocKeyPage {
            entries,
            reached_limit,
        })
    }

    async fn query_replica_keys_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        match self.by(docs_author) {
            Some(docs) => docs.query_replica_keys(replica_id, query).await,
            None => Ok(kukuri_docs_sync::DocKeyPage::default()),
        }
    }

    async fn local_docs_author(&self) -> Result<Option<String>> {
        Ok(Some(OWNER.to_string()))
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<kukuri_docs_sync::DocRecord>> {
        match self.by(docs_author) {
            Some(docs) => {
                docs.query_replica_by_author(replica_id, docs_author, key, policy)
                    .await
            }
            None => Ok(None),
        }
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.owner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.owner.import_peer_ticket(ticket).await
    }
}

struct Fixture {
    app: AppService,
    store: Arc<MemoryStore>,
    docs_sync: Arc<TwoAuthorDocsSync>,
    keys: KukuriKeys,
    pubkey: String,
}

/// 閲覧者の app と、著者の鍵。閲覧者は著者の docs author(`OWNER`)を知っている。
async fn fixture() -> Fixture {
    let docs_sync = Arc::new(TwoAuthorDocsSync::default());
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
    let keys = generate_keys();
    let pubkey = keys.public_key_hex();
    store
        .put_author_docs_author(pubkey.as_str(), OWNER)
        .await
        .expect("learn the docs author");
    Fixture {
        app,
        store,
        docs_sync,
        keys,
        pubkey,
    }
}

/// 著者の投稿を書き、object id を返す。`indexed` が偽なら、索引を書く前の版の投稿にする。
async fn put_post(fixture: &Fixture, created_at: i64, indexed: bool) -> String {
    let object_id = EnvelopeId::from(generate_keys().public_key_hex().as_str());
    let envelope = build_profile_post_envelope(
        &fixture.keys,
        &KukuriProfilePostEnvelopeContentV1 {
            author_pubkey: Pubkey::from(fixture.pubkey.as_str()),
            profile_topic_id: author_profile_topic_id(fixture.pubkey.as_str()),
            published_topic_id: TopicId::new("kukuri:topic:profile-attacks"),
            object_id: object_id.clone(),
            created_at,
            object_kind: "post".into(),
            content: format!("post {created_at}"),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        },
    )
    .expect("envelope");
    let post = parse_profile_post(&envelope).expect("parse").expect("post");
    persist_profile_post_doc(fixture.docs_sync.as_ref(), &post, &envelope)
        .await
        .expect("persist");
    if !indexed {
        fixture
            .docs_sync
            .owner
            .apply_doc_op(
                &author_replica_id(fixture.pubkey.as_str()),
                DocOp::DeletePrefix {
                    prefix: stable_key(
                        "indexes/profile",
                        &timeline_sort_key(created_at, &object_id),
                    ),
                },
            )
            .await
            .expect("remove the index entry");
    }
    object_id.as_str().to_string()
}

/// 他の名義で key を置く。
async fn put_other(fixture: &Fixture, key: String) {
    fixture
        .docs_sync
        .other
        .apply_doc_op(
            &author_replica_id(fixture.pubkey.as_str()),
            DocOp::SetJson {
                key,
                value: serde_json::json!({ "version": 1 }),
            },
        )
        .await
        .expect("write as another docs author");
}

async fn visible(fixture: &Fixture) -> Vec<String> {
    let mut ids = Vec::new();
    let mut cursor = None;
    for _ in 0..100 {
        let page = fixture
            .app
            .list_profile_timeline(fixture.pubkey.as_str(), cursor, 3)
            .await
            .expect("profile timeline");
        ids.extend(page.items.into_iter().map(|item| item.object_id));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return ids,
        }
    }
    panic!("the profile timeline did not end");
}

// B-1: 他の名義が置いた「補い終えた」印では、索引の無い投稿を隠せない。
#[tokio::test]
async fn a_completion_marker_by_another_docs_author_does_not_hide_legacy_posts() {
    let fixture = fixture().await;
    let mut ids = Vec::new();
    for index in 0..3 {
        ids.push(put_post(&fixture, BASE_TIME + index, false).await);
    }
    ids.reverse();
    put_other(&fixture, "indexes/profile-complete".into()).await;

    assert_eq!(visible(&fixture).await, ids);
}

// B-2: 補完を読み終えた後に届いた、索引の無い自分の投稿は、event で索引が足され、取りこぼしの後は補完のやり直しで拾われる。
#[tokio::test]
async fn own_legacy_posts_arriving_after_the_backfill_are_indexed() {
    let fixture = fixture().await;
    let projection = fixture.store.as_ref();
    backfill_own_profile_index_with(
        fixture.docs_sync.as_ref(),
        projection,
        fixture.pubkey.as_str(),
        16,
        64,
    )
    .await
    .expect("backfill an empty replica");
    let by_event = put_post(&fixture, BASE_TIME + 10, false).await;
    let by_restart = put_post(&fixture, BASE_TIME + 20, false).await;
    // 補い終えた印は著者の名義にあるので、閲覧者は旧 record を合わせない。索引が無い間は見えない。
    assert!(visible(&fixture).await.is_empty());

    // 1 件は、その key の event で索引が足される。
    assert!(
        index_own_profile_key(
            fixture.docs_sync.as_ref(),
            fixture.pubkey.as_str(),
            stable_key("profile/posts", by_event.as_str()).as_str(),
        )
        .await
        .expect("index by event")
    );
    // もう 1 件は、event を取りこぼした後の補完のやり直しで拾われる。
    restart_own_profile_index_backfill(projection, fixture.pubkey.as_str())
        .await
        .expect("restart");
    let written = backfill_own_profile_index_with(
        fixture.docs_sync.as_ref(),
        projection,
        fixture.pubkey.as_str(),
        16,
        64,
    )
    .await
    .expect("backfill again");
    assert_eq!(written, 1);

    assert_eq!(visible(&fixture).await, vec![by_restart, by_event]);
}

// B-3: 同じ object id の偽の索引の entry(他の名義、新しい時刻)で、本物の投稿を隠せない。
#[tokio::test]
async fn a_forged_index_entry_for_the_same_object_does_not_hide_the_post() {
    let fixture = fixture().await;
    let mut ids = Vec::new();
    for index in 0..3 {
        ids.push(put_post(&fixture, BASE_TIME + index, true).await);
    }
    ids.reverse();
    let target = EnvelopeId::from(ids[1].as_str());
    put_other(
        &fixture,
        stable_key(
            "indexes/profile",
            &format!(
                "{}/{}",
                timeline_sort_key(BASE_TIME + 100, &target),
                target.as_str()
            ),
        ),
    )
    .await;

    assert_eq!(visible(&fixture).await, ids);
    // docs author を知らない閲覧者(名義を問わない索引の読み出し)でも、読んだ印を検証の後で付けるので隠れない。
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let stranger = Fixture {
        app: app_service_from_dependencies(
            store.clone(),
            store.clone(),
            transport.clone(),
            transport,
            fixture.docs_sync.clone(),
            Arc::new(MemoryBlobService::default()),
            generate_keys(),
        ),
        store,
        docs_sync: fixture.docs_sync.clone(),
        keys: fixture.keys.clone(),
        pubkey: fixture.pubkey.clone(),
    };
    assert_eq!(visible(&stranger).await, ids);
}

// B-4: 他の名義が置いた、未来の時刻の索引の key(存在しない object)で、最初のページを空にできない。
#[tokio::test]
async fn future_index_keys_by_another_docs_author_do_not_empty_the_first_page() {
    let fixture = fixture().await;
    let mut ids = Vec::new();
    for index in 0..3 {
        ids.push(put_post(&fixture, BASE_TIME + index, true).await);
    }
    ids.reverse();
    for index in 0..100 {
        let id = EnvelopeId::from(format!("{index:064x}").as_str());
        put_other(
            &fixture,
            stable_key(
                "indexes/profile",
                &format!(
                    "{}/{}",
                    timeline_sort_key(BASE_TIME + 1_000_000 + index, &id),
                    id.as_str()
                ),
            ),
        )
        .await;
    }

    let first = fixture
        .app
        .list_profile_timeline(fixture.pubkey.as_str(), None, 3)
        .await
        .expect("first page");
    assert_eq!(
        first
            .items
            .into_iter()
            .map(|item| item.object_id)
            .collect::<Vec<_>>(),
        ids
    );
}

//! #1221 R5-C: author の現在値とプロフィールのページを、手元の次に有界な provider から読む。
//! provider の履歴(edge・投稿)を 10 倍にしても、provider から読む量が変わらないことを固定する。

use super::*;
use crate::service::author_state_support::AUTHOR_EDGE_KEYS;
use crate::service::profile_timeline_support::profile_timeline_page;
use kukuri_core::DomeMovePhaseV1;

const TOPIC: &str = "kukuri:topic:author-remote-reads";
const BASE_TIME: i64 = 1_700_000_000;

fn app_over(docs_sync: Arc<CountingDocsSync>, keys: KukuriKeys) -> (AppService, Arc<MemoryStore>) {
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

async fn put_profile(docs_sync: &dyn DocsSync, keys: &KukuriKeys) {
    let envelope = build_profile_envelope_with_docs_author(
        keys,
        &KukuriProfileEnvelopeContentV1 {
            author_pubkey: keys.public_key(),
            name: Some("provider".into()),
            display_name: None,
            about: None,
            picture_asset: None,
            nip05: None,
        },
        None,
    )
    .expect("profile envelope");
    let profile = parse_profile(&envelope)
        .expect("parse profile")
        .expect("profile");
    persist_profile_doc(docs_sync, &profile, &envelope)
        .await
        .expect("persist profile");
}

// #1637: 空のclient cacheと、応答しないauthor/providerを再現する。
struct ProfileReadSource {
    inner: Arc<CountingDocsSync>,
    readers: Vec<Arc<dyn DocsSync>>,
    stalled: Option<ReplicaId>,
}

#[async_trait]
impl DocsSync for ProfileReadSource {
    fn remote_reader_id(&self) -> Option<String> {
        Some(
            if self.stalled.is_some() {
                "stalled"
            } else {
                "ready"
            }
            .into(),
        )
    }

    async fn open_replica(&self, _: &ReplicaId) -> Result<()> {
        panic!("profile reads must not open a namespace")
    }

    async fn apply_doc_op(&self, _: &ReplicaId, _: DocOp) -> Result<()> {
        panic!("profile reads must not write docs")
    }

    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        assert!(
            matches!(&query, DocQuery::Exact(key) if key == "profile/latest" || key.starts_with("envelopes/"))
        );
        let result = self
            .inner
            .query_replica_with_policy(replica, query, policy)
            .await;
        if policy == DocFetchPolicy::LocalThenRemote
            && self
                .stalled
                .as_ref()
                .is_some_and(|stalled| stalled == replica || stalled.as_str() == "*")
        {
            std::future::pending::<()>().await;
        }
        result
    }

    async fn subscribe_replica(&self, _: &ReplicaId) -> Result<kukuri_docs_sync::DocEventStream> {
        panic!("profile reads must not subscribe")
    }

    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        panic!("profile reads must not import")
    }

    async fn remote_readers(
        &self,
        _: &ReplicaId,
        _: Option<[u8; 32]>,
        _: Vec<SeedPeer>,
    ) -> Result<Vec<Arc<dyn DocsSync>>> {
        Ok(self.readers.clone())
    }
}

fn profile_viewer(readers: Vec<Arc<dyn DocsSync>>) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        Arc::new(ProfileReadSource {
            inner: Arc::default(),
            readers,
            stalled: None,
        }),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    (app, store)
}

async fn wait_for_profile(store: &MemoryStore, author: &str) {
    loop {
        if store.get_profile(author).await.expect("profile").is_some() {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(start_paused = true)]
async fn missing_profile_retries_after_a_startup_miss_without_waiting_ten_minutes() {
    let author = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    let (app, store) = profile_viewer(vec![provider.clone()]);
    let request = (author.public_key_hex(), None);
    app.request_missing_profiles([request.clone()]).await;
    sleep(Duration::from_secs(6)).await;
    assert!(
        !provider.queries().await.is_empty(),
        "the first read missed"
    );
    put_profile(provider.as_ref(), &author).await;
    app.request_missing_profiles([request]).await;
    timeout(
        Duration::from_secs(1),
        wait_for_profile(&store, &author.public_key_hex()),
    )
    .await
    .expect("retry after provider becomes ready");
    app.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_author_does_not_block_the_next_missing_profile() {
    let stalled = generate_keys();
    let ready = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    put_profile(provider.as_ref(), &ready).await;
    let (app, store) = profile_viewer(vec![Arc::new(ProfileReadSource {
        inner: provider.clone(),
        readers: Vec::new(),
        stalled: Some(author_replica_id(&stalled.public_key_hex())),
    })]);
    let requests = [
        (stalled.public_key_hex(), None),
        (ready.public_key_hex(), None),
    ];
    app.request_missing_profiles(requests.clone()).await;
    timeout(
        Duration::from_secs(1),
        wait_for_profile(&store, &ready.public_key_hex()),
    )
    .await
    .expect("the responsive author is not queued behind the stalled one");
    app.request_missing_profiles(requests).await;
    sleep(Duration::from_secs(6)).await;
    let stalled_reads = provider
        .queries()
        .await
        .into_iter()
        .filter(|(replica, _)| replica == author_replica_id(&stalled.public_key_hex()).as_str())
        .count();
    assert_eq!(stalled_reads, 1, "a pending author is not enqueued again");
    app.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_profile_provider_leaves_time_for_the_next_provider() {
    let author = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    put_profile(provider.as_ref(), &author).await;
    let (app, store) = profile_viewer(vec![
        Arc::new(ProfileReadSource {
            inner: Arc::default(),
            readers: Vec::new(),
            stalled: Some(author_replica_id(&author.public_key_hex())),
        }),
        provider,
    ]);
    timeout(
        Duration::from_secs(20),
        crate::service::author_state_support::hydrate_author_profile(
            &app.services,
            &app.current_author_pubkey(),
            &author.public_key_hex(),
            None,
        ),
    )
    .await
    .expect("the second provider gets a share of the deadline")
    .expect("hydrate");
    assert!(
        store
            .get_profile(&author.public_key_hex())
            .await
            .expect("profile")
            .is_some()
    );
    app.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn missing_profile_work_is_bounded_deduplicated_and_cancelled_at_shutdown() {
    let provider = Arc::new(CountingDocsSync::default());
    let (app, _) = profile_viewer(vec![Arc::new(ProfileReadSource {
        inner: provider.clone(),
        readers: Vec::new(),
        stalled: Some(ReplicaId::new("*")),
    })]);
    let requests = (0..80)
        .map(|_| (generate_keys().public_key_hex(), None))
        .collect::<Vec<_>>();
    app.request_missing_profiles(requests.clone()).await;
    sleep(Duration::from_secs(1)).await;
    app.request_missing_profiles(requests).await;
    assert_eq!(provider.queries().await.len(), 4, "only four reads execute");
    app.shutdown().await;
    sleep(Duration::from_secs(60)).await;
    assert_eq!(
        provider.queries().await.len(),
        4,
        "shutdown cancels running reads and queued work"
    );
}

async fn put_follow(docs_sync: &dyn DocsSync, keys: &KukuriKeys, target: &str) {
    let envelope =
        build_follow_edge_envelope(keys, &Pubkey::from(target), FollowEdgeStatus::Active)
            .expect("follow edge");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow")
        .expect("follow");
    persist_follow_edge_doc(docs_sync, &edge, &envelope)
        .await
        .expect("persist follow");
}

async fn put_profile_post(docs_sync: &dyn DocsSync, keys: &KukuriKeys, created_at: i64) -> String {
    let author = keys.public_key_hex();
    let object_id = EnvelopeId::from(generate_keys().public_key_hex().as_str());
    let envelope = build_profile_post_envelope(
        keys,
        &KukuriProfilePostEnvelopeContentV1 {
            author_pubkey: Pubkey::from(author.as_str()),
            profile_topic_id: author_profile_topic_id(author.as_str()),
            published_topic_id: TopicId::new(TOPIC),
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
    .expect("profile post envelope");
    let post = parse_profile_post(&envelope)
        .expect("parse profile post")
        .expect("profile post");
    persist_profile_post_doc(
        docs_sync,
        &author_replica_id(post.author_pubkey.as_str()),
        &post,
        &envelope,
    )
    .await
    .expect("persist profile post");
    object_id.as_str().to_string()
}

// 購読の開始時の反映は、手元に無い profile と自分を指す follow・block と、follow の窓(512 key)を provider から読む
// (#1221 R5-H、友達の友達の判定)。provider の follow の総数を 10 倍にしても読む量は同じで、窓の外の edge は読まない。
#[tokio::test]
async fn author_state_reads_the_current_keys_and_a_bounded_follow_window_from_a_provider() {
    let mut counts = Vec::new();
    for edges in [600, 6000] {
        let author = generate_keys();
        // 自分を指す edge の key が窓の外に並ぶ相手にして、窓の key の数を 2 回で揃える。
        let local = std::iter::repeat_with(generate_keys)
            .find(|keys| keys.public_key_hex().starts_with("ff"))
            .expect("local keys");
        let provider = Arc::new(CountingDocsSync::default());
        put_profile(provider.as_ref(), &author).await;
        put_follow(provider.as_ref(), &author, local.public_key_hex().as_str()).await;
        for _ in 0..edges {
            put_follow(
                provider.as_ref(),
                &author,
                generate_keys().public_key_hex().as_str(),
            )
            .await;
        }
        let docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
        let (app, store) = app_over(docs, local.clone());

        provider.reset_records_returned();
        hydrate_author_state(
            &app.services,
            local.public_key_hex().as_str(),
            author.public_key_hex().as_str(),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("local hydration");
        assert_eq!(
            provider.records_returned(),
            0,
            "LocalOnly does not read the provider"
        );

        hydrate_author_state(
            &app.services,
            local.public_key_hex().as_str(),
            author.public_key_hex().as_str(),
            DocFetchPolicy::LocalThenRemote,
        )
        .await
        .expect("remote hydration");
        counts.push(provider.records_returned());
        assert!(
            store
                .get_profile(author.public_key_hex().as_str())
                .await
                .expect("profile")
                .is_some(),
            "the profile comes from the provider"
        );
        let relationship = app
            .services
            .projection_store
            .get_author_relationship(
                local.public_key_hex().as_str(),
                author.public_key_hex().as_str(),
            )
            .await
            .expect("relationship")
            .expect("relationship row");
        assert!(relationship.followed_by, "the follow of me is reflected");
        assert_eq!(
            store
                .list_follow_edges_by_subject(author.public_key_hex().as_str())
                .await
                .expect("edges")
                .len(),
            AUTHOR_EDGE_KEYS + 1,
            "the follow window and the follow of me are restored, the rest is not"
        );
    }
    assert_eq!(counts[0], counts[1], "provider reads: {counts:?}");
}

// 手元のページが埋まらないプロフィールは provider から読む。provider の投稿を 10 倍にしても読む量は同じで、
// ページをたどると全行が新しい順に 1 回ずつ出る。
#[tokio::test]
async fn profile_pages_read_a_constant_amount_from_a_provider() {
    let mut counts = Vec::new();
    for posts in [100, 1000] {
        let author = generate_keys();
        let local = generate_keys();
        let provider = Arc::new(CountingDocsSync::default());
        let mut expected = Vec::new();
        for index in 0..posts {
            expected.push(put_profile_post(provider.as_ref(), &author, BASE_TIME + index).await);
        }
        expected.reverse();
        let docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
        let (app, _) = app_over(docs, local.clone());
        let author_pubkey = author.public_key_hex();

        provider.reset_records_returned();
        let first = profile_timeline_page(
            &app.services,
            author_pubkey.as_str(),
            None,
            None,
            20,
            &BTreeSet::new(),
        )
        .await
        .expect("first page");
        counts.push(provider.records_returned());
        assert_eq!(first.items.len(), 20);
        assert!(first.next_cursor.is_some());

        if posts == 100 {
            let mut seen = first
                .items
                .iter()
                .map(|item| item.object_id().as_str().to_string())
                .collect::<Vec<_>>();
            let mut cursor = first.next_cursor;
            while let Some(next) = cursor {
                let page = profile_timeline_page(
                    &app.services,
                    author_pubkey.as_str(),
                    None,
                    Some(next),
                    20,
                    &BTreeSet::new(),
                )
                .await
                .expect("next page");
                seen.extend(
                    page.items
                        .iter()
                        .map(|item| item.object_id().as_str().to_string()),
                );
                cursor = page.next_cursor;
            }
            assert_eq!(seen, expected, "every post once, newest first");
        }
    }
    assert_eq!(counts[0], counts[1], "provider reads: {counts:?}");
}

// 自分のプロフィールも、手元のページが埋まらないときだけ provider を読み、同じ account の別端末の投稿を出す
// (#1221 R5-H)。手元でページが埋まれば provider へ要求しない。
#[tokio::test]
async fn the_own_profile_reads_a_provider_only_when_the_local_page_is_short() {
    let local = generate_keys();
    let own = local.public_key_hex();
    let provider = Arc::new(CountingDocsSync::default());
    let other_device_post = put_profile_post(provider.as_ref(), &local, BASE_TIME).await;
    let docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
    let (app, _) = app_over(docs.clone(), local.clone());
    provider.reset_records_returned();
    let page = profile_timeline_page(
        &app.services,
        own.as_str(),
        None,
        None,
        20,
        &BTreeSet::new(),
    )
    .await
    .expect("own page");
    let ids = page
        .items
        .iter()
        .map(|item| item.object_id().as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![other_device_post]);

    put_profile_post(docs.as_ref(), &local, BASE_TIME + 1).await;
    provider.reset_records_returned();
    let page = profile_timeline_page(&app.services, own.as_str(), None, None, 1, &BTreeSet::new())
        .await
        .expect("own full page");
    assert_eq!(page.items.len(), 1);
    assert_eq!(provider.records_returned(), 0);
}

// Dome の移動 record(author replica の対象)は、手元に無ければ owner の provider から読み、署名を確かめる。
#[tokio::test]
async fn a_dome_move_record_is_read_from_the_owner_provider() {
    let owner_keys = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    let (owner, _) = app_over(provider.clone(), owner_keys.clone());
    let owner_pubkey = Pubkey::from(owner_keys.public_key_hex());
    let record = DomeMoveRecordV1 {
        move_id: "move-1".into(),
        owner_pubkey: owner_pubkey.clone(),
        source_instance_id: "source".into(),
        source_context: kukuri_core::SpatialContextV1::Topic {
            topic_id: TopicId::new(TOPIC),
        },
        source_generation: 1,
        target_instance_id: "target".into(),
        target_context: kukuri_core::SpatialContextV1::Topic {
            topic_id: TopicId::new("kukuri:topic:author-remote-reads-target"),
        },
        target_generation: 1,
        preset_ref: DomePresetRefV1 {
            preset_id: "preset".into(),
            owner_pubkey: owner_pubkey.clone(),
            revision: 1,
            manifest_blob_hash: "a".repeat(64),
            manifest_mime: "application/json".into(),
            manifest_bytes: 1,
        },
        phase: DomeMovePhaseV1::Preparing,
        failure_reason: None,
        updated_at: BASE_TIME,
    };
    owner
        .persist_dome_move_record(&record)
        .await
        .expect("persist move");

    let reader_docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
    let (reader, _) = app_over(reader_docs.clone(), generate_keys());
    provider.reset_records_returned();
    let read = reader
        .fetch_dome_move_record(owner_pubkey.as_str(), "move-1")
        .await
        .expect("read move");
    assert_eq!(read, Some(record));
    assert!(provider.records_returned() > 0, "read from the provider");
    assert!(
        reader_docs
            .queries()
            .await
            .iter()
            .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
        "only exact keys are read locally"
    );
}

// #1221 R6-B: timeline に出た author のうち手元に profile が無いものは、購読せずに背景で profile の key だけを
// provider から読む(R2-C で読取りの暗黙の購読を外した後の、表示名の回帰の補修)。profile の無い author は
// 台帳で読み直さない。scope の lease は増えない。
#[tokio::test]
async fn timeline_authors_without_a_local_profile_are_read_once_in_the_background() {
    let named = generate_keys();
    let unnamed = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    put_profile(provider.as_ref(), &named).await;
    let docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
    let (app, store) = app_over(docs.clone(), generate_keys());
    let topic = TopicId::new(TOPIC);
    for keys in [&named, &unnamed] {
        persist_test_post_with_labels(
            docs.as_ref(),
            Some(store.as_ref()),
            keys,
            &topic,
            PayloadRef::InlineText {
                text: "post".into(),
            },
            Vec::new(),
            None,
            Vec::new(),
        )
        .await;
    }
    let name_of = |view: &TimelineView, keys: &KukuriKeys| {
        view.items
            .iter()
            .find(|item| item.author_pubkey == keys.public_key_hex())
            .and_then(|item| item.author_name.clone())
    };
    let unnamed_replica = author_replica_id(unnamed.public_key_hex().as_str());
    let unnamed_reads = || async {
        provider
            .queries()
            .await
            .iter()
            .filter(|(replica, _)| replica == unnamed_replica.as_str())
            .count()
    };

    let first = app.list_timeline(TOPIC, None, 20).await.expect("timeline");
    assert_eq!(name_of(&first, &named), None);
    tokio::time::timeout(Duration::from_secs(5), async {
        while store
            .get_profile(named.public_key_hex().as_str())
            .await
            .expect("profile")
            .is_none()
            || unnamed_reads().await == 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the missing profiles are read in the background");
    let reads = unnamed_reads().await;
    let second = app.list_timeline(TOPIC, None, 20).await.expect("timeline");
    assert_eq!(name_of(&second, &named).as_deref(), Some("provider"));
    app.list_timeline(TOPIC, None, 20).await.expect("timeline");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        unnamed_reads().await,
        reads,
        "an author without a profile is not read again"
    );
    assert_eq!(app.subscription_registry.scope_leases.lock().await.len(), 0);
}

// #1442: 手元の投稿の行(projection)でページが埋まっても、作者に届くなら provider の行を読む。閲覧者が参加していない
// topic への投稿も、ページを送れば全件が 1 回ずつ出る(手元の行は閲覧者が持つ一部分で、remote を読むかの判断に数えない)。
#[tokio::test]
async fn held_projection_rows_do_not_stop_the_provider_read() {
    let author = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    let mut other_topic = Vec::new();
    let mut held = Vec::new();
    for index in 0..20 {
        other_topic.push(put_profile_post(provider.as_ref(), &author, BASE_TIME + 2 * index).await);
        let at = BASE_TIME + 2 * index + 1;
        held.push((at, put_profile_post(provider.as_ref(), &author, at).await));
    }
    let docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
    let (app, store) = app_over(docs.clone(), generate_keys());
    let author_pubkey = author.public_key_hex();
    persist_test_post_with_labels(
        docs.as_ref(),
        Some(store.as_ref()),
        &author,
        &TopicId::new(TOPIC),
        PayloadRef::InlineText { text: "t".into() },
        Vec::new(),
        None,
        Vec::new(),
    )
    .await;
    let template = ObjectProjectionStore::list_topic_timeline(store.as_ref(), TOPIC, None, 1)
        .await
        .expect("topic page")
        .items
        .remove(0);
    // 雛形の行は非公開の行に置き換え、プロフィールに出さない。
    let mut hidden = template.clone();
    hidden.channel_id = "private-channel".into();
    store
        .put_object_projection(hidden)
        .await
        .expect("hide the template");
    for (at, id) in &held {
        let mut row = template.clone();
        row.object_id = EnvelopeId::from(id.as_str());
        row.created_at = *at;
        store.put_object_projection(row).await.expect("held row");
    }

    let mut seen = Vec::new();
    let mut cursor = None;
    for _ in 0..40 {
        let page = profile_timeline_page(
            &app.services,
            author_pubkey.as_str(),
            None,
            cursor,
            20,
            &BTreeSet::new(),
        )
        .await
        .expect("profile page");
        seen.extend(
            page.items
                .iter()
                .map(|item| item.object_id().as_str().to_string()),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let mut expected = other_topic;
    expected.extend(held.into_iter().map(|(_, id)| id));
    expected.sort();
    seen.sort();
    assert_eq!(seen, expected);
}

// #1521 AC-1b: author の読み直しで自分を指す follow の edge を新しく保存したら、相手を 1 回知らせる。他の相手を指す
// edge(友達の友達の窓)と、同じ edge の読み直しでは知らせない。
#[tokio::test]
async fn author_reads_announce_only_a_new_follow_of_me() {
    let author = generate_keys();
    let local = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    put_profile(provider.as_ref(), &author).await;
    put_follow(
        provider.as_ref(),
        &author,
        generate_keys().public_key_hex().as_str(),
    )
    .await;
    put_follow(provider.as_ref(), &author, local.public_key_hex().as_str()).await;
    let (app, _) = app_over(
        Arc::new(CountingDocsSync::reading_from(provider)),
        local.clone(),
    );
    let mut changes = app.subscribe_author_relationship_changes();
    for _ in 0..2 {
        hydrate_author_state(
            &app.services,
            local.public_key_hex().as_str(),
            author.public_key_hex().as_str(),
            DocFetchPolicy::LocalThenRemote,
        )
        .await
        .expect("hydration");
    }
    assert_eq!(changes.try_recv().unwrap(), author.public_key_hex());
    assert!(
        changes.try_recv().is_err(),
        "only a new follow of me announces"
    );
}

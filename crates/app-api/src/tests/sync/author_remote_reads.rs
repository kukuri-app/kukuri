//! #1221 R5-C: author の現在値とプロフィールのページを、手元の次に有界な provider から読む。
//! provider の履歴(edge・投稿)を 10 倍にしても、provider から読む量が変わらないことを固定する。

use super::*;
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
    persist_profile_post_doc(docs_sync, &post, &envelope)
        .await
        .expect("persist profile post");
    object_id.as_str().to_string()
}

// 購読の開始時の反映は、手元に無い profile と自分を指す follow・block だけを provider から読む。
// provider の follow の総数を 10 倍にしても読む量は同じで、他の相手を指す edge は remote から読まない。
#[tokio::test]
async fn author_state_reads_only_the_current_keys_from_a_provider() {
    let mut counts = Vec::new();
    for edges in [60, 600] {
        let author = generate_keys();
        let local = generate_keys();
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
            1,
            "edges to other authors are not restored from the provider"
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
            local.public_key_hex().as_str(),
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
                    local.public_key_hex().as_str(),
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

// 自分のプロフィールは手元だけを読む(provider へ要求しない)。
#[tokio::test]
async fn the_own_profile_is_read_only_locally() {
    let local = generate_keys();
    let provider = Arc::new(CountingDocsSync::default());
    put_profile_post(provider.as_ref(), &local, BASE_TIME).await;
    let docs = Arc::new(CountingDocsSync::reading_from(provider.clone()));
    let (app, _) = app_over(docs, local.clone());
    provider.reset_records_returned();
    let page = profile_timeline_page(
        &app.services,
        local.public_key_hex().as_str(),
        local.public_key_hex().as_str(),
        None,
        None,
        20,
        &BTreeSet::new(),
    )
    .await
    .expect("own page");
    assert!(page.items.is_empty());
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

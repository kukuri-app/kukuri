//! #1239: プロフィールのタイムラインが、author replica の投稿の総数を読まず、索引からページの行だけを読むことを固定する。
//! 索引の無い投稿(索引を書く前の版が書いた投稿)は、上限つきの一覧の分だけ表示し、後から索引を補わないことも固定する。

use super::*;
use crate::service::profile_timeline_support::PROFILE_LEGACY_KEYS;
use crate::service::projection_support::HIDDEN_AUTHOR_SKIP_PAGES;

const TOPIC: &str = "kukuri:topic:profile-index";
const DOCS_AUTHOR: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const BASE_TIME: i64 = 1_700_000_000;

/// `keys` の author replica に、`created_at` の投稿を 1 件書く(投稿の doc・envelope・索引)。
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

/// `count` 件の投稿を書き、新しい順の object id を返す。
async fn put_profile_posts(
    docs_sync: &dyn DocsSync,
    keys: &KukuriKeys,
    count: usize,
    offset: i64,
) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        ids.push(put_profile_post(docs_sync, keys, BASE_TIME + offset + index as i64).await);
    }
    ids.reverse();
    ids
}

/// 索引を書く前の版が書いた replica を作る(プロフィールの索引を消す)。
async fn remove_profile_index(docs_sync: &dyn DocsSync, author_pubkey: &str) {
    docs_sync
        .apply_doc_op(
            &author_replica_id(author_pubkey),
            DocOp::DeletePrefix {
                prefix: "indexes/profile/".into(),
            },
        )
        .await
        .expect("remove the profile index");
}

fn app_over(docs_sync: Arc<dyn DocsSync>, keys: KukuriKeys) -> AppService {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        keys,
    )
}

/// 続きの位置をたどって、最後までのページの object id を集める。
async fn read_all_pages(app: &AppService, author_pubkey: &str, limit: usize) -> Vec<String> {
    let mut ids = Vec::new();
    let mut cursor = None;
    for _ in 0..1_000 {
        let page = app
            .list_profile_timeline(author_pubkey, cursor, limit)
            .await
            .expect("profile timeline page");
        assert!(page.items.len() <= limit);
        ids.extend(page.items.into_iter().map(|item| item.object_id));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return ids,
        }
    }
    panic!("the profile timeline did not end");
}

// 取得が読む docs の量は、author の投稿の総数に依存しない。
#[tokio::test]
async fn the_profile_timeline_reads_a_constant_amount_regardless_of_the_post_count() {
    let mut counts = Vec::new();
    // 索引の 1 回の一覧は、`limit` に形の違う key の余裕を足した件数(20 + 32)まで、旧 record の key の一覧は
    // `PROFILE_LEGACY_KEYS` 件まで読む。どちらも、その件数を超える件数にする(少ない側は、ある件数しか返らないので、
    // 比べる量が揃わない)。新しい側の時刻の下 2 桁も揃える(読む範囲が百の位をまたぐかで、索引の key の一覧の回数が変わる)。
    for posts in [300usize, 1_000] {
        let docs_sync = Arc::new(CountingDocsSync::with_docs_author(DOCS_AUTHOR));
        let app = app_over(docs_sync.clone(), generate_keys());
        let remote_keys = generate_keys();
        let remote_pubkey = remote_keys.public_key_hex();
        let ids = put_profile_posts(docs_sync.as_ref(), &remote_keys, posts, 0).await;
        // 著者の docs author を知っている閲覧者(profile の tag から覚えた状態)。
        app.services
            .projection_store
            .put_author_docs_author(remote_pubkey.as_str(), DOCS_AUTHOR)
            .await
            .expect("learn the docs author");
        app.list_profile_timeline(remote_pubkey.as_str(), None, 20)
            .await
            .expect("first page");
        sleep(Duration::from_millis(150)).await;
        docs_sync.clear_queries().await;
        docs_sync.reset_records_returned();

        let page = app
            .list_profile_timeline(remote_pubkey.as_str(), None, 20)
            .await
            .expect("profile timeline");

        let returned = page
            .items
            .iter()
            .map(|item| item.object_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(returned, ids[..20].to_vec(), "the newest page");
        assert!(page.next_cursor.is_some());
        let prefix_reads = docs_sync
            .queries()
            .await
            .into_iter()
            .filter(|(_, query)| matches!(query, DocQuery::Prefix(_)))
            .collect::<Vec<_>>();
        assert!(prefix_reads.is_empty(), "{prefix_reads:?}");
        counts.push(docs_sync.records_returned());
    }
    assert_eq!(
        counts[0], counts[1],
        "docs records read must not depend on the number of posts"
    );
}

// 続きの位置をたどると、すべての投稿が 1 回ずつ、新しい順に出る(以前は、ページの境目の投稿が 1 件ずつ飛んでいた)。
// 索引のある replica、索引の無い replica、両方が混ざった replica のそれぞれで確かめる。
#[tokio::test]
async fn paging_the_profile_timeline_returns_every_post_once_in_order() {
    for mode in ["indexed", "legacy", "mixed"] {
        let docs_sync = Arc::new(MemoryDocsSync::default());
        let app = app_over(docs_sync.clone(), generate_keys());
        let remote_keys = generate_keys();
        let remote_pubkey = remote_keys.public_key_hex();
        let mut ids = put_profile_posts(docs_sync.as_ref(), &remote_keys, 25, 0).await;
        if mode != "indexed" {
            remove_profile_index(docs_sync.as_ref(), remote_pubkey.as_str()).await;
        }
        if mode == "mixed" {
            let mut newer = put_profile_posts(docs_sync.as_ref(), &remote_keys, 20, 100).await;
            newer.extend(ids);
            ids = newer;
        }

        assert_eq!(
            read_all_pages(&app, remote_pubkey.as_str(), 7).await,
            ids,
            "{mode}: every post once, newest first"
        );
    }
}

// 非表示の著者の行が続いても、読むページ数には上限があり、読み進めた位置を返す。読む量は投稿の総数に依存しない。
#[tokio::test]
async fn hidden_author_rows_are_skipped_with_a_bounded_number_of_pages() {
    let mut counts = Vec::new();
    // 旧 record の一覧の上限(`PROFILE_LEGACY_KEYS`)を超え、新しい側の時刻の下 2 桁が揃う件数。
    for posts in [300usize, 600] {
        let docs_sync = Arc::new(CountingDocsSync::with_docs_author(DOCS_AUTHOR));
        let app = app_over(docs_sync.clone(), generate_keys());
        let remote_keys = generate_keys();
        let remote_pubkey = remote_keys.public_key_hex();
        let ids = put_profile_posts(docs_sync.as_ref(), &remote_keys, posts, 0).await;
        // 著者の docs author を知っている閲覧者(profile の tag から覚えた状態)。
        app.services
            .projection_store
            .put_author_docs_author(remote_pubkey.as_str(), DOCS_AUTHOR)
            .await
            .expect("learn the docs author");
        app.mute_author(remote_pubkey.as_str())
            .await
            .expect("mute the author");
        app.list_profile_timeline(remote_pubkey.as_str(), None, 10)
            .await
            .expect("first page");
        sleep(Duration::from_millis(150)).await;
        docs_sync.reset_records_returned();

        let page = app
            .list_profile_timeline(remote_pubkey.as_str(), None, 10)
            .await
            .expect("profile timeline");

        assert!(page.items.is_empty());
        let next = page.next_cursor.expect("the position read so far");
        assert_eq!(
            next.object_id.as_str(),
            ids[HIDDEN_AUTHOR_SKIP_PAGES * 10 - 1],
            "reads a bounded number of pages"
        );
        counts.push(docs_sync.records_returned());
    }
    assert_eq!(counts[0], counts[1]);
}

// 索引の無い旧い投稿に、後から索引を補わない(全件走査になる)。自分の replica でも、購読は索引を書かない。旧い投稿は、
// key の上限つきの一覧(`PROFILE_LEGACY_KEYS` 件)に入る分だけ表示し、それを超える分は表示しない。
#[tokio::test]
async fn legacy_posts_beyond_the_bounded_list_are_not_backfilled() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let keys = generate_keys();
    let author_pubkey = keys.public_key_hex();
    put_profile_posts(docs_sync.as_ref(), &keys, PROFILE_LEGACY_KEYS + 40, 0).await;
    remove_profile_index(docs_sync.as_ref(), author_pubkey.as_str()).await;
    let app = app_over(docs_sync.clone(), keys);

    app.list_profile_timeline(author_pubkey.as_str(), None, 5)
        .await
        .expect("profile timeline");
    sleep(Duration::from_millis(500)).await;

    let index = docs_sync
        .query_replica(
            &author_replica_id(author_pubkey.as_str()),
            DocQuery::Prefix("indexes/profile/".into()),
        )
        .await
        .expect("profile index");
    assert!(index.is_empty(), "no index entry is written afterwards");
    assert_eq!(
        read_all_pages(&app, author_pubkey.as_str(), 50).await.len(),
        PROFILE_LEGACY_KEYS
    );
    app.shutdown().await;
}

// 著者の docs author が分かれば、プロフィールの行を docs author と key の組で読むので、同じ key に他の名義のごみが何件あっても
// 隠されない(ADR 0053 §6)。分からないうちは、上限つきの読み出しがごみで埋まる(旧 record の best effort)。
#[tokio::test]
async fn profile_rows_are_read_by_the_docs_author_behind_shadows() {
    let account = "e".repeat(64);
    let docs_sync = Arc::new(
        super::shadowing_docs::ShadowingDocsSync::with_account_docs_author(account.clone()),
    );
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
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let ids = put_profile_posts(docs_sync.as_ref(), &remote_keys, 1, 0).await;
    for index in 0..10 {
        docs_sync
            .shadow(
                stable_key("profile/posts", ids[0].as_str()).as_str(),
                serde_json::json!({ "garbage": index }),
            )
            .await;
    }

    let before = app
        .list_profile_timeline(remote_pubkey.as_str(), None, 20)
        .await
        .expect("profile timeline before learning");
    assert!(
        before.items.is_empty(),
        "the bounded read is filled by the shadows"
    );

    store
        .put_author_docs_author(remote_pubkey.as_str(), account.as_str())
        .await
        .expect("learn the docs author");
    let after = app
        .list_profile_timeline(remote_pubkey.as_str(), None, 20)
        .await
        .expect("profile timeline");
    assert_eq!(
        after
            .items
            .iter()
            .map(|item| item.object_id.clone())
            .collect::<Vec<_>>(),
        ids
    );
}

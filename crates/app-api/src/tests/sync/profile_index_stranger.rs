//! #1239: docs author を知らない閲覧者の読み出し量は、投稿の数が旧 record の一覧の上限を超えると増えない。

use super::*;

const BASE_TIME: i64 = 1_700_000_000;

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

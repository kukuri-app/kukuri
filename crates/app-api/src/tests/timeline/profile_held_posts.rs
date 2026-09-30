//! #1442: 閲覧者が topic で受け取って手元に持つ作者の投稿は、作者の replica が手元に無く作者にも届かなくても、
//! プロフィールのタイムラインに出る。private channel の行は出さない。

use super::super::*;

const TOPIC: &str = "kukuri:topic:profile-held-posts";

struct Fixture {
    viewer: AppService,
    viewer_store: Arc<MemoryStore>,
    author_pubkey: String,
    /// 新しい順。
    object_ids: Vec<String>,
}

fn app_over(docs_sync: Arc<MemoryDocsSync>, keys: KukuriKeys) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        keys,
    );
    (app, store)
}

/// 作者が投稿と返信を書き、閲覧者が topic のタイムラインで受け取る。`drop_author_replica` なら、作者の replica
/// (旧形式と author bucket)を閲覧者の手元から外す(作者はオフラインで、閲覧者に作者の記録が無い状態)。
async fn fixture(drop_author_replica: bool) -> Fixture {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let author_keys = generate_keys();
    let author_pubkey = author_keys.public_key_hex();
    let (author, _) = app_over(docs_sync.clone(), author_keys);
    let post = author
        .create_post(TOPIC, "held post", None)
        .await
        .expect("post");
    let reply = author
        .create_post(TOPIC, "held reply", Some(post.as_str()))
        .await
        .expect("reply");
    if drop_author_replica {
        let created_at = author
            .list_timeline(TOPIC, None, 20)
            .await
            .expect("author timeline")
            .items[0]
            .created_at;
        for replica in [
            author_replica_id(author_pubkey.as_str()),
            author
                .services
                .author_index_replica(author_pubkey.as_str(), created_at)
                .expect("author bucket"),
        ] {
            docs_sync
                .apply_doc_op(
                    &replica,
                    DocOp::DeletePrefix {
                        prefix: String::new(),
                    },
                )
                .await
                .expect("drop the author replica");
        }
    }
    let (viewer, viewer_store) = app_over(docs_sync, generate_keys());
    let topic = viewer
        .list_timeline(TOPIC, None, 20)
        .await
        .expect("viewer timeline");
    assert!(topic.items.iter().any(|item| item.object_id == post));
    let mut object_ids = Vec::new();
    for id in [&post, &reply] {
        let row = viewer_store
            .get_object_projection(&EnvelopeId::from(id.as_str()))
            .await
            .expect("projection")
            .expect("the viewer holds the post");
        object_ids.push((row.created_at, id.clone()));
    }
    object_ids.sort();
    object_ids.reverse();
    Fixture {
        viewer,
        viewer_store,
        author_pubkey,
        object_ids: object_ids.into_iter().map(|(_, id)| id).collect(),
    }
}

async fn profile_ids(fixture: &Fixture, limit: usize) -> Vec<String> {
    let mut ids = Vec::new();
    let mut cursor = None;
    for _ in 0..100 {
        let page = fixture
            .viewer
            .list_profile_timeline(fixture.author_pubkey.as_str(), cursor, limit)
            .await
            .expect("profile timeline");
        assert!(page.items.len() <= limit);
        ids.extend(page.items.into_iter().map(|item| item.object_id));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return ids,
        }
    }
    panic!("the profile timeline did not end");
}

#[tokio::test]
async fn posts_held_locally_appear_when_the_author_replica_is_absent() {
    let fixture = fixture(true).await;
    for limit in [1, 20] {
        assert_eq!(
            profile_ids(&fixture, limit).await,
            fixture.object_ids,
            "limit={limit}"
        );
    }
}

// author replica の行と手元の投稿の行が同じ投稿を指すときは、1 回だけ出る。
#[tokio::test]
async fn a_post_in_both_the_author_replica_and_the_projection_appears_once() {
    let fixture = fixture(false).await;
    for limit in [1, 20] {
        assert_eq!(
            profile_ids(&fixture, limit).await,
            fixture.object_ids,
            "limit={limit}"
        );
    }
}

// 同じ作者の private channel の行は、手元にあってもプロフィールに出さない。
#[tokio::test]
async fn private_channel_rows_held_locally_do_not_appear() {
    let fixture = fixture(true).await;
    let mut private = fixture
        .viewer_store
        .get_object_projection(&EnvelopeId::from(fixture.object_ids[0].as_str()))
        .await
        .expect("projection")
        .expect("row");
    private.object_id = EnvelopeId::from("f".repeat(64).as_str());
    private.channel_id = "private-channel".into();
    private.created_at += 10;
    fixture
        .viewer_store
        .put_object_projection(private)
        .await
        .expect("private row");

    assert_eq!(profile_ids(&fixture, 20).await, fixture.object_ids);
}

// 非表示(ミュート)の作者の手元の投稿は、プロフィールにも出さない(author replica の行と同じ扱い)。
#[tokio::test]
async fn posts_held_locally_of_a_muted_author_stay_hidden() {
    let fixture = fixture(true).await;
    fixture
        .viewer
        .mute_author(fixture.author_pubkey.as_str())
        .await
        .expect("mute");

    assert!(profile_ids(&fixture, 20).await.is_empty());
}

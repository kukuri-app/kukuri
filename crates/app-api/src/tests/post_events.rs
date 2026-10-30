//! #1981 AC-1: 手元へ新しく保存した投稿・返信だけを知らせる。知らせるのは作成から間もないもので、非表示の作者でないものだけ。
use super::*;
use tokio::sync::broadcast::error::TryRecvError;

/// 書いた docs と blob を共有し、手元の行(store)と鍵だけが別の端末。
fn device(docs: Arc<dyn DocsSync>, blobs: Arc<MemoryBlobService>) -> AppService {
    let store = Arc::new(MemoryStore::default());
    app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs,
        blobs,
        generate_keys(),
    )
}

/// 届いていた知らせを、知らせる形(知らせないものは `None`)にして読み切る。
async fn announced(
    app: &AppService,
    changes: &mut broadcast::Receiver<ObjectProjectionRow>,
) -> Vec<Option<PostReceivedView>> {
    let mut shown = Vec::new();
    loop {
        match changes.try_recv() {
            Ok(row) => shown.push(app.post_received_view(&row).await.expect("view")),
            Err(TryRecvError::Empty) => return shown,
            Err(error) => panic!("post changes: {error}"),
        }
    }
}

#[tokio::test]
async fn new_posts_and_replies_are_announced_once_and_hidden_authors_are_not_shown() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:post-events";
    let viewer = device(docs, blobs);
    let mut changes = viewer.subscribe_post_changes();

    let post_id = owner.create_post(topic, "hello", None).await.expect("post");
    viewer
        .list_timeline(topic, None, 20)
        .await
        .expect("timeline");
    let shown = announced(&viewer, &mut changes).await;
    assert_eq!(shown.len(), 1, "a post from another device is announced");
    let view = shown[0].as_ref().expect("announced");
    assert_eq!(view.object_id, post_id);
    assert_eq!(view.topic_id, topic);
    assert_eq!(view.author_pubkey, owner.current_author_pubkey());
    assert_eq!(view.reply_to, None);

    viewer.list_timeline(topic, None, 20).await.expect("again");
    assert!(
        announced(&viewer, &mut changes).await.is_empty(),
        "reading the same post again is not announced"
    );

    let reply_id = owner
        .create_post(topic, "reply", Some(post_id.as_str()))
        .await
        .expect("reply");
    viewer
        .list_thread(topic, &post_id, None, 20)
        .await
        .expect("thread");
    let shown = announced(&viewer, &mut changes).await;
    let view = shown
        .iter()
        .flatten()
        .find(|view| view.object_id == reply_id)
        .expect("the reply is announced");
    assert_eq!(view.reply_to.as_deref(), Some(post_id.as_str()));
    assert_eq!(view.root_id.as_deref(), Some(post_id.as_str()));

    viewer
        .block_author(owner.current_author_pubkey().as_str())
        .await
        .expect("block");
    owner
        .create_post(topic, "while blocked", None)
        .await
        .expect("post while blocked");
    // block した作者の投稿は一覧に出ないので、行は store から直接読む。
    let rows = owner
        .services
        .projection_store
        .list_topic_timeline(topic, None, 20)
        .await
        .expect("rows");
    let blocked = rows
        .items
        .iter()
        .find(|row| row.content.as_deref() == Some("while blocked"))
        .expect("the owner's row");
    assert_eq!(
        viewer.post_received_view(blocked).await.expect("view"),
        None,
        "a blocked author's post is not announced"
    );
    owner.shutdown().await;
    viewer.shutdown().await;
}

#[tokio::test]
async fn own_posts_are_announced_and_old_posts_are_not() {
    let (app, _, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:post-events-own";
    let mut changes = app.subscribe_post_changes();
    let post_id = app.create_post(topic, "mine", None).await.expect("post");
    let shown = announced(&app, &mut changes).await;
    assert_eq!(
        shown
            .iter()
            .map(|view| view.as_ref().map(|view| view.object_id.as_str()))
            .collect::<Vec<_>>(),
        vec![Some(post_id.as_str())]
    );

    let mut row = app
        .services
        .projection_store
        .get_object_projection(&EnvelopeId::from(post_id.as_str()))
        .await
        .expect("read")
        .expect("row");
    row.created_at -= 3_600;
    assert_eq!(
        app.post_received_view(&row).await.expect("view"),
        None,
        "a post written long before it arrived is not announced"
    );
    app.shutdown().await;
}

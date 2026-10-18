//! #1841 AC-1: 手元へ反映したリアクションの新しい版だけを知らせる。画面へ知らせるのは、付いている版で、非表示の作者でないものだけ。
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

fn emoji(emoji: &str) -> ReactionKeyV1 {
    ReactionKeyV1::Emoji {
        emoji: emoji.into(),
    }
}

/// hint を受けたときと同じ関数で、対象の reaction を上限つきで反映する。
async fn receive(app: &AppService, topic: &str, target: &str) {
    hydrate_reaction_cache_for_target_bounded(
        app.services.docs_sync.as_ref(),
        app.services.projection_store.as_ref(),
        app.services.blob_service.as_ref(),
        &app.services.reaction_changes,
        topic,
        &topic_replica_id(topic),
        &EnvelopeId::from(target),
        DocFetchPolicy::LocalOnly,
        8,
    )
    .await
    .expect("receive reactions");
}

/// 届いていた知らせを、画面へ知らせる形(知らせないものは `None`)にして読み切る。
async fn announced(
    app: &AppService,
    changes: &mut broadcast::Receiver<ReactionProjectionRow>,
) -> Vec<Option<ReactionReceivedView>> {
    let mut shown = Vec::new();
    loop {
        match changes.try_recv() {
            Ok(row) => shown.push(app.reaction_received_view(&row).await.expect("view")),
            Err(TryRecvError::Empty) => return shown,
            Err(error) => panic!("reaction changes: {error}"),
        }
    }
}

#[tokio::test]
async fn only_new_versions_are_announced_and_hidden_authors_are_not_shown() {
    let (owner, _, docs, blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:reaction-events";
    let post_id = owner
        .create_post(topic, "react here", None)
        .await
        .expect("post");
    let viewer = device(docs, blobs);
    let mut changes = viewer.subscribe_reaction_changes();

    owner
        .toggle_reaction(topic, &post_id, emoji("👍"), Some(ChannelRef::Public))
        .await
        .expect("react");
    receive(&viewer, topic, &post_id).await;
    let shown = announced(&viewer, &mut changes).await;
    assert_eq!(
        shown.len(),
        1,
        "a reaction from another device is announced"
    );
    let view = shown[0].as_ref().expect("shown on the screen");
    assert_eq!(view.target_object_id, post_id);
    assert_eq!(view.reaction.emoji.as_deref(), Some("👍"));

    receive(&viewer, topic, &post_id).await;
    assert!(
        announced(&viewer, &mut changes).await.is_empty(),
        "reading the same version again is not announced"
    );

    owner
        .toggle_reaction(topic, &post_id, emoji("👍"), Some(ChannelRef::Public))
        .await
        .expect("remove");
    receive(&viewer, topic, &post_id).await;
    assert_eq!(
        announced(&viewer, &mut changes).await,
        vec![None],
        "a removed reaction is not shown"
    );

    viewer
        .mute_author(owner.current_author_pubkey().as_str())
        .await
        .expect("mute");
    owner
        .toggle_reaction(topic, &post_id, emoji("🎉"), Some(ChannelRef::Public))
        .await
        .expect("react while muted");
    receive(&viewer, topic, &post_id).await;
    assert_eq!(
        announced(&viewer, &mut changes).await,
        vec![None],
        "a muted author's reaction is not shown"
    );
    owner.shutdown().await;
    viewer.shutdown().await;
}

#[tokio::test]
async fn own_reactions_are_announced_when_added() {
    let (app, _, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:reaction-events-own";
    let post_id = app.create_post(topic, "mine", None).await.expect("post");
    let mut changes = app.subscribe_reaction_changes();

    app.toggle_reaction(topic, &post_id, emoji("👍"), Some(ChannelRef::Public))
        .await
        .expect("react");
    let shown = announced(&app, &mut changes).await;
    assert_eq!(shown.len(), 1);
    assert_eq!(
        shown[0].as_ref().map(|view| view.target_object_id.as_str()),
        Some(post_id.as_str())
    );

    app.toggle_reaction(topic, &post_id, emoji("👍"), Some(ChannelRef::Public))
        .await
        .expect("remove");
    assert_eq!(announced(&app, &mut changes).await, vec![None]);
    app.shutdown().await;
}

#[tokio::test]
async fn reactions_that_arrive_long_after_they_were_added_are_not_shown() {
    let (app, _, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:reaction-events-late";
    let post_id = app.create_post(topic, "late", None).await.expect("post");
    let mut changes = app.subscribe_reaction_changes();
    app.toggle_reaction(topic, &post_id, emoji("👍"), Some(ChannelRef::Public))
        .await
        .expect("react");
    let mut row = changes.try_recv().expect("announced");
    assert!(
        app.reaction_received_view(&row)
            .await
            .expect("view")
            .is_some()
    );
    // 再接続や照合で、付いてから時間の経った版が届いた。
    row.updated_at -= crate::reactions::REACTION_EVENT_RECENT_MS + 1;
    assert!(
        app.reaction_received_view(&row)
            .await
            .expect("view")
            .is_none()
    );
    app.shutdown().await;
}

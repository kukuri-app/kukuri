//! 共有 replica（topic / channel）へ書く key が `SharedReplicaKeyFamily` に全て登録済みである
//! ことを、実操作で突き合わせる（#1065 INVAR-3）。
//!
//! cn-indexer は変更通知の key を種別表で分類し、未登録の key は scope 全体の見直しへ倒す。
//! ここで未登録 key が見つかったら、種別表へ追加し cn-indexer の `key_disposition` で扱いを決める。

use super::*;
use kukuri_docs_sync::SharedReplicaKeyFamily;

/// 書き込まれた `(replica id, key)` を記録しつつ `MemoryDocsSync` へ委譲する。
#[derive(Clone, Default)]
struct KeyRecordingDocsSync {
    inner: Arc<MemoryDocsSync>,
    writes: Arc<TokioMutex<Vec<(String, String)>>>,
}

#[async_trait]
impl DocsSync for KeyRecordingDocsSync {
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

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        let key = match &op {
            DocOp::SetJson { key, .. } | DocOp::SetBytes { key, .. } => key.clone(),
            DocOp::DeletePrefix { prefix } => prefix.clone(),
        };
        self.writes
            .lock()
            .await
            .push((replica_id.as_str().to_string(), key));
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
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

    async fn restart_replica_sync(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.restart_replica_sync(replica_id).await
    }
}

#[tokio::test]
async fn shared_replica_writes_use_only_registered_key_families() {
    let docs = KeyRecordingDocsSync::default();
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        Arc::new(docs.clone()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:shared-replica-keys";

    let root_id = app
        .create_post(topic, "root body", None)
        .await
        .expect("create post");
    app.create_post(topic, "reply body", Some(root_id.as_str()))
        .await
        .expect("create reply");
    app.create_repost(topic, topic, root_id.as_str(), None)
        .await
        .expect("create repost");
    let media_id = app
        .create_post_with_attachments(
            topic,
            "media body",
            None,
            vec![pending_image_attachment("image/png", &tiny_png_bytes())],
        )
        .await
        .expect("create media post");
    app.toggle_reaction(
        topic,
        root_id.as_str(),
        ReactionKeyV1::Emoji {
            emoji: "👍".into()
        },
        None,
    )
    .await
    .expect("toggle reaction");
    app.withdraw_post(
        topic,
        &media_id,
        ChannelRef::Public,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .await
    .expect("withdraw post");
    app.create_live_session(
        topic,
        CreateLiveSessionInput {
            title: "live".into(),
            description: "desc".into(),
        },
    )
    .await
    .expect("create live session");
    app.create_game_room(
        topic,
        CreateGameRoomInput {
            title: "room".into(),
            description: "desc".into(),
            participants: vec!["Alice".into(), "Bob".into()],
        },
    )
    .await
    .expect("create game room");
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "invite".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    app.create_post_in_channel(
        topic,
        ChannelRef::PrivateChannel {
            channel_id: ChannelId::new(channel.channel_id.clone()),
        },
        "channel body",
        None,
    )
    .await
    .expect("create channel post");

    let writes = docs.writes.lock().await.clone();
    let shared: Vec<&(String, String)> = writes
        .iter()
        .filter(|(replica, _)| replica.starts_with("topic::") || replica.starts_with("channel::"))
        .collect();
    let families: std::collections::BTreeSet<String> = shared
        .iter()
        .filter_map(|(_, key)| SharedReplicaKeyFamily::parse(key))
        .map(|(family, _)| format!("{family:?}"))
        .collect();
    // 突き合わせが実際に投稿・索引・reaction・session・channel の書き込みを通ったことを確認する。
    for expected in [
        "PostObject",
        "PostWithdrawal",
        "MediaManifest",
        "TimelineIndex",
        "ThreadIndex",
        "Reaction",
        "Envelope",
        "Session",
        "Channel",
    ] {
        assert!(
            families.contains(expected),
            "{expected} was not exercised: {families:?}"
        );
    }
    let unregistered: Vec<&str> = shared
        .iter()
        .filter(|(_, key)| SharedReplicaKeyFamily::parse(key).is_none())
        .map(|(_, key)| key.as_str())
        .collect();
    assert!(
        unregistered.is_empty(),
        "shared replica keys missing from SharedReplicaKeyFamily: {unregistered:?}"
    );
}

use super::*;
use kukuri_core::BlobHash;
use kukuri_store::NotificationStore;

#[tokio::test]
async fn adult_labeled_object_notification_preview_is_hidden_until_display_enabled() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-adult-mention");
    let remote_keys = generate_keys();
    let adult_preview = format!("adult preview @{}", app.current_author_pubkey());
    let remote_envelope = persist_test_post_with_labels(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: adult_preview.clone(),
        },
        Vec::new(),
        None,
        vec![kukuri_core::ADULT_CONTENT_LABEL.to_string()],
    )
    .await;
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse remote adult mention")
        .expect("remote adult mention object");

    assert!(
        create_remote_object_notification(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            remote_doc_event(
                docs_sync.as_ref(),
                &topic_replica_id(topic.as_str()),
                stable_key(
                    "objects",
                    &format!("{}/state", remote_object.object_id.as_str()),
                ),
            )
            .await,
        )
        .await
    );

    let rows = NotificationStore::list_notifications(store.as_ref())
        .await
        .expect("list notification rows");
    assert_eq!(
        rows[0].content_labels.as_deref(),
        Some(&[kukuri_core::ADULT_CONTENT_LABEL.to_string()][..])
    );

    let hidden = app
        .list_notifications()
        .await
        .expect("list hidden notification");
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].kind, NotificationKind::Mention);
    assert_eq!(hidden[0].preview_text, None);

    app.set_adult_content_display_enabled(true);
    let visible = app
        .list_notifications()
        .await
        .expect("list visible notification");
    assert_eq!(
        visible[0].preview_text.as_deref(),
        Some(adult_preview.as_str())
    );
}

#[tokio::test]
async fn adult_labeled_repost_source_notification_preview_is_hidden_until_display_enabled() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-adult-repost-source");
    let source_object_id = app
        .create_post_with_attachments_in_channel(
            topic.as_str(),
            kukuri_core::ChannelRef::Public,
            "adult repost source",
            None,
            Vec::new(),
            vec![kukuri_core::ADULT_CONTENT_LABEL.to_string()],
        )
        .await
        .expect("create adult repost source");
    let repost_source = app
        .resolve_repost_source(topic.as_str(), source_object_id.as_str())
        .await
        .expect("resolve adult repost source");
    let remote_keys = generate_keys();
    let remote_envelope =
        build_repost_envelope(&remote_keys, &topic, repost_source.repost_of, None)
            .expect("build simple repost");
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse simple repost")
        .expect("simple repost object");
    persist_post_object(
        docs_sync.as_ref(),
        &topic_replica_id(topic.as_str()),
        remote_object.clone(),
        remote_envelope,
    )
    .await
    .expect("persist simple repost");

    assert!(
        create_remote_object_notification(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            remote_doc_event(
                docs_sync.as_ref(),
                &topic_replica_id(topic.as_str()),
                stable_key(
                    "objects",
                    &format!("{}/state", remote_object.object_id.as_str()),
                ),
            )
            .await,
        )
        .await
    );

    let rows = NotificationStore::list_notifications(store.as_ref())
        .await
        .expect("list notification rows");
    assert_eq!(
        rows[0].content_labels.as_deref(),
        Some(&[kukuri_core::ADULT_CONTENT_LABEL.to_string()][..])
    );

    let hidden = app
        .list_notifications()
        .await
        .expect("list hidden notification");
    assert_eq!(hidden[0].kind, NotificationKind::Repost);
    assert_eq!(hidden[0].preview_text, None);

    app.set_adult_content_display_enabled(true);
    let visible = app
        .list_notifications()
        .await
        .expect("list visible notification");
    assert_eq!(
        visible[0].preview_text.as_deref(),
        Some("adult repost source")
    );
}

// #858 / ADR 0046: 成人向けラベル付き投稿の添付は、表示設定(既定 OFF)を有効化する
// まで `blob_media_payload` がバイト列を返さない(fail-closed)。ローカル blob store に
// バイト列が存在していても読み出さないことを確認する = ネットワーク取得・デコードの
// 前段で遮断される。OFF へ戻すと以後の取得も再び止まる。
#[tokio::test]
async fn adult_labeled_media_payload_is_blocked_until_display_enabled() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store.clone(), transport);
    let topic = "kukuri:topic:adult-gate";

    let object_id = app
        .create_post_with_attachments_in_channel(
            topic,
            kukuri_core::ChannelRef::Public,
            "labeled caption",
            None,
            vec![PendingAttachment {
                mime: "image/png".into(),
                bytes: b"adult-labeled-image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
            vec![kukuri_core::ADULT_CONTENT_LABEL.to_string()],
        )
        .await
        .expect("create labeled post");

    let timeline = app.list_timeline(topic, None, 10).await.expect("timeline");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("labeled post");
    assert_eq!(post.content_labels, vec!["adult".to_string()]);
    let attachment_hash = post.attachments[0].hash.clone();

    // 逆引き記録: 投稿の projection 書き込みで hash がゲート対象になっている。
    assert!(
        ObjectProjectionStore::is_adult_media_hash(
            store.as_ref(),
            &kukuri_core::BlobHash::new(attachment_hash.clone())
        )
        .await
        .expect("is_adult_media_hash")
    );

    // 既定 OFF: バイト列はローカルにあっても返さない。
    assert!(!app.adult_content_display_enabled());
    assert!(
        app.blob_media_payload(attachment_hash.as_str(), "image/png")
            .await
            .expect("gated payload result")
            .is_none()
    );

    // 明示的に有効化した場合だけ返す。
    app.set_adult_content_display_enabled(true);
    let payload = app
        .blob_media_payload(attachment_hash.as_str(), "image/png")
        .await
        .expect("enabled payload result")
        .expect("payload present after enabling");
    assert_eq!(payload.mime, "image/png");

    // OFF へ戻すと以後の取得は再び止まる。
    app.set_adult_content_display_enabled(false);
    assert!(
        app.blob_media_payload(attachment_hash.as_str(), "image/png")
            .await
            .expect("re-disabled payload result")
            .is_none()
    );
}

// #858: ラベルなし添付は表示設定 OFF でも従来どおり取得できる(fail-open)。
#[tokio::test]
async fn unlabeled_media_payload_is_unaffected_by_adult_display_setting() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:unlabeled-gate";

    let object_id = app
        .create_post_with_attachments(
            topic,
            "plain caption",
            None,
            vec![PendingAttachment {
                mime: "image/png".into(),
                bytes: b"plain-image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
        )
        .await
        .expect("create unlabeled post");
    let timeline = app.list_timeline(topic, None, 10).await.expect("timeline");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("unlabeled post");
    assert!(post.content_labels.is_empty());

    assert!(!app.adult_content_display_enabled());
    assert!(
        app.blob_media_payload(post.attachments[0].hash.as_str(), "image/png")
            .await
            .expect("payload result")
            .is_some()
    );
}

// #858: self-label は既知値(`adult`)だけを受け付ける。
#[tokio::test]
async fn create_post_rejects_unknown_content_labels() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let error = app
        .create_post_with_attachments_in_channel(
            "kukuri:topic:bad-label",
            kukuri_core::ChannelRef::Public,
            "body",
            None,
            Vec::new(),
            vec!["nsfw-unknown".to_string()],
        )
        .await
        .expect_err("unknown label must be rejected");
    assert!(error.to_string().contains("unknown content label"));
}

// #1055 / ADR 0046 §6.2: 設定済み Community Node が発行した content advisory の対象 hash も、
// self-label と同じ取得ゲートで扱う。表示設定 OFF の間は blob がローカルにあってもバイト列を
// 返さず、ON では ephemeral fetch で返す。OFF へ戻すと以後の取得も再び止まる。
// advisory は投稿の署名済み `content_labels` を書き換えない(INVAR-1)。
#[tokio::test]
async fn advisory_labeled_media_respects_adult_display_gate() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store.clone(), transport);
    let topic = "kukuri:topic:advisory-gate";

    // self-label の無い通常の投稿。CN の advisory だけがラベル源になる。
    let object_id = app
        .create_post_with_attachments(
            topic,
            "plain caption",
            None,
            vec![PendingAttachment {
                mime: "image/png".into(),
                bytes: b"advisory-labeled-image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
        )
        .await
        .expect("create advisory target post");

    let timeline = app.list_timeline(topic, None, 10).await.expect("timeline");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("advisory target post");
    assert!(post.content_labels.is_empty());
    let attachment_hash = post.attachments[0].hash.clone();

    // self-label 由来の逆引き記録は付かない。ゲートは advisory 集合だけで成立する。
    assert!(
        !ObjectProjectionStore::is_adult_media_hash(
            store.as_ref(),
            &kukuri_core::BlobHash::new(attachment_hash.clone())
        )
        .await
        .expect("is_adult_media_hash")
    );
    assert!(!app.is_advisory_media_hash(attachment_hash.as_str()).await);

    // 登録前は通常どおり取得できる(ラベル源が無い投稿は fail-open)。
    assert!(!app.adult_content_display_enabled());
    assert!(
        app.blob_media_payload(attachment_hash.as_str(), "image/png")
            .await
            .expect("pre-registration payload result")
            .is_some()
    );

    app.register_advisory_media_hashes(std::slice::from_ref(&attachment_hash))
        .await;
    assert!(app.is_advisory_media_hash(attachment_hash.as_str()).await);

    // 既定 OFF: バイト列はローカルにあっても返さない。
    assert!(
        app.blob_media_payload(attachment_hash.as_str(), "image/png")
            .await
            .expect("gated payload result")
            .is_none()
    );

    // 明示的に有効化した場合だけ返す。
    app.set_adult_content_display_enabled(true);
    let payload = app
        .blob_media_payload(attachment_hash.as_str(), "image/png")
        .await
        .expect("enabled payload result")
        .expect("payload present after enabling");
    assert_eq!(payload.mime, "image/png");

    // OFF へ戻すと以後の取得は再び止まる。
    app.set_adult_content_display_enabled(false);
    assert!(
        app.blob_media_payload(attachment_hash.as_str(), "image/png")
            .await
            .expect("re-disabled payload result")
            .is_none()
    );

    // INVAR-1: advisory は署名済み `content_labels` へ書き戻さない。
    let timeline = app
        .list_timeline(topic, None, 10)
        .await
        .expect("timeline after advisory registration");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("advisory target post after registration");
    assert!(post.content_labels.is_empty());
}

/// remote peer だけが持つ blob を、`IrohBlobService` と同じ規則で扱う double(#1152)。
/// `fetch_blob` はローカルに無ければ remote から取得してローカルへ保存し、`blob_status` は
/// ローカルに無ければ `fetch_blob` で確かめる(= 状態確認が取得を兼ねる)。
struct RemotePeerBlobService {
    local: MemoryBlobService,
    remote: TokioMutex<HashMap<String, Vec<u8>>>,
    remote_fetches: TokioMutex<Vec<String>>,
}

impl RemotePeerBlobService {
    fn new() -> Self {
        Self {
            local: MemoryBlobService::default(),
            remote: TokioMutex::new(HashMap::new()),
            remote_fetches: TokioMutex::new(Vec::new()),
        }
    }

    /// remote peer にだけ blob を置き、投稿へ載せる AssetRef を返す。
    async fn put_remote(&self, data: &[u8], mime: &str) -> kukuri_core::AssetRef {
        let hash = blake3::hash(data).to_hex().to_string();
        self.remote.lock().await.insert(hash.clone(), data.to_vec());
        kukuri_core::AssetRef {
            hash: BlobHash::new(hash),
            mime: mime.to_string(),
            bytes: data.len() as u64,
            role: AssetRole::ImageOriginal,
        }
    }

    async fn remote_fetches(&self) -> Vec<String> {
        self.remote_fetches.lock().await.clone()
    }

    async fn is_local(&self, hash: &str) -> bool {
        self.local
            .fetch_blob(&BlobHash::new(hash))
            .await
            .expect("local fetch")
            .is_some()
    }
}

#[async_trait]
impl BlobService for RemotePeerBlobService {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.local.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.local.fetch_blob(hash).await? {
            return Ok(Some(bytes));
        }
        let Some(bytes) = self.remote.lock().await.get(hash.as_str()).cloned() else {
            return Ok(None);
        };
        self.remote_fetches
            .lock()
            .await
            .push(hash.as_str().to_string());
        self.local
            .put_blob(bytes.clone(), "application/octet-stream")
            .await?;
        Ok(Some(bytes))
    }

    async fn fetch_blob_ephemeral(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.local.fetch_blob(hash).await? {
            return Ok(Some(bytes));
        }
        let Some(bytes) = self.remote.lock().await.get(hash.as_str()).cloned() else {
            return Ok(None);
        };
        self.remote_fetches
            .lock()
            .await
            .push(hash.as_str().to_string());
        Ok(Some(bytes))
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.local.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        Ok(match self.fetch_blob(hash).await? {
            Some(_) => BlobStatus::Available,
            None => BlobStatus::Missing,
        })
    }

    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.local.local_blob_status(hash).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.local.import_peer_ticket(ticket).await
    }
}

fn app_with_remote_peer_blobs(
    docs_sync: Arc<MemoryDocsSync>,
    blob_service: Arc<RemotePeerBlobService>,
) -> AppService {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        generate_keys(),
    )
}

// #1152 / ADR 0046 §4: 表示設定 OFF の間、remote から届いた self-label 付き投稿を projection へ
// 反映・表示しても、添付の状態確認が bytes を remote 取得・永続化しない。
#[tokio::test]
async fn projecting_remote_adult_labeled_post_does_not_fetch_attachment_while_display_disabled() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(RemotePeerBlobService::new());
    let topic = TopicId::new("kukuri:topic:adult-projection-fetch");
    let attachment = blob_service
        .put_remote(b"remote-adult-labeled-image", "image/png")
        .await;
    let attachment_hash = attachment.hash.as_str().to_string();
    persist_test_post_with_labels(
        docs_sync.as_ref(),
        None,
        &generate_keys(),
        &topic,
        PayloadRef::InlineText {
            text: "remote adult caption".into(),
        },
        vec![attachment],
        None,
        vec![kukuri_core::ADULT_CONTENT_LABEL.to_string()],
    )
    .await;
    let app = app_with_remote_peer_blobs(docs_sync, blob_service.clone());
    assert!(!app.adult_content_display_enabled());

    let timeline = app
        .list_timeline(topic.as_str(), None, 10)
        .await
        .expect("timeline");
    assert_eq!(timeline.items.len(), 1);
    assert_eq!(timeline.items[0].attachments[0].hash, attachment_hash);
    assert_eq!(
        timeline.items[0].attachments[0].status,
        BlobViewStatus::Missing
    );

    assert!(
        app.blob_media_payload(attachment_hash.as_str(), "image/png")
            .await
            .expect("gated payload result")
            .is_none()
    );
    assert_eq!(blob_service.remote_fetches().await, Vec::<String>::new());
    assert!(!blob_service.is_local(attachment_hash.as_str()).await);
}

// #1152 / ADR 0046 §6.2: CN advisory は受信時点では未判明のため、ラベルの無い添付も状態確認で
// 取得しない。advisory 登録後は表示要求でも取得せず、advisory の無い添付は表示要求で初めて取得する。
#[tokio::test]
async fn projecting_remote_posts_fetches_attachments_only_on_ungated_display_request() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(RemotePeerBlobService::new());
    let topic = TopicId::new("kukuri:topic:advisory-projection-fetch");
    let remote_keys = generate_keys();
    let advisory_attachment = blob_service
        .put_remote(b"remote-advisory-image", "image/png")
        .await;
    let advisory_hash = advisory_attachment.hash.as_str().to_string();
    let plain_attachment = blob_service
        .put_remote(b"remote-plain-image", "image/png")
        .await;
    let plain_hash = plain_attachment.hash.as_str().to_string();
    for (text, attachment) in [
        ("advisory caption", advisory_attachment),
        ("plain caption", plain_attachment),
    ] {
        persist_test_post(
            docs_sync.as_ref(),
            None,
            &remote_keys,
            &topic,
            PayloadRef::InlineText { text: text.into() },
            vec![attachment],
            None,
        )
        .await;
    }
    let app = app_with_remote_peer_blobs(docs_sync, blob_service.clone());

    let timeline = app
        .list_timeline(topic.as_str(), None, 10)
        .await
        .expect("timeline");
    assert_eq!(timeline.items.len(), 2);
    assert_eq!(blob_service.remote_fetches().await, Vec::<String>::new());

    app.register_advisory_media_hashes(std::slice::from_ref(&advisory_hash))
        .await;
    assert!(
        app.blob_media_payload(advisory_hash.as_str(), "image/png")
            .await
            .expect("advisory payload result")
            .is_none()
    );
    assert_eq!(blob_service.remote_fetches().await, Vec::<String>::new());
    assert!(!blob_service.is_local(advisory_hash.as_str()).await);

    assert!(
        app.blob_media_payload(plain_hash.as_str(), "image/png")
            .await
            .expect("plain payload result")
            .is_some()
    );
    assert_eq!(
        blob_service.remote_fetches().await,
        vec![plain_hash.clone()]
    );
    assert!(blob_service.is_local(plain_hash.as_str()).await);

    // #1207 TR-4: 取得済みの hash はローカルから返し、remote 取得を増やさない。
    assert!(
        app.blob_media_payload(plain_hash.as_str(), "image/png")
            .await
            .expect("cached payload result")
            .is_some()
    );
    assert_eq!(blob_service.remote_fetches().await, vec![plain_hash]);
}

// #1055: advisory が付いていない hash は、登録済みの別 hash があっても影響を受けない。
#[tokio::test]
async fn advisory_registration_does_not_gate_unrelated_media() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:advisory-unrelated";

    let object_id = app
        .create_post_with_attachments(
            topic,
            "unrelated caption",
            None,
            vec![PendingAttachment {
                mime: "image/png".into(),
                bytes: b"unrelated-image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
        )
        .await
        .expect("create unrelated post");
    let timeline = app.list_timeline(topic, None, 10).await.expect("timeline");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("unrelated post");
    let attachment_hash = post.attachments[0].hash.clone();

    app.register_advisory_media_hashes(&[
        "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
    ])
    .await;

    assert!(!app.adult_content_display_enabled());
    assert!(
        app.blob_media_payload(attachment_hash.as_str(), "image/png")
            .await
            .expect("payload result")
            .is_some()
    );
}

use super::*;
use kukuri_core::BlobHash;
use kukuri_store::{MediaBookmarkStore, MediaShelf};

#[derive(Default)]
struct CountedBlobs {
    inner: MemoryBlobService,
    media_writes: AtomicUsize,
}

#[async_trait]
impl BlobService for CountedBlobs {
    async fn put_blob(&self, bytes: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        if mime != "text/plain" {
            self.media_writes.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.put_blob(bytes, mime).await
    }
    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.inner.fetch_blob(hash).await
    }
    async fn fetch_local_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.inner.fetch_local_blob(hash).await
    }
    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }
    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.inner.blob_status(hash).await
    }
    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.inner.local_blob_status(hash).await
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

fn reuse(hash: &str) -> PendingAttachment {
    PendingAttachment {
        mime: String::new(),
        bytes: Vec::new(),
        role: AssetRole::Attachment,
        bookmarked_media_hash: Some(hash.into()),
    }
}

#[tokio::test]
async fn media_bookmarks_reuse_image_and_video_without_writing_blobs_and_keep_original_source() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("media", FakeNetwork::default()));
    let blobs = Arc::new(CountedBlobs::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        Arc::new(MemoryDocsSync::default()),
        blobs.clone(),
        generate_keys(),
    );
    let topic = "kukuri:topic:media-bookmarks";
    let source = app
        .create_post_with_attachments(
            topic,
            "source",
            None,
            vec![
                pending_image_attachment("image/png", b"image"),
                pending_video_attachment(AssetRole::VideoManifest, "video/mp4", b"video"),
                pending_video_attachment(AssetRole::VideoPoster, "image/jpeg", b"poster"),
                pending_video_attachment(AssetRole::VideoManifest, "video/mp4", b"second-video"),
                pending_video_attachment(AssetRole::VideoPoster, "image/jpeg", b"second-poster"),
            ],
        )
        .await
        .unwrap();
    let original = app
        .list_timeline(topic, None, 10)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|post| post.object_id == source)
        .unwrap();
    let image = app
        .bookmark_media(
            topic,
            &source,
            &original.attachments[0].hash,
            ChannelRef::Public,
        )
        .await
        .unwrap();
    let video = app
        .bookmark_media(
            topic,
            &source,
            &original.attachments[2].hash,
            ChannelRef::Public,
        )
        .await
        .unwrap();
    assert_eq!(video.attachments.len(), 2);
    assert_eq!(video.hash, original.attachments[1].hash);
    assert!(
        store
            .list_bookmarked_posts_page(None, false)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(blobs.media_writes.load(Ordering::SeqCst), 5);
    let reused = app
        .create_post_with_attachments(
            topic,
            "reuse",
            None,
            vec![reuse(&image.hash), reuse(&video.hash)],
        )
        .await
        .unwrap();
    let post = app
        .list_timeline(topic, None, 10)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|post| post.object_id == reused)
        .unwrap();
    assert_eq!(
        post.attachments
            .iter()
            .map(|asset| &asset.hash)
            .collect::<Vec<_>>(),
        original
            .attachments
            .iter()
            .take(3)
            .map(|asset| &asset.hash)
            .collect::<Vec<_>>()
    );
    assert_eq!(blobs.media_writes.load(Ordering::SeqCst), 5);
    for asset in &post.attachments {
        assert_eq!(asset.origin.as_ref().unwrap().object_id, source);
    }
    app.bookmark_media(topic, &reused, &image.hash, ChannelRef::Public)
        .await
        .unwrap();
    assert_eq!(
        app.get_bookmarked_media(&image.hash)
            .await
            .unwrap()
            .unwrap()
            .source,
        image.source
    );
    app.remove_bookmarked_media(MediaShelf::Bookmarks, &image.hash)
        .await
        .unwrap();
    assert!(
        app.get_bookmarked_media(&image.hash)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_bookmarked_media(&video.hash)
            .await
            .unwrap()
            .is_some()
    );
}

/// #1949: 投稿の画像・動画を表示のために取得すると、キャッシュの一覧へ載る（poster の取得は動画の組で載る）。ブックマーク
/// するとキャッシュから外れ、外すと戻る。キャッシュから外せる。自分の添付を除くのは保存の側（SQLite・IndexedDB）が行う。
#[tokio::test]
async fn displayed_media_enters_the_cache_shelf_and_moves_with_bookmarks() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("media", FakeNetwork::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:media-cache";
    let source = app
        .create_post_with_attachments(
            topic,
            "source",
            None,
            vec![
                pending_video_attachment(AssetRole::VideoManifest, "video/mp4", b"video"),
                pending_video_attachment(AssetRole::VideoPoster, "image/jpeg", b"poster"),
            ],
        )
        .await
        .unwrap();
    let post = app
        .list_timeline(topic, None, 10)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|post| post.object_id == source)
        .unwrap();
    let (video, poster) = (&post.attachments[0].hash, &post.attachments[1].hash);
    let cached = async || {
        app.list_bookmarked_media(MediaShelf::Cache, None, false)
            .await
            .unwrap()
            .items
    };
    assert!(
        app.blob_media_payload_for_post(poster, "image/jpeg", Some(&source))
            .await
            .unwrap()
            .is_some()
    );
    let items = cached().await;
    assert_eq!(items.len(), 1);
    assert_eq!(&items[0].hash, video);
    assert_eq!(items[0].attachments.len(), 2);

    app.bookmark_media(topic, &source, video, ChannelRef::Public)
        .await
        .unwrap();
    assert!(cached().await.is_empty());
    app.remove_bookmarked_media(MediaShelf::Bookmarks, video)
        .await
        .unwrap();
    assert_eq!(cached().await.len(), 1);
    app.remove_bookmarked_media(MediaShelf::Cache, video)
        .await
        .unwrap();
    assert!(cached().await.is_empty());
}

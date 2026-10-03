//! 投稿者が書くリンクプレビューの record と、Web の読取り(ADR 0051 §7、#1220 W8 AC-2d)。
//!
//! 投稿者本人の native が自分の公開投稿の record を書き、手元に replica を持たない閲覧者(Web)が投稿者の docs author と
//! key の組で 1 件読んで検証する。投稿者以外が書いた record、URL が一致しない record、上限を超えた record は読まない。

use super::shadowing_docs::ShadowingDocsSync;
use super::*;
use kukuri_core::{
    KukuriLinkPreviewContentV1, KukuriLinkPreviewImageV1, LINK_PREVIEW_KIND,
    build_link_preview_envelope, sign_envelope_json,
};

const URL: &str = "https://example.test/article?q=1";

fn author_docs_author() -> String {
    "f0".repeat(32)
}

fn png() -> Vec<u8> {
    let mut bytes = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend([7_u8; 24]);
    bytes
}

fn input(url: &str, title: &str, image: Option<Vec<u8>>) -> LinkPreviewRecordInput {
    LinkPreviewRecordInput {
        url: url.into(),
        title: title.into(),
        description: Some("説明".into()),
        site_name: "Example".into(),
        image,
    }
}

fn app(
    docs_sync: Arc<dyn DocsSync>,
    blobs: Arc<MemoryBlobService>,
    keys: KukuriKeys,
) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync,
        blobs,
        keys,
    );
    (app, store)
}

struct Fixture {
    docs: Arc<ShadowingDocsSync>,
    writer: AppService,
    /// 手元に replica を持たず、投稿者の docs を provider として読む閲覧者(Web)。
    viewer: AppService,
    author_keys: KukuriKeys,
    replica: ReplicaId,
    post_id: String,
    key: String,
}

async fn fixture(name: &str) -> Fixture {
    let docs = Arc::new(ShadowingDocsSync::with_account_docs_author(
        author_docs_author(),
    ));
    let blobs = Arc::new(MemoryBlobService::default());
    let author_keys = generate_keys();
    let (writer, writer_store) = app(docs.clone(), blobs.clone(), author_keys.clone());
    let topic = format!("kukuri:topic:link-preview-{name}");
    let post_id = writer
        .create_post(&topic, &format!("見て {URL} です"), None)
        .await
        .expect("create post");
    let row = writer_store
        .get_object_projection(&EnvelopeId::from(post_id.as_str()))
        .await
        .expect("read the row")
        .expect("projected post");
    assert_eq!(
        row.source_docs_author.as_deref(),
        Some(author_docs_author().as_str())
    );
    let provider: Arc<dyn DocsSync> = Arc::new(docs.as_remote_reader("provider"));
    let (viewer, viewer_store) = app(
        Arc::new(CountingDocsSync::reading_from(provider)),
        blobs,
        generate_keys(),
    );
    let replica = row.source_replica_id.clone();
    viewer_store
        .put_object_projection(row)
        .await
        .expect("viewer row");
    Fixture {
        docs,
        writer,
        viewer,
        author_keys,
        replica,
        key: stable_key("link-previews", &format!("{post_id}/state")),
        post_id,
    }
}

impl Fixture {
    async fn read(&self) -> Option<LinkPreviewRecordView> {
        self.viewer
            .link_preview_record(&self.post_id, URL)
            .await
            .expect("read the record")
    }

    /// 投稿者の docs author の名義で、key へ任意の envelope を置く。
    async fn put(&self, envelope: &KukuriEnvelope) {
        self.docs
            .apply_doc_op(
                &self.replica,
                DocOp::SetJson {
                    key: self.key.clone(),
                    value: serde_json::to_value(envelope).expect("envelope json"),
                },
            )
            .await
            .expect("write the record");
    }

    fn content(&self, url: &str, title: &str) -> KukuriLinkPreviewContentV1 {
        KukuriLinkPreviewContentV1 {
            target_object_id: EnvelopeId::from(self.post_id.as_str()),
            url: url.into(),
            title: title.into(),
            description: None,
            site_name: "Example".into(),
            image: None,
        }
    }
}

#[tokio::test]
async fn the_author_writes_the_record_once_and_web_reads_it_with_the_image() {
    let fixture = fixture("roundtrip").await;
    assert_eq!(fixture.read().await, None, "no record yet");

    fixture
        .writer
        .record_link_preview(&fixture.post_id, input(URL, "記事", Some(png())))
        .await
        .expect("write the record");
    let view = fixture.read().await.expect("the record");
    assert_eq!(
        (
            view.url.as_str(),
            view.title.as_str(),
            view.source_label.as_str()
        ),
        (URL, "記事", "Example")
    );
    assert_eq!(view.description.as_deref(), Some("説明"));
    assert_eq!(
        view.image_data_url,
        Some(format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(png())
        ))
    );

    // 取得し直しても書き換えない(取得のたびに画像の blob を増やさない)。
    fixture
        .writer
        .record_link_preview(&fixture.post_id, input(URL, "更新後", None))
        .await
        .expect("second write");
    assert_eq!(fixture.read().await.expect("the record").title, "記事");
}

#[tokio::test]
async fn only_the_author_writes_a_record_for_a_url_in_the_post() {
    let fixture = fixture("writer-guard").await;
    assert!(
        fixture
            .viewer
            .record_link_preview(&fixture.post_id, input(URL, "他人", None))
            .await
            .is_err(),
        "a viewer must not write for someone else's post"
    );
    assert!(
        fixture
            .writer
            .record_link_preview(
                &fixture.post_id,
                input("https://other.test/", "別の URL", None)
            )
            .await
            .is_err(),
        "the url must be in the post"
    );
    assert_eq!(fixture.read().await, None);
}

#[tokio::test]
async fn web_ignores_records_by_others_mismatched_urls_and_over_the_limits() {
    let fixture = fixture("reader-guard").await;
    let valid = build_link_preview_envelope(&fixture.author_keys, &fixture.content(URL, "正しい"))
        .expect("valid record");

    // 投稿者の署名でも、別の docs author の名義の record は読まない。
    fixture
        .docs
        .shadow(&fixture.key, serde_json::to_value(&valid).expect("json"))
        .await;
    assert_eq!(
        fixture.read().await,
        None,
        "record under another docs author"
    );

    let stranger = build_link_preview_envelope(&generate_keys(), &fixture.content(URL, "偽"))
        .expect("stranger record");
    fixture.put(&stranger).await;
    assert_eq!(fixture.read().await, None, "record signed by someone else");

    let other_url = build_link_preview_envelope(
        &fixture.author_keys,
        &fixture.content("https://example.test/other", "別の URL"),
    )
    .expect("other url record");
    fixture.put(&other_url).await;
    assert_eq!(fixture.read().await, None, "record for another url");

    let long_title = sign_envelope_json(
        &fixture.author_keys,
        LINK_PREVIEW_KIND,
        Vec::new(),
        &fixture.content(URL, &"あ".repeat(201)),
    )
    .expect("long title record");
    fixture.put(&long_title).await;
    assert_eq!(fixture.read().await, None, "title over the limit");

    let mut large_image = fixture.content(URL, "大きい画像");
    large_image.image = Some(KukuriLinkPreviewImageV1 {
        hash: kukuri_core::blob_hash(&png()),
        mime: "image/png".into(),
        bytes: 1024 * 1024 + 1,
    });
    let large_image = sign_envelope_json(
        &fixture.author_keys,
        LINK_PREVIEW_KIND,
        Vec::new(),
        &large_image,
    )
    .expect("large image record");
    fixture.put(&large_image).await;
    assert_eq!(fixture.read().await, None, "image over the limit");

    fixture.put(&valid).await;
    assert_eq!(fixture.read().await.expect("valid record").title, "正しい");
}

#[tokio::test]
async fn web_keeps_the_text_when_the_image_cannot_be_read() {
    let fixture = fixture("missing-image").await;
    let mut content = fixture.content(URL, "画像なし");
    content.image = Some(KukuriLinkPreviewImageV1 {
        hash: kukuri_core::blob_hash(b"not stored"),
        mime: "image/png".into(),
        bytes: 10,
    });
    fixture
        .put(&build_link_preview_envelope(&fixture.author_keys, &content).expect("record"))
        .await;
    let view = fixture.read().await.expect("text preview");
    assert_eq!(view.title, "画像なし");
    assert_eq!(view.image_data_url, None);
}

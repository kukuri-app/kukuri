//! #1984: ゲームのアセットの更新の hint を受けたら、hint を届けた peer を参加者より先に読む(参加者の候補は回るので、
//! 新しい release を持つ peer が入らないことがある)。

use super::*;
use crate::GAME_ASSET_HINT_KIND;
use kukuri_core::{GameAssetFormat, GameAssetReleaseV1, build_game_asset_release_envelope};
use kukuri_docs_sync::{author_replica_id, stable_key};
use kukuri_transport::{HintStream, SeedPeer};

const TOPIC: &str = "kukuri:topic:game-asset-hint-source";

/// topic の参加者の候補は、release を持たない peer だけ。
struct OtherReadPeer(Arc<StaticTransport>);

#[async_trait]
impl HintTransport for OtherReadPeer {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        self.0.subscribe_hints(topic).await
    }

    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        self.0.unsubscribe_hints(topic).await
    }

    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        self.0.publish_hint(topic, hint).await
    }

    async fn topic_read_candidates(&self, _: &TopicId) -> Result<Vec<SeedPeer>> {
        Ok(vec![SeedPeer {
            endpoint_id: "other".into(),
            addr_hint: None,
        }])
    }
}

#[tokio::test]
async fn game_asset_hint_reads_the_peer_that_delivered_it() {
    let publisher = generate_keys();
    let holder = Arc::new(CountingDocsSync::default());
    let release = GameAssetReleaseV1 {
        application_id: "com.example.game".into(),
        asset_id: "skin".into(),
        revision: 1,
        blob_hash: BlobHash::new("a".repeat(64)),
        bytes: 1,
        format: GameAssetFormat::Glb,
        published_at: Utc::now().timestamp_millis(),
    };
    let envelope = build_game_asset_release_envelope(&publisher, &release).expect("sign");
    holder
        .apply_doc_op(
            &author_replica_id(&publisher.public_key_hex()),
            DocOp::SetJson {
                key: stable_key("game-assets", "com.example.game/skin"),
                value: serde_json::to_value(&envelope).expect("json"),
            },
        )
        .await
        .expect("hold the release");
    let root = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteStore::connect_file(root.path().join("game.db"))
            .await
            .expect("sqlite"),
    );
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        Arc::new(OtherReadPeer(transport)),
        Arc::new(CountingDocsSync::reading_from_peers(HashMap::from([
            (
                "other".to_string(),
                Arc::new(CountingDocsSync::default()) as Arc<dyn DocsSync>,
            ),
            ("holder".to_string(), holder as Arc<dyn DocsSync>),
        ]))),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let hint = GossipHint::SessionChanged {
        topic_id: TopicId::new(TOPIC),
        session_id: format!("{}/skin/com.example.game", publisher.public_key_hex()),
        object_kind: GAME_ASSET_HINT_KIND.into(),
        sent_at: None,
    };
    let mut updates = app.subscribe_game_asset_updates();
    assert_eq!(
        app.apply_content_hint(TOPIC, &TimelineScope::Public, &hint, Some("holder"))
            .await,
        1
    );
    assert_eq!(updates.try_recv().expect("notified").revision, 1);
}

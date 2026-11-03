//! #1984: 適用中の版があるアセットの取得は、新しい版の確認を待たずに適用中の版を返す（2026-11-04 のユーザー判断）。
//! 確認は背景で行う。読み先が応答しなくても、SDK の他の command を止めない。

use super::*;
use kukuri_core::{GameAssetFormat, GameAssetReleaseV1};
use kukuri_store::AppliedGameAssetRow;
use kukuri_transport::{HintStream, SeedPeer};

const TOPIC: &str = "kukuri:topic:game-asset-applied-fetch";

/// topic の参加者の候補は、応答しない peer だけ。
struct StalledReadPeer(Arc<StaticTransport>);

#[async_trait]
impl HintTransport for StalledReadPeer {
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
            endpoint_id: "stalled".into(),
            addr_hint: None,
        }])
    }
}

/// 読みに応答しない peer。
#[derive(Default)]
struct Stalled;

#[async_trait]
impl DocsSync for Stalled {
    async fn open_replica(&self, _: &ReplicaId) -> Result<()> {
        Ok(())
    }

    async fn apply_doc_op(&self, _: &ReplicaId, _: DocOp) -> Result<()> {
        Ok(())
    }

    async fn query_replica_with_policy(
        &self,
        _: &ReplicaId,
        _: DocQuery,
        _: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        std::future::pending().await
    }

    async fn subscribe_replica(&self, _: &ReplicaId) -> Result<kukuri_docs_sync::DocEventStream> {
        Ok(Box::pin(futures_util::stream::pending()))
    }

    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn an_applied_asset_is_returned_without_waiting_for_the_newer_release_check() {
    let publisher = generate_keys().public_key_hex();
    let release = GameAssetReleaseV1 {
        application_id: "com.example.game".into(),
        asset_id: "skin".into(),
        revision: 1,
        blob_hash: BlobHash::new("a".repeat(64)),
        bytes: 1,
        format: GameAssetFormat::Glb,
        published_at: Utc::now().timestamp_millis(),
    };
    let root = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteStore::connect_file(root.path().join("game.db"))
            .await
            .expect("sqlite"),
    );
    let path = root.path().join("1.glb");
    assert!(
        store
            .apply_game_asset(
                &AppliedGameAssetRow {
                    publisher_pubkey: publisher.clone(),
                    application_id: release.application_id.clone(),
                    asset_id: release.asset_id.clone(),
                    revision: 1,
                    release_json: serde_json::to_string(&release).expect("json"),
                    file_path: path.to_string_lossy().into_owned(),
                    applied_at: Utc::now().timestamp_millis(),
                },
                &[],
            )
            .await
            .expect("apply")
    );
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        Arc::new(StalledReadPeer(transport)),
        Arc::new(CountingDocsSync::reading_from_peers(HashMap::from([(
            "stalled".to_string(),
            Arc::new(Stalled) as Arc<dyn DocsSync>,
        )]))),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let fetched = timeout(
        Duration::from_secs(1),
        app.fetch_game_asset(TOPIC, &publisher, "com.example.game", "skin"),
    )
    .await
    .expect("the applied asset is returned without waiting for the readers")
    .expect("fetch");
    assert_eq!(fetched.release.revision, 1);
    assert_eq!(fetched.path, path.to_string_lossy());
}

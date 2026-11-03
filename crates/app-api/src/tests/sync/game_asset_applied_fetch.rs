//! #1984: 適用中の版があるアセットの取得は、新しい版の確認を待たずに適用中の版を返す（2026-11-04 のユーザー判断）。
//! 確認と取得は背景で行い、新しい版を適用したら `game_asset_updated` で path とともに知らせる。読み先が応答しなくても、
//! SDK の他の command を止めない。

use super::*;
use kukuri_core::{GameAssetFormat, GameAssetReleaseV1, build_game_asset_release_envelope};
use kukuri_docs_sync::{author_replica_id, stable_key};
use kukuri_store::AppliedGameAssetRow;
use kukuri_transport::{HintStream, SeedPeer};

const TOPIC: &str = "kukuri:topic:game-asset-applied-fetch";
const APPLICATION: &str = "com.example.game";

/// topic の参加者の候補は `peer` だけ。
struct ReadPeer(Arc<StaticTransport>, &'static str);

#[async_trait]
impl HintTransport for ReadPeer {
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
            endpoint_id: self.1.into(),
            addr_hint: None,
        }])
    }
}

/// 読みに応答しない peer。
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

fn release(revision: u64, blob_hash: BlobHash, bytes: u64) -> GameAssetReleaseV1 {
    GameAssetReleaseV1 {
        application_id: APPLICATION.into(),
        asset_id: "skin".into(),
        revision,
        blob_hash,
        bytes,
        format: GameAssetFormat::Glb,
        published_at: Utc::now().timestamp_millis(),
    }
}

/// 版 1 を適用済みの端末。読み先は `peer`（`docs`）だけ。
async fn applied_player(
    root: &std::path::Path,
    publisher: &str,
    peer: &'static str,
    docs: Arc<dyn DocsSync>,
    blobs: Arc<MemoryBlobService>,
) -> (AppService, String) {
    let store = Arc::new(
        SqliteStore::connect_file(root.join("game.db"))
            .await
            .expect("sqlite"),
    );
    let path = root.join("1.glb").to_string_lossy().into_owned();
    let applied = AppliedGameAssetRow {
        publisher_pubkey: publisher.into(),
        application_id: APPLICATION.into(),
        asset_id: "skin".into(),
        revision: 1,
        release_json: serde_json::to_string(&release(1, BlobHash::new("a".repeat(64)), 1))
            .expect("json"),
        file_path: path.clone(),
        applied_at: Utc::now().timestamp_millis(),
    };
    assert!(store.apply_game_asset(&applied, &[]).await.expect("apply"));
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = AppService::from_handles(
        ServiceHandles::new(
            store.clone(),
            store,
            transport.clone(),
            Arc::new(ReadPeer(transport, peer)),
            Arc::new(CountingDocsSync::reading_from_peers(HashMap::from([(
                peer.to_string(),
                docs,
            )]))),
            blobs,
            generate_keys(),
        )
        .with_game_assets(root.join("game-assets")),
    );
    (app, path)
}

#[tokio::test]
async fn an_applied_asset_is_returned_without_waiting_for_the_newer_release_check() {
    let root = tempfile::tempdir().expect("tempdir");
    let publisher = generate_keys().public_key_hex();
    let (app, path) = applied_player(
        root.path(),
        &publisher,
        "stalled",
        Arc::new(Stalled),
        Arc::new(MemoryBlobService::default()),
    )
    .await;
    let fetched = timeout(
        Duration::from_secs(1),
        app.fetch_game_asset(TOPIC, &publisher, APPLICATION, "skin"),
    )
    .await
    .expect("the applied asset is returned without waiting for the readers")
    .expect("fetch");
    assert_eq!(fetched.release.revision, 1);
    assert_eq!(fetched.path, path);
}

#[tokio::test]
async fn a_newer_release_found_after_returning_the_applied_one_is_announced_with_its_path() {
    let root = tempfile::tempdir().expect("tempdir");
    let publisher = generate_keys();
    let blobs = Arc::new(MemoryBlobService::default());
    let body = b"the second skin".to_vec();
    let stored = blobs
        .put_blob(body.clone(), "model/gltf-binary")
        .await
        .expect("blob");
    let envelope =
        build_game_asset_release_envelope(&publisher, &release(2, stored.hash, body.len() as u64))
            .expect("sign");
    let holder = Arc::new(CountingDocsSync::default());
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
    let (app, _) = applied_player(
        root.path(),
        &publisher.public_key_hex(),
        "holder",
        holder,
        blobs,
    )
    .await;
    let mut updates = app.subscribe_game_asset_updates();
    let fetched = app
        .fetch_game_asset(TOPIC, &publisher.public_key_hex(), APPLICATION, "skin")
        .await
        .expect("fetch");
    assert_eq!(fetched.release.revision, 1);
    let update = timeout(Duration::from_secs(10), updates.recv())
        .await
        .expect("the newer release is announced")
        .expect("update");
    assert_eq!(update.revision, 2);
    assert_eq!(
        std::fs::read(update.path.expect("the applied file")).expect("read"),
        body
    );
}

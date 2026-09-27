//! #1221 R5-H: 別の端末の owner が提案・受諾した Dome の接続の記録を、topology の表示で provider から読む(実 Iroh)。

use super::*;
use crate::{AcceptDomeConnectionProposalInput, CreateDomeConnectionProposalInput};
use kukuri_blob_service::DisplayBlobFetch;
use kukuri_core::{BlobHash, DomeDirection, SpatialContextV1};
use kukuri_transport::{EndpointAddr, SeedPeer};

/// 指定した hash の blob を手元にも provider からも取れない blob service(取得の失敗・cooldown の場面)。ほかは実 Iroh へ渡す。
struct WithheldBlob {
    inner: Arc<kukuri_blob_service::IrohBlobService>,
    withheld: TokioMutex<Option<BlobHash>>,
}

impl WithheldBlob {
    async fn withheld(&self, hash: &BlobHash) -> bool {
        self.withheld.lock().await.as_ref() == Some(hash)
    }
}

#[async_trait]
impl BlobService for WithheldBlob {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }
    async fn put_remote_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_remote_blob(data, mime).await
    }
    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        if self.withheld(hash).await {
            return Ok(None);
        }
        self.inner.fetch_blob(hash).await
    }
    async fn fetch_local_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        if self.withheld(hash).await {
            return Ok(None);
        }
        self.inner.fetch_local_blob(hash).await
    }
    async fn fetch_blob_ephemeral(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        if self.withheld(hash).await {
            return Ok(None);
        }
        self.inner.fetch_blob_ephemeral(hash).await
    }
    async fn prepare_display_fetch(&self, hash: &BlobHash) -> Result<DisplayBlobFetch> {
        self.inner.prepare_display_fetch(hash).await
    }
    async fn fetch_verified_receive_offer_payload(
        &self,
        offer: &kukuri_core::VerifiedReceiveOffer,
        provider: EndpointAddr,
    ) -> Result<Vec<u8>> {
        self.inner
            .fetch_verified_receive_offer_payload(offer, provider)
            .await
    }
    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }
    async fn unpin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.unpin_blob(hash).await
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
    async fn learn_peer(&self, endpoint_id: &str) -> Result<()> {
        self.inner.learn_peer(endpoint_id).await
    }
    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        self.inner.set_seed_peers(peers).await
    }
    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        self.inner.assist_peer_ids().await
    }
}

async fn wait_for_topology(
    app: &AppService,
    context: &SpatialContextV1,
    found: impl Fn(&DomeConnectionTopologyView) -> bool,
) -> bool {
    timeout(Duration::from_secs(60), async {
        loop {
            let topology = app
                .list_dome_connection_topology(context.clone())
                .await
                .expect("topology");
            if found(&topology) {
                return;
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .is_ok()
}

async fn create_dome(app: &AppService, topic: &str, title: &str) -> String {
    app.create_metaverse_room(
        topic,
        CreateMetaverseRoomInput {
            title: title.into(),
            description: String::new(),
            max_peers: Some(4),
        },
    )
    .await
    .expect("create Dome")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_owners_proposal_and_acceptance_reach_the_topology() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("connection-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("connection-b")).await;
    let app_a = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_a);
    // b は a の Dome Instance の manifest blob を取れない(CI で起きた取得の失敗と同じ場面。#1221 R5-H)。
    let blob_b = Arc::new(WithheldBlob {
        inner: stack_b.blob_service.clone(),
        withheld: TokioMutex::new(None),
    });
    let store_b = Arc::new(MemoryStore::default());
    let app_b = app_service_from_dependencies(
        store_b.clone(),
        store_b,
        stack_b.transport.clone(),
        stack_b.transport.clone(),
        stack_b.docs_sync.clone(),
        blob_b.clone(),
        generate_keys(),
    );
    app_a.switch_writer(1);
    app_b.switch_writer(1);
    let topic = "kukuri:topic:dome-connection-remote";
    display_topic_in(&[&app_a, &app_b], topic).await;
    let ticket_a = app_a.peer_ticket().await.unwrap().unwrap();
    let ticket_b = app_b.peer_ticket().await.unwrap().unwrap();
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    app_b.import_peer_ticket(&ticket_a).await.expect("import a");
    let dome_a = create_dome(&app_a, topic, "proposer").await;
    let dome_b = create_dome(&app_b, topic, "receiver").await;
    display_remote_session(&app_a, topic, &dome_b, "game").await;
    display_remote_session(&app_b, topic, &dome_a, "game").await;
    for (app, dome) in [(&app_a, &dome_b), (&app_b, &dome_a)] {
        timeout(Duration::from_secs(60), async {
            while !app
                .list_game_rooms(topic)
                .await
                .expect("rooms")
                .iter()
                .any(|room| room.room_id == *dome)
            {
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("the other Dome is listed");
    }
    // 一覧に出た後に、b は a の Instance の manifest blob を取れなくなる。
    let (instance_a, _) = app_a
        .fetch_dome_instance_manifest(
            &SpatialContextV1::Topic {
                topic_id: TopicId::new(topic),
            },
            &app_a.keys().public_key(),
        )
        .await
        .expect("instance")
        .expect("a's instance");
    *blob_b.withheld.lock().await = Some(instance_a.current_manifest.hash);
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    app_a
        .create_dome_connection_proposal(CreateDomeConnectionProposalInput {
            proposal_id: "remote-connection".into(),
            spatial_context: context.clone(),
            proposer_instance_id: dome_a.clone(),
            receiver_instance_id: dome_b.clone(),
            proposer_direction: DomeDirection::East,
        })
        .await
        .expect("propose");
    assert!(
        wait_for_topology(&app_b, &context, |topology| topology
            .proposals
            .iter()
            .any(|item| item.proposal.proposal_id == "remote-connection"))
        .await,
        "the receiver reads the proposal from the proposer's device"
    );
    let connection = app_b
        .accept_dome_connection_proposal(AcceptDomeConnectionProposalInput {
            spatial_context: context.clone(),
            proposal_id: "remote-connection".into(),
        })
        .await
        .expect("accept");
    let connection_id = connection.record.agreement.connection_id.clone();
    assert!(
        wait_for_topology(&app_a, &context, |topology| topology
            .resolution
            .topology
            .active_connection_ids
            .contains(&connection_id))
        .await,
        "the proposer reads the acceptance from the receiver's device"
    );
}

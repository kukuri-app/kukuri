//! #1221 R5-H: 別の端末の owner が提案・受諾した Dome の接続の記録を、topology の表示で provider から読む(実 Iroh)。

use super::*;
use crate::{AcceptDomeConnectionProposalInput, CreateDomeConnectionProposalInput};
use kukuri_core::{DomeDirection, SpatialContextV1};

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
    let app_b = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_b);
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

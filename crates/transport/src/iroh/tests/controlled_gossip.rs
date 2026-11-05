//! D9 feasibility: retain the upstream membership/wire state machine while the
//! caller owns the selected connection, reads, and hard cancellation boundary.
use super::*;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh_gossip::proto::topic::{InEvent, OutEvent};
use iroh_gossip::proto::{self, Command};
use rand::{SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// The upstream net::util module is private. This is its wire contract (one
// topic ID in a postcard header, then u32-BE length-prefixed postcard messages),
// not a copied networking actor or membership implementation.
#[derive(Serialize, Deserialize)]
struct StreamHeader {
    topic_id: GossipTopicId,
}

async fn write_frame<T: Serialize>(stream: &mut SendStream, value: &T) {
    let bytes = postcard::to_allocvec(value).unwrap();
    assert!(bytes.len() < proto::DEFAULT_MAX_MESSAGE_SIZE);
    stream.write_u32(bytes.len() as u32).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
}

async fn read_frame<T: DeserializeOwned>(stream: &mut RecvStream) -> T {
    let length = stream.read_u32().await.unwrap() as usize;
    assert!(length <= proto::DEFAULT_MAX_MESSAGE_SIZE);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await.unwrap();
    postcard::from_bytes(&bytes).unwrap()
}

struct OwnedConnection(Connection);

impl Drop for OwnedConnection {
    fn drop(&mut self) {
        self.0.close(0_u32.into(), b"demand ended");
    }
}

async fn emit_to_selected_peer(
    output: Vec<OutEvent<EndpointId>>,
    selected: EndpointId,
    stream: &mut SendStream,
) -> Vec<proto::Event<EndpointId>> {
    let mut events = Vec::new();
    for event in output {
        match event {
            OutEvent::SendMessage(peer, message) => {
                // Production owner admission will choose destinations before I/O.
                // This fixture has precisely one authorized peer and no dial loop.
                assert_eq!(peer, selected, "state proposed an unselected destination");
                write_frame(stream, &message).await;
            }
            OutEvent::EmitEvent(event) => events.push(event),
            // This short handshake/roundtrip does not advance periodic timers.
            // Timer admission and maintenance remain explicit production work.
            OutEvent::ScheduleTimer(_, _) | OutEvent::PeerData(_, _) => {}
            OutEvent::DisconnectPeer(_) => panic!("unexpected disconnect during roundtrip"),
            other => panic!("unexpected gossip output during roundtrip: {other:?}"),
        }
    }
    events
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_gossip_state_roundtrips_with_native_peer_on_owned_connection() {
    let mut native = IrohGossipTransport::bind_local().await.unwrap();
    let topic = topic_to_gossip_id(&TopicId::new("controlled-gossip-contract"));
    let mut native_topic = native.gossip.subscribe(topic, Vec::new()).await.unwrap();
    let controlled = EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let connection = OwnedConnection(
        controlled
            .connect(native.endpoint.addr(), GOSSIP_ALPN)
            .await
            .unwrap(),
    );
    let mut state = proto::topic::State::with_rng(
        controlled.id(),
        Some(proto::PeerData::default()),
        proto::Config::default(),
        StdRng::seed_from_u64(7),
    );
    timeout(Duration::from_secs(10), async {
        let mut send = connection.0.open_uni().await.unwrap();
        write_frame(&mut send, &StreamHeader { topic_id: topic }).await;
        let output = state
            .handle(
                InEvent::Command(Command::Join(vec![native.endpoint.id()])),
                n0_future::time::Instant::now(),
            )
            .collect();
        emit_to_selected_peer(output, native.endpoint.id(), &mut send).await;
        let mut recv = connection.0.accept_uni().await.unwrap();
        let header: StreamHeader = read_frame(&mut recv).await;
        assert_eq!(header.topic_id, topic);
        loop {
            let message: proto::topic::Message<EndpointId> = read_frame(&mut recv).await;
            let output = state
                .handle(
                    InEvent::RecvMessage(native.endpoint.id(), message),
                    n0_future::time::Instant::now(),
                )
                .collect();
            let events = emit_to_selected_peer(output, native.endpoint.id(), &mut send).await;
            if events
                .iter()
                .any(|event| matches!(event, proto::Event::NeighborUp(_)))
            {
                break;
            }
        }
        // Native NeighborUp requires the protocol response we just drove.
        native_topic.joined().await.unwrap();
        let output = state
            .handle(
                InEvent::Command(Command::Broadcast(
                    b"owned to native".to_vec().into(),
                    proto::Scope::Swarm,
                )),
                n0_future::time::Instant::now(),
            )
            .collect();
        emit_to_selected_peer(output, native.endpoint.id(), &mut send).await;
        loop {
            if let GossipEvent::Received(message) = native_topic.next().await.unwrap().unwrap() {
                assert_eq!(message.content.as_ref(), b"owned to native");
                break;
            }
        }
        native_topic
            .broadcast(b"native to owned".to_vec().into())
            .await
            .unwrap();
        loop {
            let message: proto::topic::Message<EndpointId> = read_frame(&mut recv).await;
            let output = state
                .handle(
                    InEvent::RecvMessage(native.endpoint.id(), message),
                    n0_future::time::Instant::now(),
                )
                .collect();
            let events = emit_to_selected_peer(output, native.endpoint.id(), &mut send).await;
            if let Some(proto::Event::Received(message)) = events
                .iter()
                .find(|event| matches!(event, proto::Event::Received(_)))
            {
                assert_eq!(message.content.as_ref(), b"native to owned");
                break;
            }
        }
        assert!(state.has_active_peers());
        let _: Vec<_> = state
            .handle(
                InEvent::Command(Command::Quit),
                n0_future::time::Instant::now(),
            )
            .collect();
        assert!(!state.has_active_peers());
    })
    .await
    .unwrap();
    drop(connection);
    drop(native_topic);
    native._router.take().unwrap().shutdown().await.unwrap();
    controlled.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_gossip_read_cancellation_closes_even_an_incomplete_header() {
    let local = EndpointBuilder::new(presets::Minimal)
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let remote = EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let (reading, ready) = tokio::sync::oneshot::channel();
    let accepted = local.clone();
    let task = tokio::spawn(async move {
        let connection = OwnedConnection(accepted.accept().await.unwrap().await.unwrap());
        let mut stream = connection.0.accept_uni().await.unwrap();
        reading.send(()).unwrap();
        let _: StreamHeader = read_frame(&mut stream).await;
    });
    let connection = remote.connect(local.addr(), GOSSIP_ALPN).await.unwrap();
    let mut stream = connection.open_uni().await.unwrap();
    stream.write_all(&[0, 0]).await.unwrap(); // Hold half of the length header open.
    timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    timeout(Duration::from_secs(5), connection.closed())
        .await
        .unwrap();
    assert!(
        !local.is_closed(),
        "cancel a request without destroying its endpoint"
    );
    local.close().await;
    remote.close().await;
}

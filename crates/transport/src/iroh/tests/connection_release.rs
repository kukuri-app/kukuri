use super::*;

#[derive(Debug)]
struct ObserveGossipConnection {
    gossip: Gossip,
    observed: tokio::sync::mpsc::Sender<iroh::endpoint::WeakConnectionHandle>,
}

impl iroh::protocol::ProtocolHandler for ObserveGossipConnection {
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        let _ = self.observed.try_send(connection.weak_handle());
        iroh::protocol::ProtocolHandler::accept(&self.gossip, connection).await
    }
}

async fn join_pair(
    left: &IrohGossipTransport,
    right: &IrohGossipTransport,
    name: &str,
) -> (iroh_gossip::api::GossipTopic, iroh_gossip::api::GossipTopic) {
    let topic = topic_to_gossip_id(&TopicId::new(name));
    let mut left_topic = left
        .gossip
        .subscribe(topic, vec![right.endpoint.id()])
        .await
        .unwrap();
    let mut right_topic = right
        .gossip
        .subscribe(topic, vec![left.endpoint.id()])
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        tokio::try_join!(left_topic.joined(), right_topic.joined())
    })
    .await
    .unwrap()
    .unwrap();
    (left_topic, right_topic)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gossip_releases_connection_after_both_topic_leases_end() {
    let mut left = IrohGossipTransport::bind_local().await.unwrap();
    let right = EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let right_gossip = Gossip::builder().spawn(right.clone());
    let (observed, mut connections) = tokio::sync::mpsc::channel(8);
    let right_router = Router::builder(right.clone())
        .accept(
            GOSSIP_ALPN,
            ObserveGossipConnection {
                gossip: right_gossip.clone(),
                observed,
            },
        )
        .spawn();
    left.discovery.add_endpoint_info(right.addr());
    let topic = topic_to_gossip_id(&TopicId::new("lease-release-contract"));
    let mut right_topic = right_gossip.subscribe(topic, Vec::new()).await.unwrap();
    let mut left_topic = left
        .gossip
        .subscribe(topic, vec![right.id()])
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        tokio::try_join!(left_topic.joined(), right_topic.joined())
    })
    .await
    .unwrap()
    .unwrap();
    let connection = timeout(Duration::from_secs(5), connections.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(connection.upgrade().unwrap().close_reason().is_none());
    drop(left_topic);
    drop(right_topic);

    // Address usage is a cached path observation, not a connection lifetime.
    // This weak close observer does not itself keep the connection alive.
    let released = timeout(Duration::from_secs(12), connection.closed())
        .await
        .is_ok();
    left._router.take().unwrap().shutdown().await.unwrap();
    right_router.shutdown().await.unwrap();
    assert!(
        released,
        "gossip kept active connections after both topic leases ended"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gossip_keeps_other_topic_when_one_lease_ends() {
    let mut left = IrohGossipTransport::bind_local().await.unwrap();
    let mut right = IrohGossipTransport::bind_local().await.unwrap();
    left.discovery.add_endpoint_info(right.endpoint.addr());
    right.discovery.add_endpoint_info(left.endpoint.addr());
    let (closed_left, closed_right) = join_pair(&left, &right, "closed-topic").await;
    let (mut active_left, mut active_right) = join_pair(&left, &right, "active-topic").await;
    drop(closed_left);
    drop(closed_right);
    sleep(Duration::from_secs(6)).await;
    active_left
        .broadcast(b"still-needed".to_vec().into())
        .await
        .unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            match active_right.next().await.unwrap().unwrap() {
                GossipEvent::NeighborDown(_) => panic!("closing one topic disconnected the other"),
                GossipEvent::Received(message) if message.content.as_ref() == b"still-needed" => {
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    drop(active_left);
    drop(active_right);
    left._router.take().unwrap().shutdown().await.unwrap();
    right._router.take().unwrap().shutdown().await.unwrap();
}

async fn bind_gossip_endpoint() -> Endpoint {
    EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap()
}

async fn connect_gossip(
    from: &Endpoint,
    to: &Endpoint,
) -> (iroh::endpoint::Connection, iroh::endpoint::Connection) {
    let (dialed, accepted) = tokio::join!(from.connect(to.addr(), GOSSIP_ALPN), async {
        to.accept().await.unwrap().await
    });
    (dialed.unwrap(), accepted.unwrap())
}

// Two peers that dial each other at once can each keep a different connection.
// Here the right peer adopts the left's dial only after both sides opened their
// topic streams on the right's dial, which the left had already stopped sending on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gossip_keeps_connection_a_peer_switched_to_after_joining() {
    let left = bind_gossip_endpoint().await;
    let right = bind_gossip_endpoint().await;
    let left_gossip = Gossip::builder().spawn(left.clone());
    let right_gossip = Gossip::builder().spawn(right.clone());
    let (left_dial, right_accept) = connect_gossip(&left, &right).await;
    let (right_dial, left_accept) = connect_gossip(&right, &left).await;
    let switched = right_accept.weak_handle();
    left_gossip.handle_connection(left_dial).await.unwrap();
    left_gossip.handle_connection(left_accept).await.unwrap();
    right_gossip.handle_connection(right_dial).await.unwrap();
    let topic = topic_to_gossip_id(&TopicId::new("late-switch"));
    let mut left_topic = left_gossip
        .subscribe(topic, vec![right.id()])
        .await
        .unwrap();
    let mut right_topic = right_gossip
        .subscribe(topic, vec![left.id()])
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        tokio::try_join!(left_topic.joined(), right_topic.joined())
    })
    .await
    .unwrap()
    .unwrap();
    right_gossip.handle_connection(right_accept).await.unwrap();

    // The left closes a connection it stopped sending on once the right keeps no
    // stream open on it for the idle grace (5s). The right now sends on it.
    let closed = timeout(Duration::from_secs(7), switched.closed())
        .await
        .is_ok();
    assert!(!closed, "closed the connection the peer switched to");
    left_topic
        .broadcast(b"still-needed".to_vec().into())
        .await
        .unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            match right_topic.next().await.unwrap().unwrap() {
                GossipEvent::NeighborDown(_) => panic!("the peer lost its neighbor"),
                GossipEvent::Received(message) if message.content.as_ref() == b"still-needed" => {
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    drop(left_topic);
    drop(right_topic);
    left_gossip.shutdown().await.unwrap();
    right_gossip.shutdown().await.unwrap();
}

#[derive(Debug)]
struct CountGossipConnections {
    gossip: Gossip,
    count: Arc<std::sync::atomic::AtomicUsize>,
    connections: tokio::sync::mpsc::Sender<iroh::endpoint::Connection>,
}

impl iroh::protocol::ProtocolHandler for CountGossipConnections {
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        let _ = self.connections.try_send(connection.clone());
        iroh::protocol::ProtocolHandler::accept(&self.gossip, connection).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gossip_pending_join_quit_does_not_retry_and_new_demand_can_rejoin() {
    let mut client = IrohGossipTransport::bind_local().await.unwrap();
    let server = EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let gossip = Gossip::builder().spawn(server.clone());
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (connections, mut received) = tokio::sync::mpsc::channel(8);
    let router = Router::builder(server.clone())
        .accept(
            GOSSIP_ALPN,
            CountGossipConnections {
                gossip: gossip.clone(),
                count: count.clone(),
                connections,
            },
        )
        .spawn();
    client.discovery.add_endpoint_info(server.addr());
    let topic = topic_to_gossip_id(&TopicId::new("pending-join"));
    let pending = client
        .gossip
        .subscribe(topic, vec![server.id()])
        .await
        .unwrap();
    let connection = timeout(Duration::from_secs(5), received.recv())
        .await
        .unwrap()
        .unwrap();
    drop(pending);
    sleep(Duration::from_millis(100)).await;
    connection.close(0u32.into(), b"peer closed during join");
    drop(connection);
    sleep(Duration::from_secs(2)).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "a retired pending join redialed"
    );

    let mut server_topic = gossip.subscribe(topic, Vec::new()).await.unwrap();
    let mut client_topic = client
        .gossip
        .subscribe(topic, vec![server.id()])
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        tokio::try_join!(server_topic.joined(), client_topic.joined())
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    drop(server_topic);
    drop(client_topic);
    router.shutdown().await.unwrap();
    client._router.take().unwrap().shutdown().await.unwrap();
}

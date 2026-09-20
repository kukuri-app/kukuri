//! #1206: relay 受信 queue の容量不足による packet 破棄の再現と復旧確認。
//!
//! ここで扱う経路は `Relay Fallback`(実データが relay 経由)であり、通常成功経路ではない。
//! loopback では直接経路が成立してしまうため、IP transport を除いた endpoint で強制する。

use super::*;

use std::sync::mpsc as std_mpsc;

use tracing::field::{Field, Visit};
use tracing::span;

const OVERLOAD_ALPN: &[u8] = b"kukuri/test/relay-overload/1";
const RELAY_ACTOR_TARGET: &str = "iroh::socket::transports::relay::actor";
const DROP_MESSAGE_PREFIX: &str = "Dropping received relay packet";
const RECEIVER_WORKERS: usize = 2;
const WARMUP_BYTES: usize = 2 * 1024 * 1024;
const BURST_STREAMS: usize = 8;
const BURST_STREAM_BYTES: usize = 512 * 1024;

/// 受信側 runtime の thread にだけ設定し、relay actor の drop 警告を数える。
struct RelayDropCounter {
    drops: Arc<AtomicU64>,
}

struct MessageVisitor {
    is_drop: bool,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && format!("{value:?}").starts_with(DROP_MESSAGE_PREFIX) {
            self.is_drop = true;
        }
    }
}

impl tracing::Subscriber for RelayDropCounter {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.is_event()
            && *metadata.level() <= tracing::Level::WARN
            && metadata.target() == RELAY_ACTOR_TARGET
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = MessageVisitor { is_drop: false };
        event.record(&mut visitor);
        if visitor.is_drop {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

fn relay_only_builder(relay_map: iroh::RelayMap) -> EndpointBuilder {
    EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Custom(relay_map))
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .clear_ip_transports()
}

async fn send_uni(connection: &iroh::endpoint::Connection, len: usize) -> Result<()> {
    let mut send = connection.open_uni().await?;
    send.write_all(&vec![0x5a_u8; len]).await?;
    send.finish()?;
    send.stopped().await?;
    Ok(())
}

async fn wait_for_received(
    received: &mut tokio::sync::mpsc::UnboundedReceiver<usize>,
    streams: usize,
) -> usize {
    let mut total = 0;
    for _ in 0..streams {
        total += received.recv().await.expect("receiver task alive");
    }
    total
}

/// sender の送信 datagram 数が増えなくなるまで待つ(= 受信側停止で cwnd / flow control を使い切った)。
async fn wait_until_sender_stalls(connection: &iroh::endpoint::Connection) -> u64 {
    timeout(Duration::from_secs(20), async {
        let mut last = connection.stats().udp_tx.datagrams;
        let mut unchanged = 0;
        loop {
            sleep(Duration::from_millis(100)).await;
            let current = connection.stats().udp_tx.datagrams;
            if current == last {
                unchanged += 1;
                if unchanged >= 5 {
                    return current;
                }
            } else {
                unchanged = 0;
                last = current;
            }
        }
    })
    .await
    .expect("sender stalls while receiver runtime is blocked")
}

struct OverloadOutcome {
    drops_while_running: u64,
    drops_total: u64,
    sent_before_release: u64,
    sent_total: u64,
    lost_packets: u64,
    received_bytes: usize,
}

/// `senders` 台の relay-only peer から 1 台の受信 endpoint へ送り、途中で受信側 runtime の
/// 全 worker を止めてから再開する。
async fn run_receiver_stall(senders: usize) -> OverloadOutcome {
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");

    let drops = Arc::new(AtomicU64::new(0));
    let dispatch = tracing::Dispatch::new(RelayDropCounter {
        drops: Arc::clone(&drops),
    });
    let receiver_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RECEIVER_WORKERS)
        .enable_all()
        .on_thread_start(move || {
            // runtime の thread が終わるまで有効にする。
            std::mem::forget(tracing::dispatcher::set_default(&dispatch));
        })
        .build()
        .expect("receiver runtime");

    // 陽性対照: 受信側 runtime 上の同じ target・message を計数器が拾えることを先に確かめる。
    // これが無いと「drop 0 件」が計測漏れと区別できない。
    receiver_runtime
        .spawn(async {
            tracing::warn!(target: RELAY_ACTOR_TARGET, "{DROP_MESSAGE_PREFIX}: self-check");
        })
        .await
        .expect("self-check task");
    assert_eq!(
        drops.swap(0, Ordering::Relaxed),
        1,
        "drop counter self-check"
    );

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (received_tx, mut received_rx) = tokio::sync::mpsc::unbounded_channel();
    let (close_tx, close_rx) = tokio::sync::oneshot::channel::<()>();
    let receiver_relay_map = relay_map.clone();
    receiver_runtime.spawn(async move {
        let endpoint = relay_only_builder(receiver_relay_map)
            .alpns(vec![OVERLOAD_ALPN.to_vec()])
            .bind()
            .await
            .expect("receiver endpoint");
        endpoint.online().await;
        ready_tx.send(endpoint.id()).expect("report receiver id");
        let accept_connections = async {
            while let Some(incoming) = endpoint.accept().await {
                let received_tx = received_tx.clone();
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else {
                        return;
                    };
                    while let Ok(mut recv) = connection.accept_uni().await {
                        let received_tx = received_tx.clone();
                        tokio::spawn(async move {
                            if let Ok(bytes) =
                                recv.read_to_end(BURST_STREAM_BYTES.max(WARMUP_BYTES)).await
                            {
                                let _ = received_tx.send(bytes.len());
                            }
                        });
                    }
                });
            }
        };
        tokio::select! {
            _ = accept_connections => {}
            _ = close_rx => {}
        }
        endpoint.close().await;
    });

    let receiver_id = timeout(Duration::from_secs(30), ready_rx)
        .await
        .expect("receiver online timeout")
        .expect("receiver id");

    let mut endpoints = Vec::new();
    let mut connections = Vec::new();
    for _ in 0..senders {
        let sender = relay_only_builder(relay_map.clone())
            .bind()
            .await
            .expect("sender endpoint");
        let connection = timeout(
            Duration::from_secs(30),
            sender.connect(
                EndpointAddr::new(receiver_id).with_relay_url(relay_url.clone()),
                OVERLOAD_ALPN,
            ),
        )
        .await
        .expect("connect timeout")
        .expect("connect over relay");
        endpoints.push(sender);
        connections.push(connection);
    }

    // TR-1: consumer が動いている間は、cwnd を温める転送で drop が起きない。
    let warmups = connections.iter().map(|connection| async move {
        timeout(Duration::from_secs(120), send_uni(connection, WARMUP_BYTES))
            .await
            .expect("warmup timeout")
            .expect("warmup transfer");
    });
    futures_util::future::join_all(warmups).await;
    let warmup_bytes = timeout(
        Duration::from_secs(60),
        wait_for_received(&mut received_rx, senders),
    )
    .await
    .expect("warmup receive timeout");
    assert_eq!(warmup_bytes, senders * WARMUP_BYTES);
    let drops_while_running = drops.load(Ordering::Relaxed);

    // TR-2: 受信側 runtime の全 worker を止め、relay 受信 queue の consumer を停止させる。
    let (entered_tx, entered_rx) = std_mpsc::channel();
    let mut releases = Vec::new();
    for _ in 0..RECEIVER_WORKERS {
        let entered_tx = entered_tx.clone();
        let (release_tx, release_rx) = std_mpsc::channel::<()>();
        releases.push(release_tx);
        receiver_runtime.spawn(async move {
            entered_tx.send(()).expect("report stall");
            let _ = release_rx.recv();
        });
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("receiver worker stalls");
    }

    let mut transfers = Vec::new();
    for connection in &connections {
        for _ in 0..BURST_STREAMS {
            let connection = connection.clone();
            transfers.push(tokio::spawn(async move {
                send_uni(&connection, BURST_STREAM_BYTES).await
            }));
        }
    }
    let mut sent_before_release = 0;
    for connection in &connections {
        sent_before_release += wait_until_sender_stalls(connection).await;
    }

    drop(releases);

    for transfer in transfers {
        timeout(Duration::from_secs(300), transfer)
            .await
            .expect("burst transfer timeout")
            .expect("burst task")
            .expect("burst transfer");
    }
    let received_bytes = timeout(
        Duration::from_secs(60),
        wait_for_received(&mut received_rx, senders * BURST_STREAMS),
    )
    .await
    .expect("burst receive timeout");

    let mut outcome = OverloadOutcome {
        drops_while_running,
        drops_total: drops.load(Ordering::Relaxed),
        sent_before_release,
        sent_total: 0,
        lost_packets: 0,
        received_bytes,
    };
    for connection in &connections {
        let stats = connection.stats();
        outcome.sent_total += stats.udp_tx.datagrams;
        outcome.lost_packets += stats.lost_packets;
        connection.close(0u32.into(), b"done");
    }
    let _ = close_tx.send(());
    for endpoint in endpoints {
        endpoint.close().await;
    }
    receiver_runtime.shutdown_background();
    outcome
}

fn report(label: &str, outcome: &OverloadOutcome) {
    eprintln!(
        "relay overload[{label}]: drops_while_running={} drops_total={} sent_before_release={} \
         sent_total={} lost_packets={}",
        outcome.drops_while_running,
        outcome.drops_total,
        outcome.sent_before_release,
        outcome.sent_total,
        outcome.lost_packets,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_fallback_transfer_recovers_after_receiver_runtime_stall() {
    let outcome = run_receiver_stall(1).await;
    report("1 sender", &outcome);
    assert_eq!(outcome.drops_while_running, 0);
    assert_eq!(outcome.received_bytes, BURST_STREAMS * BURST_STREAM_BYTES);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_fallback_many_senders_recover_after_receiver_runtime_stall() {
    const SENDERS: usize = 4;
    let outcome = run_receiver_stall(SENDERS).await;
    report("4 senders", &outcome);
    assert_eq!(
        outcome.received_bytes,
        SENDERS * BURST_STREAMS * BURST_STREAM_BYTES
    );
}

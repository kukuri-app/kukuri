//! 計測用の UDP proxy を通す custom transport（#1984 AC-1、ADR 0070 §8.4）。
//!
//! 環境変数 `KUKURI_TEST_UDP_PROXY`（proxy の `IP:port`）があると、endpoint は IP の socket を持たず、この transport
//! だけで送受信する。同じ PC の process どうしでも直接の経路ができないので、全 packet が proxy
//! （`kukuri-sdk-net-proxy`）を通る。宛先は EndpointId で表し、どの EndpointId にも同じ形の address を返す lookup を置く。
//! packet は `[送り手の EndpointId][宛先の EndpointId][QUIC の datagram]`。宛先が 0 の packet は proxy への登録で、
//! まだ何も送っていない受け手へも proxy が渡せるように、起動時と 1 秒ごとに送る。

use std::{
    fmt, io,
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use anyhow::{Context as _, Result};
use iroh::{
    EndpointAddr, EndpointId, SecretKey, TransportAddr,
    address_lookup::{AddressLookup, Item as AddressLookupItem},
    endpoint::{
        Builder as EndpointBuilder,
        transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit},
    },
    endpoint_info::EndpointInfo,
};
use iroh_base::CustomAddr;
use tokio::{io::ReadBuf, net::UdpSocket};

/// proxy の address を渡す環境変数。
pub const UDP_PROXY_ENV: &str = "KUKURI_TEST_UDP_PROXY";
/// proxy の packet の先頭（送り手と宛先の EndpointId）の長さ。
pub const UDP_PROXY_HEADER_LEN: usize = 64;
const TRANSPORT_ID: u64 = 0x6b6b_7570;
/// socket の送受信の buffer（iroh の IP の socket と同じ大きさ。溢れて捨てた packet は proxy が数えられない）。
pub const UDP_PROXY_SOCKET_BUFFER: usize = 7 << 20;

pub(super) fn proxy_from_env() -> Result<Option<SocketAddr>> {
    std::env::var(UDP_PROXY_ENV)
        .ok()
        .map(|proxy| {
            proxy
                .parse::<SocketAddr>()
                .with_context(|| format!("failed to parse {UDP_PROXY_ENV}"))
        })
        .transpose()
}

/// IP の transport の代わりにこの transport を置く（自分の address は EndpointId から決まるので、鍵が渡されて
/// いなければここで作る）。
pub(super) fn with_proxy(
    builder: EndpointBuilder,
    proxy: SocketAddr,
    secret_key: Option<&SecretKey>,
) -> EndpointBuilder {
    let (builder, id) = match secret_key {
        Some(key) => (builder, key.public()),
        None => {
            let key = SecretKey::generate();
            let id = key.public();
            (builder.secret_key(key), id)
        }
    };
    builder
        .clear_ip_transports()
        .add_custom_transport(Arc::new(ProxyTransport { id, proxy }))
        .address_lookup(ProxyLookup)
}

fn address(id: &[u8]) -> CustomAddr {
    CustomAddr::from_parts(TRANSPORT_ID, id)
}

#[derive(Debug)]
struct ProxyTransport {
    id: EndpointId,
    proxy: SocketAddr,
}

impl CustomTransport for ProxyTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let socket = std::net::UdpSocket::bind(SocketAddr::new(self.proxy.ip(), 0))?;
        let options = socket2::SockRef::from(&socket);
        options.set_send_buffer_size(UDP_PROXY_SOCKET_BUFFER)?;
        options.set_recv_buffer_size(UDP_PROXY_SOCKET_BUFFER)?;
        socket.set_nonblocking(true)?;
        let socket = Arc::new(UdpSocket::from_std(socket)?);
        let mut register = self.id.as_bytes().to_vec();
        register.resize(UDP_PROXY_HEADER_LEN, 0);
        let (weak, proxy) = (Arc::downgrade(&socket), self.proxy);
        tokio::spawn(async move {
            while let Some(socket) = weak.upgrade() {
                let _ = socket.try_send_to(&register, proxy);
                drop(socket);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        Ok(Box::new(ProxyEndpoint {
            id: self.id,
            proxy: self.proxy,
            local: n0_watcher::Watchable::new(vec![address(self.id.as_bytes())]),
            scratch: vec![0; u16::MAX as usize + UDP_PROXY_HEADER_LEN],
            socket,
        }))
    }
}

struct ProxyEndpoint {
    id: EndpointId,
    proxy: SocketAddr,
    socket: Arc<UdpSocket>,
    local: n0_watcher::Watchable<Vec<CustomAddr>>,
    scratch: Vec<u8>,
}

impl fmt::Debug for ProxyEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProxyEndpoint")
    }
}

impl CustomEndpoint for ProxyEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.local.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(ProxySender {
            id: self.id,
            proxy: self.proxy,
            socket: Arc::clone(&self.socket),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        let mut filled = 0;
        while filled < bufs.len() {
            let mut read = ReadBuf::new(&mut self.scratch);
            let from = match self.socket.poll_recv_from(cx, &mut read) {
                Poll::Ready(Ok(from)) => from,
                Poll::Ready(Err(error)) if filled == 0 => return Poll::Ready(Err(error)),
                Poll::Ready(Err(_)) | Poll::Pending => break,
            };
            let packet = read.filled();
            let payload = packet.get(UDP_PROXY_HEADER_LEN..).unwrap_or_default();
            if from != self.proxy || payload.is_empty() || payload.len() > bufs[filled].len() {
                continue;
            }
            bufs[filled][..payload.len()].copy_from_slice(payload);
            metas[filled].len = payload.len();
            metas[filled].stride = payload.len();
            recv_infos[filled] =
                RecvInfo::new(address(&packet[..32]), Some(address(self.id.as_bytes())));
            filled += 1;
        }
        if filled == 0 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(filled))
        }
    }
}

struct ProxySender {
    id: EndpointId,
    proxy: SocketAddr,
    socket: Arc<UdpSocket>,
}

impl fmt::Debug for ProxySender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProxySender")
    }
}

impl CustomSender for ProxySender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        addr.id() == TRANSPORT_ID && addr.data().len() == 32
    }

    /// 送り待ちが満杯なら捨てる（QUIC は損失として再送する）。
    fn poll_send(
        &self,
        _cx: &mut Context,
        dst: &CustomAddr,
        _src: Option<&CustomAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        let segment = transmit
            .segment_size
            .unwrap_or(transmit.contents.len())
            .max(1);
        for datagram in transmit.contents.chunks(segment) {
            let mut packet = Vec::with_capacity(UDP_PROXY_HEADER_LEN + datagram.len());
            packet.extend_from_slice(self.id.as_bytes());
            packet.extend_from_slice(dst.data());
            packet.extend_from_slice(datagram);
            let _ = self.socket.try_send_to(&packet, self.proxy);
        }
        Poll::Ready(Ok(()))
    }
}

/// どの EndpointId にも、proxy の transport の address を返す。
#[derive(Debug)]
struct ProxyLookup;

impl AddressLookup for ProxyLookup {
    fn resolve(
        &self,
        endpoint_id: EndpointId,
    ) -> Option<n0_future::boxed::BoxStream<Result<AddressLookupItem, iroh::address_lookup::Error>>>
    {
        let info = EndpointInfo::from(EndpointAddr::from_parts(
            endpoint_id,
            [TransportAddr::Custom(address(endpoint_id.as_bytes()))],
        ));
        Some(Box::pin(futures_util::stream::once(async move {
            Ok(AddressLookupItem::new(info, "measurement-proxy", None))
        })))
    }
}

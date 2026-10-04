//! native の backend（str0m）。session ごとに ICE の UDP socket を 1 つ持つ task で `Rtc` を動かす。

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use bytes::Bytes;
use str0m::{
    Candidate, Event, IceConnectionState, Input, Output, Rtc,
    change::{SdpAnswer, SdpOffer, SdpPendingOffer},
    channel::{ChannelConfig, ChannelId, Reliability},
    net::{Protocol, Receive},
};
use tokio::net::UdpSocket;

use crate::{
    BUFFERED_HIGH_BYTES, BUFFERED_LOW_BYTES, CHANNEL_LABEL, CHANNEL_STREAM_ID, Control, Role,
    SessionId, Shared, Start,
};

/// STUN の送信先ごとに応答を待つ上限。
const STUN_TIMEOUT: Duration = Duration::from_millis(500);

/// 既定の経路の IP（パケットは送らない）。
pub(crate) fn default_route_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

const STUN_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];

/// `server` へ STUN の binding を 1 回問い合わせ、`socket` の外から見えるアドレスを返す（ADR 0057 §6）。
/// 認証の無い binding（RFC 8489）だけを使う。
async fn reflexive(socket: &UdpSocket, local: SocketAddr, server: &str) -> Option<SocketAddr> {
    let target = tokio::net::lookup_host(server)
        .await
        .ok()?
        .find(|addr| addr.is_ipv4() == local.is_ipv4())?;
    let mut request = [0u8; 20];
    request[1] = 0x01;
    request[4..8].copy_from_slice(&STUN_COOKIE);
    getrandom::fill(&mut request[8..]).ok()?;
    socket.send_to(&request, target).await.ok()?;
    let mut buf = [0u8; 512];
    loop {
        let (len, from) = socket.recv_from(&mut buf).await.ok()?;
        if from == target
            && let Some(addr) = xor_mapped_address(&buf[..len], &request[8..])
        {
            return Some(addr);
        }
    }
}

/// binding の成功応答から XOR-MAPPED-ADDRESS を読む。`id` は要求の transaction id。
fn xor_mapped_address(message: &[u8], id: &[u8]) -> Option<SocketAddr> {
    if message.get(..2)? != [0x01, 0x01]
        || message.get(4..8)? != STUN_COOKIE
        || message.get(8..20)? != id
    {
        return None;
    }
    let mut attrs = message.get(20..)?;
    while attrs.len() >= 4 {
        let kind = u16::from_be_bytes([attrs[0], attrs[1]]);
        let len = usize::from(u16::from_be_bytes([attrs[2], attrs[3]]));
        let value = attrs.get(4..4 + len)?;
        if kind == 0x0020 && len >= 8 {
            let port = u16::from_be_bytes([value[2], value[3]]) ^ 0x2112;
            let mask = [&STUN_COOKIE[..], id].concat();
            let ip = match (value[1], value.get(4..)?) {
                (1, [a, b, c, d]) => IpAddr::V4(Ipv4Addr::new(
                    a ^ mask[0],
                    b ^ mask[1],
                    c ^ mask[2],
                    d ^ mask[3],
                )),
                (2, bytes) if bytes.len() == 16 => {
                    let mut ip = [0u8; 16];
                    for (index, byte) in bytes.iter().enumerate() {
                        ip[index] = byte ^ mask[index];
                    }
                    IpAddr::V6(Ipv6Addr::from(ip))
                }
                _ => return None,
            };
            return Some(SocketAddr::new(ip, port));
        }
        attrs = attrs.get(4 + len.next_multiple_of(4)..)?;
    }
    None
}

pub(crate) fn spawn(start: Start) {
    tokio::spawn(async move {
        let session = start.session;
        let shared = Arc::clone(&start.shared);
        let reason = match run(start).await {
            Ok(reason) => reason,
            Err(error) => format!("{error:#}"),
        };
        shared.backend_finished(session, reason);
    });
}

fn channel_config() -> ChannelConfig {
    ChannelConfig {
        label: CHANNEL_LABEL.to_string(),
        ordered: false,
        // 寿命 1 ms の chunk は最初の送信では abandon されず、最初の送信から 1 ms 以上たって再送する時点で abandon される。
        // 再送 0 回は str0m の SCTP が最初の送信で abandon し、SACK と FORWARD-TSN が往復し続ける（ADR 0057 §2、#1575）。
        reliability: Reliability::MaxPacketLifetime { lifetime: 1 },
        negotiated: Some(CHANNEL_STREAM_ID),
        protocol: String::new(),
    }
}

struct Driver {
    rtc: Rtc,
    socket: UdpSocket,
    local: SocketAddr,
    session: SessionId,
    shared: Arc<Shared>,
    channel: Option<ChannelId>,
    open: bool,
    over_high: bool,
}

impl Driver {
    /// 1 回の変更の後に、出力を `Timeout` まで出し切る（str0m の single-mutation invariant）。
    async fn drain(&mut self) -> Result<Option<Instant>> {
        loop {
            match self.rtc.poll_output()? {
                Output::Timeout(deadline) => return Ok(Some(deadline)),
                Output::Transmit(transmit) => {
                    // 送れない UDP は捨てる（ICE と SCTP が再送する）。
                    let _ = self
                        .socket
                        .send_to(&transmit.contents, transmit.destination)
                        .await;
                }
                Output::Event(event) => {
                    if !self.handle_event(event) {
                        return Ok(None);
                    }
                }
            }
            if !self.rtc.is_alive() {
                return Ok(None);
            }
        }
    }

    /// event を処理し、session を続けるなら true を返す。
    fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::ChannelOpen(id, _) if Some(id) == self.channel => {
                if let Some(mut channel) = self.rtc.channel(id) {
                    channel.set_buffered_amount_low_threshold(BUFFERED_LOW_BYTES);
                }
                self.open = true;
                self.shared.opened(self.session);
            }
            Event::ChannelData(data) if Some(data.id) == self.channel => {
                self.shared.deliver(self.session, Bytes::from(data.data));
            }
            Event::ChannelBufferedAmountLow(id) if Some(id) == self.channel => {
                self.over_high = false;
            }
            Event::ChannelClose(id) if Some(id) == self.channel => return false,
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => return false,
            _ => {}
        }
        true
    }

    /// DataChannel を閉じる要求（SCTP の stream の reset）を相手へ送ってから終える。相手の session も閉じる。
    async fn close(&mut self) -> String {
        if let Some(id) = self.channel {
            self.rtc.direct_api().close_data_channel(id);
            // 自分の `ChannelClose` の event では止めずに、要求を送り切る。
            while let Ok(output) = self.rtc.poll_output() {
                match output {
                    Output::Timeout(_) => break,
                    Output::Transmit(transmit) => {
                        let _ = self
                            .socket
                            .send_to(&transmit.contents, transmit.destination)
                            .await;
                    }
                    Output::Event(_) => {}
                }
            }
        }
        "closed".to_string()
    }

    fn send(&mut self, datagram: &[u8]) -> Result<()> {
        let Some(id) = self.channel else {
            return Ok(());
        };
        if self.over_high || self.shared.test_drops_send() {
            self.shared.note_dropped_send();
            return Ok(());
        }
        let Some(mut channel) = self.rtc.channel(id) else {
            self.shared.note_dropped_send();
            return Ok(());
        };
        if !channel.write(true, datagram)? {
            self.shared.note_dropped_send();
        }
        let buffered = channel.buffered_amount();
        self.shared.note_buffered(buffered);
        if buffered >= BUFFERED_HIGH_BYTES {
            self.over_high = true;
        }
        Ok(())
    }
}

async fn run(start: Start) -> Result<String> {
    let Start {
        session,
        role,
        remote_offer,
        stun,
        reply,
        mut out,
        mut control,
        shared,
    } = start;
    let mut ip = shared.config.bind_ip;
    if ip.is_unspecified() {
        ip = default_route_ip().context("no default route for the webrtc ice socket")?;
    }
    let socket = UdpSocket::bind(SocketAddr::new(ip, 0))
        .await
        .context("bind webrtc ice socket")?;
    let local = socket.local_addr()?;
    let mut reflexive_addrs = Vec::new();
    for server in &stun {
        if let Ok(Some(addr)) =
            tokio::time::timeout(STUN_TIMEOUT, reflexive(&socket, local, server)).await
        {
            reflexive_addrs.push(addr);
        }
    }
    let mut driver = Driver {
        rtc: Rtc::builder().build(Instant::now()),
        socket,
        local,
        session,
        shared,
        channel: None,
        open: false,
        over_high: false,
    };
    let candidate = Candidate::host(local, "udp").context("host candidate")?;
    driver.rtc.add_local_candidate(candidate);
    for addr in reflexive_addrs {
        if let Ok(candidate) = Candidate::server_reflexive(addr, local, "udp") {
            driver.rtc.add_local_candidate(candidate);
        }
    }
    let mut deadline = driver.drain().await?;

    let mut pending: Option<SdpPendingOffer> = None;
    let local_sdp = match role {
        Role::Offerer => {
            let mut api = driver.rtc.sdp_api();
            driver.channel = Some(api.add_channel_with_config(channel_config()));
            let (offer, pending_offer) = api.apply().context("create webrtc offer")?;
            pending = Some(pending_offer);
            offer.to_sdp_string()
        }
        Role::Answerer => {
            let offer = remote_offer.context("answer without an offer")?;
            let offer = SdpOffer::from_sdp_string(&offer).context("parse webrtc offer")?;
            let answer = driver
                .rtc
                .sdp_api()
                .accept_offer(offer)
                .context("accept webrtc offer")?;
            deadline = driver.drain().await?;
            driver.channel = Some(
                driver
                    .rtc
                    .direct_api()
                    .create_data_channel(channel_config()),
            );
            answer.to_sdp_string()
        }
    };
    deadline = match deadline {
        Some(_) => driver.drain().await?,
        None => None,
    };
    if reply.send(Ok(local_sdp)).is_err() {
        return Ok("offer or answer was not taken".to_string());
    }

    let mut buf = vec![0u8; 2048];
    loop {
        let Some(next) = deadline else {
            return Ok("webrtc session ended".to_string());
        };
        let wait = next.saturating_duration_since(Instant::now());
        tokio::select! {
            received = driver.socket.recv_from(&mut buf) => {
                // 届かない宛先への送信の ICMP は、Windows では次の受信のエラーになる。session は続ける。
                let Ok((len, source)) = received else {
                    continue;
                };
                let Ok(contents) = buf[..len].try_into() else {
                    continue;
                };
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: driver.local,
                        contents,
                    },
                );
                if driver.rtc.accepts(&input) {
                    driver.rtc.handle_input(input)?;
                }
            }
            _ = tokio::time::sleep(wait.max(Duration::from_millis(1))) => {
                driver.rtc.handle_input(Input::Timeout(Instant::now()))?;
            }
            command = control.recv() => match command {
                Some(Control::Answer(answer, ack)) => {
                    let result = apply_answer(&mut driver.rtc, pending.take(), &answer);
                    let failed = result.is_err();
                    let _ = ack.send(result);
                    if failed {
                        bail!("webrtc answer was rejected");
                    }
                }
                None => return Ok(driver.close().await),
            },
            datagram = out.recv(), if driver.open => match datagram {
                Some(datagram) => {
                    // 待ちにある分を続けて送る。1 回の write ごとに出力を出し切る。
                    driver.send(&datagram)?;
                    let mut sent = 1;
                    while sent < crate::SEND_QUEUE_DATAGRAMS {
                        let Ok(datagram) = out.try_recv() else {
                            break;
                        };
                        if driver.drain().await?.is_none() {
                            return Ok("webrtc session ended".to_string());
                        }
                        driver.send(&datagram)?;
                        sent += 1;
                    }
                }
                None => return Ok(driver.close().await),
            },
        }
        deadline = driver.drain().await?;
    }
}

fn apply_answer(rtc: &mut Rtc, pending: Option<SdpPendingOffer>, answer: &str) -> Result<()> {
    let pending = pending.context("no pending webrtc offer")?;
    let answer = SdpAnswer::from_sdp_string(answer).context("parse webrtc answer")?;
    rtc.sdp_api()
        .accept_answer(pending, answer)
        .context("accept webrtc answer")?;
    Ok(())
}

//! native の backend（str0m）。session ごとに ICE の UDP socket を 1 つ持つ task で `Rtc` を動かす。

use std::{
    net::SocketAddr,
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
        reliability: Reliability::MaxRetransmits { retransmits: 0 },
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
        reply,
        mut out,
        mut control,
        shared,
    } = start;
    let socket = UdpSocket::bind(SocketAddr::new(shared.config.bind_ip, 0))
        .await
        .context("bind webrtc ice socket")?;
    let local = socket.local_addr()?;
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
                let (len, source) = received.context("receive on webrtc ice socket")?;
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
                None => return Ok("closed".to_string()),
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
                None => return Ok("closed".to_string()),
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

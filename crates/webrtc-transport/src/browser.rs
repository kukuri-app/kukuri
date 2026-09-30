//! browser の backend（web-sys の `RTCPeerConnection`）。
//!
//! JS のオブジェクトは session ごとの `spawn_local` の task と、その task が持つ callback の中だけで扱う（ADR 0056 §4）。
//! Rust の他の部分とは、`Send` の channel と `Shared` を通してだけやり取りする。

use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};

use anyhow::{Context as _, Result, anyhow, bail};
use bytes::Bytes;
use js_sys::{Reflect, Uint8Array};
use tokio::sync::mpsc;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelState,
    RtcDataChannelType, RtcIceGatheringState, RtcPeerConnection, RtcPeerConnectionState,
    RtcSdpType, RtcSessionDescriptionInit,
};

use crate::{
    BUFFERED_HIGH_BYTES, BUFFERED_LOW_BYTES, CHANNEL_LABEL, CHANNEL_STREAM_ID, Control, Role,
    SessionId, Shared, Start,
};

/// ICE の候補を集め終えるまで待つ上限。過ぎたら集まった候補で SDP を返す。
const GATHER_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) fn spawn(start: Start) {
    spawn_local(async move {
        let session = start.session;
        let shared = Arc::clone(&start.shared);
        let reason = match run(start).await {
            Ok(reason) => reason,
            Err(error) => format!("{error:#}"),
        };
        shared.backend_finished(session, reason);
    });
}

fn js_error(error: JsValue) -> anyhow::Error {
    anyhow!("{error:?}")
}

enum Signal {
    Gathered,
    Ended,
}

/// session の JS オブジェクトと callback。drop で callback を外して閉じる。
struct Peer {
    connection: RtcPeerConnection,
    channel: RtcDataChannel,
    _callbacks: Vec<Closure<dyn FnMut(JsValue)>>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.channel.set_onopen(None);
        self.channel.set_onmessage(None);
        self.channel.set_onclose(None);
        self.channel.set_onbufferedamountlow(None);
        self.connection.set_onicegatheringstatechange(None);
        self.connection.set_onconnectionstatechange(None);
        self.channel.close();
        self.connection.close();
    }
}

fn open_peer(
    session: SessionId,
    shared: &Arc<Shared>,
    over_high: &Rc<Cell<bool>>,
    signals: mpsc::Sender<Signal>,
) -> Result<Peer> {
    let connection =
        RtcPeerConnection::new_with_configuration(&RtcConfiguration::new()).map_err(js_error)?;
    let init = RtcDataChannelInit::new();
    init.set_negotiated(true);
    init.set_id(CHANNEL_STREAM_ID);
    init.set_ordered(false);
    init.set_max_retransmits(0);
    let channel = connection.create_data_channel_with_data_channel_dict(CHANNEL_LABEL, &init);
    channel.set_binary_type(RtcDataChannelType::Arraybuffer);
    channel.set_buffered_amount_low_threshold(BUFFERED_LOW_BYTES as u32);

    let mut callbacks = Vec::new();
    let on_open = {
        let shared = Arc::clone(shared);
        Closure::<dyn FnMut(JsValue)>::new(move |_| shared.opened(session))
    };
    channel.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    callbacks.push(on_open);

    let on_message = {
        let shared = Arc::clone(shared);
        Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            let Ok(event) = event.dyn_into::<MessageEvent>() else {
                return;
            };
            let data = Uint8Array::new(&event.data()).to_vec();
            shared.deliver(session, Bytes::from(data));
        })
    };
    channel.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    callbacks.push(on_message);

    let on_low = {
        let over_high = Rc::clone(over_high);
        Closure::<dyn FnMut(JsValue)>::new(move |_| over_high.set(false))
    };
    channel.set_onbufferedamountlow(Some(on_low.as_ref().unchecked_ref()));
    callbacks.push(on_low);

    let on_close = {
        let signals = signals.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |_| {
            let _ = signals.try_send(Signal::Ended);
        })
    };
    channel.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    callbacks.push(on_close);

    let on_gathering = {
        let signals = signals.clone();
        let connection = connection.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |_| {
            if connection.ice_gathering_state() == RtcIceGatheringState::Complete {
                let _ = signals.try_send(Signal::Gathered);
            }
        })
    };
    connection.set_onicegatheringstatechange(Some(on_gathering.as_ref().unchecked_ref()));
    callbacks.push(on_gathering);

    let on_state = {
        let connection = connection.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |_| {
            if matches!(
                connection.connection_state(),
                RtcPeerConnectionState::Failed | RtcPeerConnectionState::Closed
            ) {
                let _ = signals.try_send(Signal::Ended);
            }
        })
    };
    connection.set_onconnectionstatechange(Some(on_state.as_ref().unchecked_ref()));
    callbacks.push(on_state);

    Ok(Peer {
        connection,
        channel,
        _callbacks: callbacks,
    })
}

fn description(kind: RtcSdpType, sdp: &str) -> RtcSessionDescriptionInit {
    let init = RtcSessionDescriptionInit::new(kind);
    init.set_sdp(sdp);
    init
}

fn sdp_of(value: &JsValue) -> Result<String> {
    Reflect::get(value, &JsValue::from_str("sdp"))
        .map_err(js_error)?
        .as_string()
        .context("session description without sdp")
}

/// local description を設定し、ICE の候補を集め終えた SDP を返す（trickle しない）。
async fn local_description(
    peer: &Peer,
    kind: RtcSdpType,
    created: JsValue,
    signals: &mut mpsc::Receiver<Signal>,
) -> Result<String> {
    let sdp = sdp_of(&created)?;
    JsFuture::from(
        peer.connection
            .set_local_description(&description(kind, &sdp)),
    )
    .await
    .map_err(js_error)?;
    if peer.connection.ice_gathering_state() != RtcIceGatheringState::Complete {
        let _ = n0_future::time::timeout(GATHER_TIMEOUT, async {
            while let Some(signal) = signals.recv().await {
                if matches!(signal, Signal::Gathered) {
                    break;
                }
            }
        })
        .await;
    }
    peer.connection
        .local_description()
        .map(|description| description.sdp())
        .context("no local description")
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
    let over_high = Rc::new(Cell::new(false));
    let (signal_tx, mut signals) = mpsc::channel(8);
    let peer = open_peer(session, &shared, &over_high, signal_tx)?;

    let local_sdp = match role {
        Role::Offerer => {
            let offer = JsFuture::from(peer.connection.create_offer())
                .await
                .map_err(js_error)?;
            local_description(&peer, RtcSdpType::Offer, offer, &mut signals).await?
        }
        Role::Answerer => {
            let offer = remote_offer.context("answer without an offer")?;
            JsFuture::from(
                peer.connection
                    .set_remote_description(&description(RtcSdpType::Offer, &offer)),
            )
            .await
            .map_err(js_error)?;
            let answer = JsFuture::from(peer.connection.create_answer())
                .await
                .map_err(js_error)?;
            local_description(&peer, RtcSdpType::Answer, answer, &mut signals).await?
        }
    };
    if reply.send(Ok(local_sdp)).is_err() {
        return Ok("offer or answer was not taken".to_string());
    }

    loop {
        tokio::select! {
            command = control.recv() => match command {
                Some(Control::Answer(answer, ack)) => {
                    let result = JsFuture::from(
                        peer.connection
                            .set_remote_description(&description(RtcSdpType::Answer, &answer)),
                    )
                    .await
                    .map(|_| ())
                    .map_err(js_error);
                    let failed = result.is_err();
                    let _ = ack.send(result);
                    if failed {
                        bail!("webrtc answer was rejected");
                    }
                }
                None => return Ok("closed".to_string()),
            },
            datagram = out.recv() => match datagram {
                Some(datagram) => send(&peer.channel, &shared, &over_high, &datagram),
                None => return Ok("closed".to_string()),
            },
            signal = signals.recv() => match signal {
                Some(Signal::Ended) | None => return Ok("webrtc session ended".to_string()),
                Some(Signal::Gathered) => {}
            },
        }
    }
}

fn send(channel: &RtcDataChannel, shared: &Shared, over_high: &Cell<bool>, datagram: &[u8]) {
    if channel.ready_state() != RtcDataChannelState::Open || over_high.get() {
        shared.note_dropped_send();
        return;
    }
    if shared.test_drops_send() || channel.send_with_u8_array(datagram).is_err() {
        shared.note_dropped_send();
        return;
    }
    let buffered = channel.buffered_amount() as usize;
    shared.note_buffered(buffered);
    if buffered >= BUFFERED_HIGH_BYTES {
        over_high.set(true);
    }
}

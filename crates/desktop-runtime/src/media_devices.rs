//! 音声の機器(metaverse の空間音声と live の配信で共用。ADR 0067 §9)。`audio-devices`(Tauri の app だけ)は
//! cpal と echo cancellation、Web はブラウザの mic(#1879)、無い build(CLI・harness・試験)は mic を開けず、再生は
//! 試験が確かめる記録だけを残す。

use std::time::Duration;

use anyhow::Result;

/// Opus の設定(声。ADR 0067 §9)。
pub(crate) const OPUS_SAMPLE_RATE: u32 = 48_000;

#[cfg(feature = "audio-devices")]
mod real {
    use super::*;
    use moq_audio::{Layout, encode, playback};
    use moq_net::track;
    use n0_future::task::AbortOnDropHandle;
    use n0_future::time::timeout;

    /// 機器を開いて最初の音が届くまで待つ上限(状態の変わり目ごと)。
    const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

    const OPUS_BITRATE_BPS: u64 = 24_000;

    pub(crate) struct Playback {
        engine: playback::Engine,
    }

    impl Playback {
        pub(crate) async fn open() -> Result<Self> {
            let engine = playback::Engine::open(playback::Config::default()).await?;
            Ok(Self { engine })
        }

        pub(crate) fn output(&self) -> Result<Output> {
            let mut input = playback::Input::default();
            input.sample_rate = OPUS_SAMPLE_RATE;
            input.layout = Layout::Mono;
            Ok(Output {
                sink: self.engine.sink(input)?,
            })
        }

        fn canceller(&self) -> Option<moq_audio::aec::Control> {
            self.engine
                .canceller(moq_audio::aec::Config::default())
                .ok()
        }
    }

    pub(crate) struct Output {
        sink: playback::Sink,
    }

    #[derive(Clone)]
    pub(crate) struct Volume(playback::Control);

    impl Volume {
        pub(crate) fn set(&self, gain: f32) {
            self.0.set_volume(gain);
        }
    }

    impl Output {
        pub(crate) fn volume(&self) -> Volume {
            Volume(self.sink.control())
        }

        /// 再生前に保持している量。
        pub(crate) fn buffered(&self) -> Duration {
            self.sink.buffered()
        }

        /// 再生前の保持が上限(`max_buffered`)を超える分は書かずに捨てる(ADR 0067 §4)。
        pub(crate) fn write(&mut self, samples: &[f32], max_buffered: Duration) -> Result<()> {
            let duration =
                Duration::from_secs_f64(samples.len() as f64 / f64::from(OPUS_SAMPLE_RATE));
            if self.sink.buffered() + duration > max_buffered {
                return Ok(());
            }
            let bytes: Vec<u8> = samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect();
            let _ = self.sink.write(&bytes)?;
            Ok(())
        }
    }

    /// mic の取り込み・echo cancellation・Opus の圧縮と公開。落とすと止まる。
    pub(crate) struct Capture {
        publication: encode::Publication,
        /// moq-audio は購読者がいる間だけ機器を開く。mic が on の間はこの端末も購読し、機器を開いたままにする。
        _demand: track::Subscriber,
        _driver: AbortOnDropHandle<()>,
    }

    impl Capture {
        /// `catalog` の broadcast に `track_name` の Opus の track を置く。timestamp は catalog の clock(同じ broadcast の
        /// 映像と同じ時計)で付ける。
        pub(crate) async fn open(
            broadcast: &mut moq_net::broadcast::Producer,
            catalog: moq_mux::catalog::Producer,
            track_name: &str,
            playback: Option<&Playback>,
        ) -> Result<Self> {
            let mut options = encode::PublicationOptions::default();
            options.capture.aec = playback.and_then(Playback::canceller);
            options.clock = catalog.clock();
            options.encode.track = Some(track_name.to_string());
            options.encode.settings = encode::Settings::new(OPUS_SAMPLE_RATE, Layout::Mono);
            options.encode.settings.bitrate =
                Some(moq_net::bandwidth::Rate::from_bps(OPUS_BITRATE_BPS));
            let (publication, driver) =
                encode::Publication::new(broadcast.clone(), catalog, options)?;
            let driver = n0_future::task::spawn(async move {
                if let Err(error) = driver.run().await {
                    tracing::debug!(%error, "microphone publication ended");
                }
            });
            let demand = broadcast
                .consume()
                .track(track_name)?
                .subscribe(track::Subscription::default())
                .await?;
            let mut capture = Self {
                publication,
                _demand: demand,
                _driver: AbortOnDropHandle::new(driver),
            };
            capture.wait_live().await?;
            Ok(capture)
        }

        /// 機器だけを閉じる(track と catalog は残し、`start` で同じ track に戻る)。
        pub(crate) fn stop(&self) {
            self.publication.stop();
        }

        /// `stop` した機器を開き直す。開けなければ閉じたままにする。
        pub(crate) async fn start(&mut self) -> Result<()> {
            self.publication.start();
            let opened = self.wait_live().await;
            if opened.is_err() {
                self.publication.stop();
            }
            opened
        }

        /// 機器を開いて最初の音が届いたか(OS の許可を含む)を、開始を表示する前に確かめる。
        async fn wait_live(&mut self) -> Result<()> {
            let mut status = self.publication.state().status();
            while status != encode::Status::Live {
                if !matches!(
                    status,
                    encode::Status::Starting | encode::Status::Waiting | encode::Status::Stopped
                ) {
                    anyhow::bail!("the microphone could not be opened");
                }
                status = match timeout(OPEN_TIMEOUT, self.publication.changed()).await {
                    Ok(Some(state)) => state.status(),
                    _ => encode::Status::Failed,
                };
            }
            Ok(())
        }
    }
}

#[cfg(not(feature = "audio-devices"))]
mod null {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    /// 作った出力の記録(試験が、話者ごとの再生を確かめる)。
    #[derive(Default)]
    pub(crate) struct Playback {
        pub(crate) outputs: StdMutex<Vec<Output>>,
    }

    impl Playback {
        pub(crate) async fn open() -> Result<Self> {
            Ok(Self::default())
        }

        pub(crate) fn output(&self) -> Result<Output> {
            let output = Output::default();
            self.outputs
                .lock()
                .expect("outputs poisoned")
                .push(output.clone());
            Ok(output)
        }
    }

    /// 書いた sample の記録(試験が順と量を確かめる)と、最後の音量。
    #[derive(Clone, Default)]
    pub(crate) struct Output {
        pub(crate) written: Arc<StdMutex<Vec<f32>>>,
        volume: Volume,
    }

    #[derive(Clone, Default)]
    pub(crate) struct Volume(pub(crate) Arc<StdMutex<Option<f32>>>);

    impl Volume {
        pub(crate) fn set(&self, gain: f32) {
            *self.0.lock().expect("volume poisoned") = Some(gain);
        }
    }

    impl Output {
        pub(crate) fn volume(&self) -> Volume {
            self.volume.clone()
        }

        pub(crate) fn buffered(&self) -> Duration {
            Duration::ZERO
        }

        pub(crate) fn write(&mut self, samples: &[f32], _max_buffered: Duration) -> Result<()> {
            self.written
                .lock()
                .expect("written samples poisoned")
                .extend_from_slice(samples);
            Ok(())
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) struct Capture;

    #[cfg(not(target_family = "wasm"))]
    impl Capture {
        pub(crate) async fn open(
            _broadcast: &mut moq_net::broadcast::Producer,
            _catalog: moq_mux::catalog::Producer,
            _track_name: &str,
            _playback: Option<&Playback>,
        ) -> Result<Self> {
            anyhow::bail!("audio devices are not available in this build")
        }

        pub(crate) fn stop(&self) {}

        pub(crate) async fn start(&mut self) -> Result<()> {
            anyhow::bail!("audio devices are not available in this build")
        }
    }
}

/// Web の mic(#1879): ブラウザの `getUserMedia` の音を AudioWorklet(`live_audio_worklet.js`)で 20 ms ずつ受け、
/// moq-audio で Opus に圧縮して置く。echo cancellation・雑音抑制はブラウザの機能を使う。JS の object は main thread
/// だけで触る(runtime は Web では live の状態を `SendWrapper` に入れて持つ)。
#[cfg(target_family = "wasm")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use anyhow::{Context as _, anyhow};
    use moq_audio::{Frame, Layout, encode};
    use moq_net::Timestamp;
    use n0_future::time::timeout;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::*;

    /// mic の許可を求めてから最初の音が届くまで待つ上限(状態の変わり目ごと)。
    const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
    const OPUS_BITRATE_BPS: u64 = 24_000;

    fn js(error: JsValue) -> anyhow::Error {
        anyhow!("{error:?}")
    }

    /// mic の取り込みと Opus の圧縮・公開。落とすと止まる。
    pub(crate) struct Capture {
        context: web_sys::AudioContext,
        node: web_sys::AudioWorkletNode,
        producer: Rc<RefCell<encode::Producer>>,
        /// 開いている mic(`stop` で閉じる。track と catalog は残る)。
        source: RefCell<Option<(web_sys::MediaStream, web_sys::MediaStreamAudioSourceNode)>>,
        /// 受けた音の数と、届いたことの知らせ(開いた後の最初の音を待つ)。
        heard: Rc<Cell<u64>>,
        arrived: Rc<tokio::sync::Notify>,
        _on_audio: Closure<dyn FnMut(web_sys::MessageEvent)>,
    }

    impl Capture {
        /// `catalog` の broadcast に `track_name` の Opus の track を置き、mic を開く。timestamp は catalog の clock(同じ
        /// broadcast の映像と同じ時計)で付ける。
        pub(crate) async fn open(
            broadcast: &mut moq_net::broadcast::Producer,
            catalog: moq_mux::catalog::Producer,
            track_name: &str,
            _playback: Option<&super::Playback>,
        ) -> Result<Self> {
            let context = web_sys::AudioContext::new().map_err(js)?;
            let worklet = wasm_bindgen::link_to!(module = "/src/live_audio_worklet.js");
            JsFuture::from(
                context
                    .audio_worklet()
                    .map_err(js)?
                    .add_module(&worklet)
                    .map_err(js)?,
            )
            .await
            .map_err(js)?;
            let node =
                web_sys::AudioWorkletNode::new(&context, "kukuri-live-capture").map_err(js)?;
            // 出力は無音。destination へつながない node は、ブラウザによっては処理されない。
            node.connect_with_audio_node(&context.destination())
                .map_err(js)?;
            let mut options = encode::Options::default();
            options.track = Some(track_name.to_string());
            options.settings = encode::Settings::new(OPUS_SAMPLE_RATE, Layout::Mono);
            options.settings.bitrate = Some(moq_net::bandwidth::Rate::from_bps(OPUS_BITRATE_BPS));
            let clock = catalog.clock();
            let input = encode::Input::new(context.sample_rate() as u32, Layout::Mono);
            let producer = Rc::new(RefCell::new(encode::Producer::new(
                broadcast, catalog, input, &options,
            )?));
            let heard = Rc::new(Cell::new(0));
            let arrived = Rc::new(tokio::sync::Notify::new());
            let rate = f64::from(context.sample_rate());
            let on_audio = Closure::<dyn FnMut(web_sys::MessageEvent)>::new({
                let (producer, heard, arrived) = (producer.clone(), heard.clone(), arrived.clone());
                move |event: web_sys::MessageEvent| {
                    let samples = js_sys::Float32Array::new(&event.data()).to_vec();
                    // 受けた 20 ms の始まりの時刻(取り込みからの遅れは数 ms で、映像の時刻合わせの範囲に収まる)。
                    let span = (samples.len() as f64 / rate * 1_000_000.0) as u64;
                    let now = u64::try_from(clock.now().as_micros()).unwrap_or(u64::MAX);
                    let data: Vec<u8> = samples
                        .iter()
                        .flat_map(|sample| sample.to_le_bytes())
                        .collect();
                    if let Ok(timestamp) = Timestamp::from_micros(now.saturating_sub(span)) {
                        let _ = producer
                            .borrow_mut()
                            .write(&Frame::new(data.into(), timestamp));
                    }
                    heard.set(heard.get() + 1);
                    arrived.notify_waiters();
                }
            });
            node.port()
                .map_err(js)?
                .set_onmessage(Some(on_audio.as_ref().unchecked_ref()));
            let mut capture = Self {
                context,
                node,
                producer,
                source: RefCell::default(),
                heard,
                arrived,
                _on_audio: on_audio,
            };
            capture.start().await?;
            Ok(capture)
        }

        /// mic を閉じる(track と catalog は残し、`start` で同じ track に戻る)。
        pub(crate) fn stop(&self) {
            if let Some((stream, source)) = self.source.borrow_mut().take() {
                let _ = source.disconnect();
                stop_tracks(&stream);
            }
        }

        /// `stop` した mic を開き直す。許可を含め、最初の音が届くまで待つ。開けなければ閉じたままにする。
        pub(crate) async fn start(&mut self) -> Result<()> {
            let opened = self.open_source().await;
            if opened.is_err() {
                self.stop();
            }
            opened
        }

        async fn open_source(&self) -> Result<()> {
            let audio = js_sys::Object::new();
            for key in ["echoCancellation", "noiseSuppression", "autoGainControl"] {
                js_sys::Reflect::set(&audio, &key.into(), &JsValue::TRUE).map_err(js)?;
            }
            let constraints = web_sys::MediaStreamConstraints::new();
            constraints.set_audio(&audio);
            let devices = web_sys::window()
                .context("the window is not available")?
                .navigator()
                .media_devices()
                .map_err(js)?;
            let stream: web_sys::MediaStream = JsFuture::from(
                devices
                    .get_user_media_with_constraints(&constraints)
                    .map_err(js)?,
            )
            .await
            .map_err(js)?
            .unchecked_into();
            let source = match self.context.create_media_stream_source(&stream) {
                Ok(source) => source,
                Err(error) => {
                    stop_tracks(&stream);
                    return Err(js(error));
                }
            };
            *self.source.borrow_mut() = Some((stream, source.clone()));
            source.connect_with_audio_node(&self.node).map_err(js)?;
            // 止めていた間は時刻に空きを残す(映像の時計に合わせる)。
            self.producer.borrow_mut().reset_epoch();
            let before = self.heard.get();
            JsFuture::from(self.context.resume().map_err(js)?)
                .await
                .map_err(js)?;
            timeout(OPEN_TIMEOUT, async {
                while self.heard.get() == before {
                    self.arrived.notified().await;
                }
            })
            .await
            .context("the microphone sent no sound")
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            self.stop();
            let _ = self.context.close();
        }
    }

    fn stop_tracks(stream: &web_sys::MediaStream) {
        for track in stream.get_tracks() {
            track.unchecked_into::<web_sys::MediaStreamTrack>().stop();
        }
    }
}

#[cfg(all(not(feature = "audio-devices"), not(target_family = "wasm")))]
pub(super) use null::*;
// Web の再生(視聴)は #1879 AC-2。
#[cfg(target_family = "wasm")]
pub(super) use null::{Output, Playback};
#[cfg(feature = "audio-devices")]
pub(super) use real::*;
#[cfg(target_family = "wasm")]
pub(super) use web::Capture;

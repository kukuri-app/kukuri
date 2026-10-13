//! 機器。`audio-devices`(Tauri の app だけ)は cpal と echo cancellation、無い build(CLI・harness・試験)は
//! mic を開けず、再生は試験が確かめる記録だけを残す。

#[cfg(feature = "audio-devices")]
mod real {
    use super::super::*;
    use moq_audio::{Layout, encode, playback};

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
        _publication: encode::Publication,
        /// moq-audio は購読者がいる間だけ機器を開く。mic が on の間はこの端末も購読し、機器を開いたままにする。
        _demand: track::Subscriber,
        _driver: AbortOnDropHandle<()>,
    }

    impl Capture {
        pub(crate) async fn open(
            broadcast: &mut moq_net::broadcast::Producer,
            playback: Option<&Playback>,
        ) -> Result<Self> {
            let catalog =
                moq_mux::catalog::Producer::new(broadcast, moq_mux::catalog::Config::default())?;
            let mut options = encode::PublicationOptions::default();
            options.capture.aec = playback.and_then(Playback::canceller);
            options.encode.track = Some(METAVERSE_AUDIO_TRACK.to_string());
            options.encode.settings = encode::Settings::new(OPUS_SAMPLE_RATE, Layout::Mono);
            options.encode.settings.bitrate =
                Some(moq_net::bandwidth::Rate::from_bps(OPUS_BITRATE_BPS));
            let (mut publication, driver) =
                encode::Publication::new(broadcast.clone(), catalog, options)?;
            let driver = n0_future::task::spawn(async move {
                if let Err(error) = driver.run().await {
                    tracing::debug!(%error, "microphone publication ended");
                }
            });
            let demand = broadcast
                .consume()
                .track(METAVERSE_AUDIO_TRACK)?
                .subscribe(track::Subscription::default())
                .await?;
            // 機器を開いて最初の音が届いたか(OS の許可を含む)を、開始を表示する前に確かめる。
            let mut status = publication.state().status();
            while status != encode::Status::Live {
                if !matches!(status, encode::Status::Starting | encode::Status::Waiting) {
                    bail!("the microphone could not be opened");
                }
                status = match timeout(SUBSCRIBE_TIMEOUT, publication.changed()).await {
                    Ok(Some(state)) => state.status(),
                    _ => encode::Status::Failed,
                };
            }
            Ok(Self {
                _publication: publication,
                _demand: demand,
                _driver: AbortOnDropHandle::new(driver),
            })
        }
    }
}

#[cfg(not(feature = "audio-devices"))]
mod null {
    use super::super::*;

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

        pub(crate) fn write(&mut self, samples: &[f32], _max_buffered: Duration) -> Result<()> {
            self.written
                .lock()
                .expect("written samples poisoned")
                .extend_from_slice(samples);
            Ok(())
        }
    }

    pub(crate) struct Capture;

    impl Capture {
        pub(crate) async fn open(
            _broadcast: &mut moq_net::broadcast::Producer,
            _playback: Option<&Playback>,
        ) -> Result<Self> {
            bail!("audio devices are not available in this build")
        }
    }
}

#[cfg(not(feature = "audio-devices"))]
pub(super) use null::*;
#[cfg(feature = "audio-devices")]
pub(super) use real::*;

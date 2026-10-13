//! ADR 0067 の #1034 の contract。実 iroh の 2 node の間で、機器を使わずに配信の映像と音声を流して確かめる
//! (2 つの app は docs と blob を共有し、視聴者は配信者の session を読める)。

use super::*;
use kukuri_app_api::CreateLiveSessionInput;
use kukuri_blob_service::MemoryBlobService;
use kukuri_core::{ChannelRef, KukuriKeys, TopicId, generate_keys};
use kukuri_docs_sync::MemoryDocsSync;
use kukuri_iroh_node::MediaSubscription;
use kukuri_store::MemoryStore;
use kukuri_transport::{FakeNetwork, FakeTransport};

use crate::media_access::media_authorizer;
use crate::media_devices::OPUS_SAMPLE_RATE;

const TOPIC: &str = "kukuri:topic:live-media";

fn app(
    keys: KukuriKeys,
    network: &FakeNetwork,
    docs: &Arc<MemoryDocsSync>,
    blobs: &Arc<MemoryBlobService>,
) -> Arc<AppService> {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new(
        keys.public_key().as_str(),
        network.clone(),
    ));
    Arc::new(AppService::from_handles(
        kukuri_app_api::ServiceHandles::new(
            store.clone(),
            store,
            transport.clone(),
            transport,
            docs.clone(),
            blobs.clone(),
            keys,
        ),
    ))
}

struct Pair {
    broadcaster_node: Arc<IrohDocsNode>,
    viewer_node: Arc<IrohDocsNode>,
    broadcaster_app: Arc<AppService>,
    viewer_app: Arc<AppService>,
    owner: Pubkey,
    viewer_pubkey: Pubkey,
    broadcaster: Arc<LiveMedia>,
    viewer: Arc<LiveMedia>,
    session_id: String,
}

/// 配信者が公開 topic に media のある live session を作った状態。
async fn pair() -> Pair {
    let network = FakeNetwork::default();
    let docs = Arc::new(MemoryDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let (owner_keys, viewer_keys) = (generate_keys(), generate_keys());
    let (owner, viewer_pubkey) = (owner_keys.public_key(), viewer_keys.public_key());
    let broadcaster_app = app(owner_keys, &network, &docs, &blobs);
    let viewer_app = app(viewer_keys, &network, &docs, &blobs);
    let broadcaster_node = IrohDocsNode::memory().await.expect("broadcaster node");
    let viewer_node = IrohDocsNode::memory().await.expect("viewer node");
    viewer_node
        .discovery()
        .add_endpoint_info(broadcaster_node.endpoint().addr());
    let broadcaster = Arc::new(LiveMedia::default());
    broadcaster_node.install_media_authorizer(Some(media_authorizer(
        Weak::new(),
        Arc::downgrade(&broadcaster),
        Arc::downgrade(&broadcaster_app),
    )));
    let session_id = broadcaster_app
        .create_live_session_in_channel(
            TOPIC,
            ChannelRef::Public,
            CreateLiveSessionInput {
                title: "live".into(),
                description: String::new(),
            },
            Some(broadcaster_node.endpoint().id().to_string()),
        )
        .await
        .expect("live session");
    Pair {
        broadcaster_node,
        viewer_node,
        broadcaster_app,
        viewer_app,
        owner,
        viewer_pubkey,
        broadcaster,
        viewer: Arc::new(LiveMedia::default()),
        session_id,
    }
}

impl Pair {
    async fn start(&self) -> LiveBroadcastStatus {
        self.broadcaster
            .set_broadcast(&self.broadcaster_app, self.start_request())
            .await
            .expect("start")
    }

    fn start_request(&self) -> SetLiveBroadcastRequest {
        SetLiveBroadcastRequest {
            topic: TOPIC.into(),
            session_id: self.session_id.clone(),
            source: LiveVideoSource::Camera,
            microphone: true,
        }
    }

    /// 機器の無い build の mic の代わりに、配信の catalog と clock で音声を置く。
    async fn fake_microphone(&self) -> FakeMicrophone {
        let mut broadcasts = self.broadcaster.broadcasts.lock().await;
        let current = broadcasts.get_mut(&self.session_id).expect("broadcast");
        fake_microphone(&mut current.broadcast, current.catalog.clone())
    }

    /// 画面が圧縮した映像の代わり(Rust は映像を復号しない)。1 byte 目に番号を入れる。
    async fn push(&self, index: u8, keyframe: bool) {
        self.broadcaster
            .push_video(
                &self.session_id,
                keyframe,
                Duration::ZERO,
                vec![index, 0, 0, 0],
            )
            .await
            .expect("push");
    }

    /// 視聴をやり直す(前の視聴を止めてから始める)。
    async fn watch(&self) {
        self.viewer.stop_watching(&self.session_id).await;
        self.viewer
            .watch(
                &self.viewer_app,
                Some(self.viewer_node.clone()),
                LiveMediaRequest {
                    topic: TOPIC.into(),
                    session_id: self.session_id.clone(),
                },
            )
            .await
            .expect("watch");
    }

    /// 画面と同じく取り、届いた形を読み直す。
    async fn next(&self) -> Batch {
        Batch::decode(&self.viewer.next_video(&self.session_id).await)
    }

    /// 視聴の状態が変わるか `done` が満たされるまで取り続ける(上限 30 回)。
    async fn collect_until(&self, done: impl Fn(&[Chunk]) -> bool) -> (u8, Vec<Chunk>) {
        let mut chunks = Vec::new();
        for _ in 0..30 {
            let batch = self.next().await;
            chunks.extend(batch.chunks);
            if batch.state != LiveWatchState::Watching.code() || done(&chunks) {
                return (batch.state, chunks);
            }
        }
        (LiveWatchState::Watching.code(), chunks)
    }

    /// 視聴者の proof つきの認可要求で、配信者の端末へ直接購読する。
    async fn subscribe(&self) -> Result<MediaSubscription> {
        let context = SpatialContextV1::Topic {
            topic_id: TopicId::new(TOPIC),
        };
        let request = serde_json::to_vec(&MediaAccessRequestV1::Live(LiveMediaAccessRequestV1 {
            proof: self
                .viewer_app
                .build_dome_access_proof(context, self.owner.clone())
                .await
                .expect("proof"),
            topic_id: TOPIC.into(),
            session_id: self.session_id.clone(),
        }))
        .expect("request");
        self.viewer_node
            .media_subscribe(&self.broadcaster_node.endpoint().id().to_string(), &request)
            .await
    }

    async fn shutdown(self) {
        let _ = self.viewer_node.shutdown().await;
        let _ = self.broadcaster_node.shutdown().await;
    }
}

/// (timestamp μs, keyframe, 1 byte 目)
type Chunk = (u64, bool, u8);

#[derive(Debug)]
struct Batch {
    state: u8,
    audio_clock_us: u64,
    chunks: Vec<Chunk>,
}

impl Batch {
    fn decode(bytes: &[u8]) -> Self {
        let number = |at: usize, len: usize| {
            bytes[at..at + len]
                .iter()
                .rev()
                .fold(0_u64, |value, byte| value << 8 | u64::from(*byte))
        };
        let mut chunks = Vec::new();
        let mut at = 11;
        for _ in 0..number(9, 2) {
            let len = usize::try_from(number(at + 9, 4)).expect("length");
            chunks.push((number(at, 8), bytes[at + 8] == 1, bytes[at + 13]));
            at += 13 + len;
        }
        assert_eq!(at, bytes.len(), "the batch has no trailing bytes");
        Self {
            state: bytes[0],
            audio_clock_us: number(1, 8),
            chunks,
        }
    }
}

#[tokio::test]
async fn live_media_requires_explicit_start_and_permission() {
    let pair = pair().await;
    // 配信の開始の前は、視聴者に何も届かない(media の無い状態)。
    pair.watch().await;
    let (state, chunks) = pair.collect_until(|_| false).await;
    assert_eq!(state, LiveWatchState::NoMedia.code());
    assert!(chunks.is_empty());

    // 自分の session でなければ配信を始められない。
    assert!(
        pair.viewer
            .set_broadcast(&pair.viewer_app, pair.start_request())
            .await
            .is_err(),
        "only the owner broadcasts"
    );

    // 開始しても、画面の取り込みが始まる(最初の keyframe が届く)までは公開しない(取り込みの拒否なら送信 0)。
    // 機器の無い build は mic を開けず、映像だけを送る。
    let status = pair.start().await;
    assert!(!status.microphone);
    assert_eq!(status.limits, LiveVideoSource::Camera.limits());
    pair.push(1, false).await;
    pair.watch().await;
    let (state, chunks) = pair.collect_until(|_| false).await;
    assert_eq!(
        state,
        LiveWatchState::NoMedia.code(),
        "nothing is published before the first keyframe"
    );
    assert!(chunks.is_empty());

    // 最初の keyframe で公開し、視聴者へ届く。
    pair.push(2, true).await;
    pair.watch().await;
    pair.push(3, false).await;
    let (state, chunks) = pair.collect_until(|chunks| chunks.len() >= 2).await;
    assert_eq!(state, LiveWatchState::Watching.code());
    assert_eq!(
        chunks
            .iter()
            .map(|(_, keyframe, index)| (*keyframe, *index))
            .collect::<Vec<_>>(),
        [(true, 2), (false, 3)]
    );
    pair.shutdown().await;
}

#[tokio::test]
async fn live_media_late_joiner_starts_from_latest_group() {
    let pair = pair().await;
    pair.start().await;
    // 20 group(keyframe と差分 2 つ)を、取り込みの間隔(30 fps なら 33 ms)より短い 5 ms ごとに置く。どれも配信者の
    // cache に残っている間に参加する。
    let mut index = 0_u8;
    for _ in 0..20 {
        for keyframe in [true, false, false] {
            index += 1;
            pair.push(index, keyframe).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    // 途中から参加すると、過去の group の数に依らず今の group(最後の keyframe)から届き、前の group は送らない。
    pair.watch().await;
    pair.push(index + 1, false).await;
    let (state, chunks) = pair
        .collect_until(|chunks| chunks.iter().any(|(_, _, each)| *each == index + 1))
        .await;
    assert_eq!(state, LiveWatchState::Watching.code());
    assert_eq!(
        chunks
            .iter()
            .map(|(_, keyframe, each)| (*keyframe, *each))
            .collect::<Vec<_>>(),
        [
            (true, index - 2),
            (false, index - 1),
            (false, index),
            (false, index + 1)
        ]
    );
    pair.shutdown().await;
}

#[tokio::test]
async fn ended_live_session_rejects_media_subscribe() {
    let pair = pair().await;
    pair.start().await;
    pair.push(1, true).await;
    pair.watch().await;
    let (state, _) = pair.collect_until(|chunks| !chunks.is_empty()).await;
    assert_eq!(state, LiveWatchState::Watching.code());

    // 配信の停止: 見ている視聴者の購読は閉じ、新しい購読は拒否する(停止後の送信 0)。
    pair.broadcaster.stop_broadcast(&pair.session_id).await;
    let (state, _) = pair.collect_until(|_| false).await;
    assert_eq!(
        state,
        LiveWatchState::Ended.code(),
        "stopping the broadcast closes its viewers"
    );
    let denied = pair.subscribe().await.err().expect("stopped");
    assert_eq!(denial_code(&denied), Some(DENIED_NOT_LIVE));
    pair.start().await;
    pair.push(2, true).await;
    pair.watch().await;
    let (state, _) = pair.collect_until(|chunks| !chunks.is_empty()).await;
    assert_eq!(state, LiveWatchState::Watching.code());

    // 資格の失効: owner が視聴者を block すると、5 秒ごとの評価し直しで閉じ、新しい購読も拒否する。
    pair.broadcaster_app
        .block_author(pair.viewer_pubkey.as_str())
        .await
        .expect("block");
    let (state, _) = pair.collect_until(|_| false).await;
    assert_eq!(
        state,
        LiveWatchState::Ended.code(),
        "the broadcaster closes the revoked viewer"
    );
    let denied = pair.subscribe().await.err().expect("blocked");
    assert_eq!(denial_code(&denied), Some(DENIED_ACCESS));
    pair.broadcaster_app
        .unblock_author(pair.viewer_pubkey.as_str())
        .await
        .expect("unblock");

    // session の終了: 配信を止める前でも、見ている視聴者は評価し直しで閉じ、新しい購読は拒否する。
    pair.watch().await;
    pair.push(3, true).await;
    let (state, _) = pair.collect_until(|chunks| !chunks.is_empty()).await;
    assert_eq!(state, LiveWatchState::Watching.code());
    pair.broadcaster_app
        .end_live_session(TOPIC, &pair.session_id)
        .await
        .expect("end");
    let (state, _) = pair.collect_until(|_| false).await;
    assert_eq!(
        state,
        LiveWatchState::Ended.code(),
        "the broadcaster closes viewers of the ended session"
    );
    let denied = pair.subscribe().await.err().expect("ended");
    assert_eq!(denial_code(&denied), Some(DENIED_NOT_LIVE));
    // 視聴者の端末は終わった session へ購読しない。
    pair.watch().await;
    let (state, chunks) = pair.collect_until(|_| false).await;
    assert_eq!(state, LiveWatchState::NoMedia.code());
    assert!(chunks.is_empty());
    pair.shutdown().await;
}

#[tokio::test]
async fn live_media_direct_viewers_are_bounded() {
    let pair = pair().await;
    pair.start().await;
    pair.push(1, true).await;
    let mut viewers = Vec::new();
    for _ in 0..MAX_DIRECT_VIEWERS {
        viewers.push(pair.subscribe().await.expect("within the limit"));
    }
    // 5 人目は満員(視聴者どうしの転送は #1034 AC-2b)。
    let full = pair.subscribe().await.err().expect("full");
    assert_eq!(denial_code(&full), Some(DENIED_FULL));
    // 1 人抜ければ次の視聴者が入れる。
    viewers.pop();
    let mut admitted = None;
    for _ in 0..50 {
        if let Ok(viewer) = pair.subscribe().await {
            admitted = Some(viewer);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(admitted.is_some(), "a freed slot admits the next viewer");
    drop(viewers);
    pair.shutdown().await;
}

#[tokio::test]
async fn live_media_presents_video_against_audio_clock() {
    let pair = pair().await;
    pair.start().await;
    pair.push(1, true).await;
    pair.watch().await;
    let mut index = 1_u8;
    // 映像を 1 frame 置いて、画面と同じく取る。
    let mut next = async || {
        index += 1;
        pair.push(index, false).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let batch = pair.next().await;
        assert_eq!(batch.state, LiveWatchState::Watching.code());
        batch
    };
    let (_, chunks) = pair.collect_until(|chunks| !chunks.is_empty()).await;
    assert!(!chunks.is_empty());
    assert_eq!(
        next().await.audio_clock_us,
        0,
        "no audio clock before the microphone"
    );

    // 視聴の途中で mic を開いても、視聴者は音声を足し、画面へ渡す映像に同じ clock の音声の再生時刻が付く。
    let voice = pair.fake_microphone().await;
    let mut heard = Vec::new();
    for _ in 0..100 {
        let batch = next().await;
        if batch.audio_clock_us > 0 {
            heard.push(batch);
        }
        if heard.len() == 10 {
            break;
        }
    }
    let (video, audio) = latest(&heard);
    assert!(
        video.abs_diff(audio) < 1_000_000,
        "the video {video} and the audio {audio} share the broadcast clock"
    );

    // mic を止めても(track は残り、音声だけが来なくなる)、音声の再生時刻は経過時間で進み、映像は止まらない。
    voice.speaking.store(false, Ordering::Release);
    let mut silent = Vec::new();
    for _ in 0..25 {
        silent.push(next().await);
    }
    let (video, after) = latest(&silent);
    assert!(
        after > audio + 300_000,
        "the clock keeps moving without audio: {audio} -> {after}"
    );
    assert!(
        video.abs_diff(after) < 1_000_000,
        "the video {video} and the clock {after} stay together"
    );
    pair.shutdown().await;
}

/// 最後に届いた映像の timestamp と、最後の音声の再生時刻。
fn latest(batches: &[Batch]) -> (u64, u64) {
    let (video, _, _) = *batches
        .iter()
        .rev()
        .find_map(|batch| batch.chunks.last())
        .expect("video");
    (video, batches.last().expect("batch").audio_clock_us)
}

/// mic の代わり: `speaking` の間、20 ms ごとに Opus を書く音声の track。止めても track は残る(mic の `stop` と同じ)。
struct FakeMicrophone {
    speaking: Arc<std::sync::atomic::AtomicBool>,
    _task: AbortOnDropHandle<()>,
}

fn fake_microphone(
    broadcast: &mut moq_net::broadcast::Producer,
    catalog: moq_mux::catalog::Producer,
) -> FakeMicrophone {
    let clock = catalog.clock();
    let mut options = moq_audio::encode::Options::default();
    options.track = Some(LIVE_AUDIO_TRACK.into());
    options.settings = moq_audio::encode::Settings::new(OPUS_SAMPLE_RATE, moq_audio::Layout::Mono);
    let mut producer = moq_audio::encode::Producer::new(
        broadcast,
        catalog,
        moq_audio::encode::Input::new(OPUS_SAMPLE_RATE, moq_audio::Layout::Mono),
        &options,
    )
    .expect("encoder");
    let speaking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let writing = Arc::clone(&speaking);
    let task = n0_future::task::spawn(async move {
        let mut tick = n0_future::time::interval(Duration::from_millis(20));
        loop {
            tick.tick().await;
            if !writing.load(Ordering::Acquire) {
                continue;
            }
            let samples: Vec<u8> = (0..960)
                .map(|index| ((index as f32) * 0.05).sin() * 0.25)
                .flat_map(f32::to_le_bytes)
                .collect();
            let _ = producer.write(&moq_audio::Frame::new(samples.into(), clock.now()));
        }
    });
    FakeMicrophone {
        speaking,
        _task: AbortOnDropHandle::new(task),
    }
}

#[test]
fn live_media_bounds_frames_waiting_for_the_screen() {
    let screen = ScreenQueue::default();
    let chunk = |index: u8, keyframe: bool| ScreenChunk {
        timestamp_us: u64::from(index),
        keyframe,
        data: vec![index].into(),
    };
    let queued = || {
        let state = screen.state.lock().expect("state");
        (
            state
                .chunks
                .iter()
                .map(|chunk| chunk.timestamp_us)
                .collect::<Vec<_>>(),
            state.waiting_keyframe,
        )
    };
    // 画面が取りに来ない間は 1 秒分(30)まで持ち、超えたら次の keyframe まで捨てる。
    screen.push(chunk(0, true));
    for index in 1..40 {
        screen.push(chunk(index, false));
    }
    let (chunks, waiting) = queued();
    assert_eq!(chunks, (0..30).collect::<Vec<_>>());
    assert!(waiting);
    screen.push(chunk(41, false));
    screen.push(chunk(42, true));
    screen.push(chunk(43, false));
    assert_eq!(
        queued(),
        (vec![42, 43], false),
        "the stale backlog is dropped at the next keyframe"
    );
    // 受ける側の上限は、catalog の source の frame 数と bitrate のそれぞれ 1.25 倍(frame 数は切り上げ)。
    for source in [LiveVideoSource::Camera, LiveVideoSource::Screen] {
        let limits = source.limits();
        let budget = video_budget(&video_config(limits));
        assert_eq!(
            (budget.max_frames(), budget.max_bytes()),
            (
                (u64::from(limits.framerate) * 5).div_ceil(4),
                u64::from(limits.bitrate) * 5 / 4 / 8
            )
        );
    }
}

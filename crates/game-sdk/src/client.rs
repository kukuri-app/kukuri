//! Game SDK の handle（ADR 0070 §3）。要求を `kukuri-cli` の dispatcher で実行し、応答と event を有限の待ち行列に置く。
//! 呼出側（Unity の main thread）からの send・cancel・poll は network を待たない。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures_util::FutureExt;
use kukuri_cli::{
    dispatcher::{DispatchReply, Dispatcher, decode_request},
    protocol::{
        CommandEffect, MAX_FRAME_BYTES, ProtocolError, RequestEnvelope, ResponseEnvelope,
        SecretInput, error_code,
    },
    session::ClientSession,
};
use kukuri_desktop_runtime::{
    ClientHost, ClientProfile, ClientProfileKind, DesktopRuntime, GameDelivery, GameSendRefused,
    ProfileLease,
};
use serde::Deserialize;
use tokio::sync::{Notify, Semaphore, oneshot, watch};

/// 未完了の要求と、poll されていない応答の合計の上限。
pub const MAX_PENDING: usize = 256;
/// 知らせの event の上限。溢れたら古いものから捨てる。
pub const MAX_EVENTS: usize = 1024;
/// 1 回の poll で返す件数の上限。
pub const MAX_POLL_ITEMS: usize = 256;
/// 同時に dispatcher で実行する要求の数。残りは実行前の待ちで、取り消せる。
const MAX_RUNNING: usize = 16;
/// close で実行中の書き込みを待つ時間。
const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// 実行前に取り消した要求の error code（`kukuri-cli` の `error_code` への SDK の追加。ADR 0070 §3）。
pub const CANCELLED: &str = "cancelled";

/// ABI の関数が同期的に返す状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Status {
    Ok = 0,
    InvalidArgument = 1,
    Backpressure = 2,
    Closed = 3,
    Faulted = 4,
    AbiMismatch = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ItemKind {
    /// `ResponseEnvelope` の JSON。
    Response = 1,
    /// 直前の応答の秘密の出力（JSON ではない bytes）。
    Secret = 2,
    /// `RuntimeEvent` と SDK の event（`events_lost`・`game_session_packet`）の JSON。
    Event = 3,
    /// 直前の event（`game_session_packet`）の bytes。
    Bytes = 4,
}

/// `kukuri_sdk_send_binary` の kind（ADR 0070 §3）。3 は live の映像（`[幅 u16][高さ u16][下の行からの RGBA]`。feature
/// `vp8`）、4 はゲームの音（`[sample rate u32][channel 数 u16][interleave した f32]`。#1982）。数値は little endian。
pub const BINARY_DATAGRAM: u32 = 1;
pub const BINARY_STREAM: u32 = 2;
pub const BINARY_LIVE_VIDEO: u32 = 3;
pub const BINARY_LIVE_AUDIO: u32 = 4;
/// 1 回に渡すゲームの音の上限（Unity の 1 回の audio の callback は数十 ms で、これを大きく下回る）。
const MAX_LIVE_AUDIO_BYTES: usize = 64 << 10;
/// poll で packet の event に数える JSON の大きさ（session ID と配送の名前を含む上限の見積もり）。
const PACKET_EVENT_BYTES: usize = 128;

#[derive(Debug)]
pub struct Item {
    pub kind: ItemKind,
    pub data: Vec<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkConfig {
    pub profile: String,
    pub profile_dir: PathBuf,
    /// 本番の既定は FFI（process の中で動かす）。Sidecar は開発用で、動いている Sidecar へ接続する（ADR 0070 §1）。
    #[serde(default)]
    pub mode: Mode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Ffi,
    Sidecar,
}

/// 起動の結果。
pub struct Started {
    pub dispatcher: Dispatcher,
    pub session: Option<Arc<ClientSession>>,
}

type StartResult = Option<Result<Arc<Started>, ProtocolError>>;

struct Pending {
    /// 実行前だけ Some。実行を始めたら取り消せない。
    cancel: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
struct Queues {
    pending: HashMap<String, Pending>,
    /// 未完了の要求と poll されていない応答の合計（応答の置き場の予約）。
    reserved: usize,
    /// 応答（request_id、JSON、秘密の出力）。
    responses: VecDeque<(String, Vec<u8>, Option<Vec<u8>>)>,
    /// 届け先が切断した要求。終わっても応答を置かない（Sidecar の接続の切替）。
    abandoned: HashSet<String>,
    events: VecDeque<Vec<u8>>,
    lost: u64,
    closed: bool,
}

struct Shared {
    queues: Mutex<Queues>,
    faulted: AtomicBool,
    /// 要求の完了ごとに知らせ、event の転送が host の出入りを確かめる。
    host_changed: Notify,
    /// pending が減ったことの知らせ（close の待ち）。
    settled: Notify,
}

impl Shared {
    fn queues(&self) -> std::sync::MutexGuard<'_, Queues> {
        self.queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn complete(&self, request_id: &str, response: Vec<u8>, secret: Option<Vec<u8>>) {
        let mut queues = self.queues();
        // close が成否不明として先に返した要求は、遅れて終わっても二重に返さない。
        if queues.pending.remove(request_id).is_some() {
            if queues.abandoned.remove(request_id) {
                queues.reserved -= 1;
            } else {
                queues
                    .responses
                    .push_back((request_id.to_owned(), response, secret));
            }
        }
        drop(queues);
        self.settled.notify_waiters();
        self.host_changed.notify_one();
    }

    fn push_event(&self, event: Vec<u8>) {
        let mut queues = self.queues();
        if queues.events.len() >= MAX_EVENTS {
            queues.events.pop_front();
            queues.lost += 1;
        }
        queues.events.push_back(event);
    }
}

pub struct SdkClient {
    runtime: Option<tokio::runtime::Runtime>,
    shared: Arc<Shared>,
    started: watch::Receiver<StartResult>,
    shutdown: watch::Sender<bool>,
    running: Arc<Semaphore>,
    profile: String,
    /// profile の所有。close で session を止めた後に手放す（同じ profile を free の前に開き直せる）。
    lease: Arc<Mutex<Option<ProfileLease>>>,
    /// アセットの発行（`publish_game_asset`）を受け付けるか。開発用の Sidecar の process だけが許す（ADR 0070 §5.3）。
    publish: Arc<AtomicBool>,
    #[cfg(feature = "vp8")]
    live: Arc<crate::live::Live>,
}

impl SdkClient {
    /// profile を所有して runtime を背景で起動する。完了は `startup_status_changed` の event で知らせる。
    pub fn open(config: SdkConfig) -> std::io::Result<Self> {
        let SdkConfig {
            profile,
            profile_dir,
            ..
        } = config;
        let name = profile.clone();
        let lease = Arc::new(Mutex::new(None));
        let slot = lease.clone();
        let mut client = Self::open_with(profile, async move {
            let acquired = ProfileLease::acquire(ClientProfile {
                name,
                app_data_dir: profile_dir.clone(),
                kind: ClientProfileKind::GameSdk,
            })
            .map_err(|error| ProtocolError::new(error.code(), error.message))?;
            *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(acquired);
            let session = ClientSession::start(profile_dir).await?;
            Ok(Started {
                dispatcher: Dispatcher::for_session(session.clone()),
                session: Some(session),
            })
        })?;
        client.lease = lease;
        Ok(client)
    }

    /// 起動の処理を差し替えて開く（試験で dispatcher を注入する）。
    pub fn open_with(
        profile: String,
        start: impl Future<Output = Result<Started, ProtocolError>> + Send + 'static,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("kukuri-sdk")
            .enable_all()
            .build()?;
        let shared = Arc::new(Shared {
            queues: Mutex::new(Queues::default()),
            faulted: AtomicBool::new(false),
            host_changed: Notify::new(),
            settled: Notify::new(),
        });
        let (started_tx, started) = watch::channel(None);
        let (shutdown, shutdown_rx) = watch::channel(false);
        runtime.spawn({
            let shared = shared.clone();
            async move {
                let result = start.await.map(Arc::new);
                let session = result
                    .as_ref()
                    .ok()
                    .and_then(|started| started.session.clone());
                started_tx.send_replace(Some(result));
                pump_events(shared, session, shutdown_rx).await;
            }
        });
        #[cfg(feature = "vp8")]
        let live = crate::live::Live::new(runtime.handle().clone(), {
            let shared = shared.clone();
            Arc::new(move |event| shared.push_event(event))
        });
        Ok(Self {
            runtime: Some(runtime),
            shared,
            started,
            shutdown,
            running: Arc::new(Semaphore::new(MAX_RUNNING)),
            profile,
            lease: Arc::default(),
            publish: Arc::default(),
            #[cfg(feature = "vp8")]
            live,
        })
    }

    /// アセットの発行を許す（開発用の Sidecar の process が開いた直後に呼ぶ）。
    pub fn allow_publish(&self) {
        self.publish.store(true, Ordering::Release);
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// command が書き込みかどうか。起動の前で分からなければ None。
    pub fn is_write(&self, command: &str) -> Option<bool> {
        let started = self.started.borrow();
        let started = started.as_ref()?.as_ref().ok()?;
        Some(matches!(
            started
                .dispatcher
                .registry()
                .get(command)
                .map(|registration| registration.metadata.effect),
            Some(CommandEffect::Write | CommandEffect::Destructive)
        ))
    }

    /// 届け先が切断した要求の応答を捨てる。実行中のものは終わっても応答を置かない（取り消しはしない）。
    pub fn abandon(&self, request_ids: &[String]) {
        let mut queues = self.shared.queues();
        for request_id in request_ids {
            if queues.pending.contains_key(request_id) {
                queues.abandoned.insert(request_id.clone());
            }
        }
        let before = queues.responses.len();
        queues
            .responses
            .retain(|(request_id, _, _)| !request_ids.contains(request_id));
        let dropped = before - queues.responses.len();
        queues.reserved -= dropped;
    }

    /// 溜まった event を捨てる。新しく接続した受け手は状態を読み直す。
    pub fn clear_events(&self) {
        let mut queues = self.shared.queues();
        queues.events.clear();
        queues.lost = 0;
    }

    fn check(&self) -> Result<(), Status> {
        if self.shared.faulted.load(Ordering::Acquire) {
            Err(Status::Faulted)
        } else {
            Ok(())
        }
    }

    pub fn send(&self, request: &[u8], secret: Option<&[u8]>) -> Status {
        if let Err(status) = self.check() {
            return status;
        }
        let Some(runtime) = &self.runtime else {
            return Status::Closed;
        };
        if request.len() > MAX_FRAME_BYTES {
            return Status::InvalidArgument;
        }
        let Ok(request) = decode_request(request) else {
            return Status::InvalidArgument;
        };
        if request.request_id.trim().is_empty() {
            return Status::InvalidArgument;
        }
        let (cancel, cancelled) = oneshot::channel();
        {
            let mut queues = self.shared.queues();
            if queues.closed {
                return Status::Closed;
            }
            if queues.pending.contains_key(&request.request_id) {
                return Status::InvalidArgument;
            }
            if queues.reserved >= MAX_PENDING {
                return Status::Backpressure;
            }
            queues.reserved += 1;
            queues.pending.insert(
                request.request_id.clone(),
                Pending {
                    cancel: Some(cancel),
                },
            );
        }
        let secret = secret.map(|bytes| SecretInput::new(bytes.to_vec()));
        let shared = self.shared.clone();
        let task = RequestTask {
            shared: shared.clone(),
            started: self.started.clone(),
            shutdown: self.shutdown.subscribe(),
            running: self.running.clone(),
            profile: self.profile.clone(),
            publish: self.publish.load(Ordering::Acquire),
            #[cfg(feature = "vp8")]
            live: self.live.clone(),
        };
        runtime.spawn(async move {
            let request_id = request.request_id.clone();
            let failure_request = request.clone();
            let (response, secret) = match AssertUnwindSafe(task.run(request, secret, cancelled))
                .catch_unwind()
                .await
            {
                Ok(reply) => reply,
                Err(_) => {
                    // 実行中の panic。handle を止め、以後の関数は Faulted を返す（ADR 0070 §3）。
                    shared.faulted.store(true, Ordering::Release);
                    (
                        failure(
                            &failure_request,
                            ProtocolError::new(error_code::INTERNAL_ERROR, "request task panicked"),
                        ),
                        None,
                    )
                }
            };
            shared.complete(&request_id, response, secret);
        });
        Status::Ok
    }

    /// 実行前の要求を取り消す。実行中の要求は取り消せず、実際の結果を返す。
    pub fn cancel(&self, request_id: &str) -> Status {
        if let Err(status) = self.check() {
            return status;
        }
        let mut queues = self.shared.queues();
        match queues.pending.get_mut(request_id) {
            Some(pending) => {
                if let Some(cancel) = pending.cancel.take() {
                    let _ = cancel.send(());
                }
                Status::Ok
            }
            None => Status::InvalidArgument,
        }
    }

    /// 開いている session の packet を送り待ちに置いて即座に戻る（ADR 0070 §3）。
    pub fn send_binary(&self, kind: u32, target: &str, bytes: &[u8]) -> Status {
        if let Err(status) = self.check() {
            return status;
        }
        let delivery = match kind {
            BINARY_DATAGRAM => Some(GameDelivery::Datagram),
            BINARY_STREAM => Some(GameDelivery::Stream),
            BINARY_LIVE_AUDIO => None,
            #[cfg(feature = "vp8")]
            BINARY_LIVE_VIDEO => None,
            _ => return Status::InvalidArgument,
        };
        if self.runtime.is_none() || self.shared.queues().closed {
            return Status::Closed;
        }
        let Some(runtime) = self.host_runtime() else {
            return Status::Closed;
        };
        match (kind, delivery) {
            (_, Some(delivery)) => match runtime.send_game_packet(target, delivery, bytes) {
                Ok(()) => Status::Ok,
                Err(GameSendRefused::TooLarge) => Status::InvalidArgument,
                Err(GameSendRefused::Backpressure) => Status::Backpressure,
                Err(GameSendRefused::Closed) => Status::Closed,
            },
            (BINARY_LIVE_AUDIO, None) => push_live_audio(&runtime, target, bytes),
            #[cfg(feature = "vp8")]
            _ => match self.live.push_video(runtime, target, bytes) {
                Ok(()) => Status::Ok,
                Err(crate::live::VideoRefused::Invalid) => Status::InvalidArgument,
                Err(crate::live::VideoRefused::NotBroadcasting) => Status::Closed,
            },
            #[cfg(not(feature = "vp8"))]
            _ => Status::InvalidArgument,
        }
    }

    fn host_runtime(&self) -> Option<Arc<DesktopRuntime>> {
        let started = self.started.borrow();
        let session = started.as_ref()?.as_ref().ok()?.session.clone()?;
        session.host().map(|host| host.runtime())
    }

    /// 届いている応答・秘密の出力・event・packet を上限まで取り出す。応答とその秘密の出力、packet の event とその
    /// bytes は分けない。`accept_packets` が false なら（未解放の batch が上限に達した）packet を取り出さない。
    pub fn poll(
        &self,
        max_items: usize,
        max_bytes: usize,
        accept_packets: bool,
    ) -> Result<Vec<Item>, Status> {
        self.check()?;
        let max_items = max_items.clamp(1, MAX_POLL_ITEMS);
        let mut queues = self.shared.queues();
        let mut items = Vec::new();
        let mut bytes = 0;
        let fits = |items: &Vec<Item>, bytes: usize, size: usize, count: usize| {
            items.len() + count <= max_items && (items.is_empty() || bytes + size <= max_bytes)
        };
        while let Some((_, response, secret)) = queues.responses.front() {
            let size = response.len() + secret.as_ref().map_or(0, Vec::len);
            if !fits(&items, bytes, size, 1 + usize::from(secret.is_some())) {
                break;
            }
            let (_, response, secret) = queues.responses.pop_front().expect("front exists");
            queues.reserved -= 1;
            bytes += size;
            items.push(Item {
                kind: ItemKind::Response,
                data: response,
            });
            if let Some(secret) = secret {
                items.push(Item {
                    kind: ItemKind::Secret,
                    data: secret,
                });
            }
        }
        if queues.lost > 0 {
            let lost = serde_json::to_vec(&serde_json::json!({
                "type": "events_lost",
                "count": queues.lost,
            }))
            .expect("events_lost encodes");
            if fits(&items, bytes, lost.len(), 1) {
                queues.lost = 0;
                bytes += lost.len();
                items.push(Item {
                    kind: ItemKind::Event,
                    data: lost,
                });
            }
        }
        while queues.lost == 0
            && let Some(event) = queues.events.front()
        {
            if !fits(&items, bytes, event.len(), 1) {
                break;
            }
            let event = queues.events.pop_front().expect("front exists");
            bytes += event.len();
            items.push(Item {
                kind: ItemKind::Event,
                data: event,
            });
        }
        drop(queues);
        let Some(runtime) = self.host_runtime() else {
            return Ok(items);
        };
        let mut count = items.len();
        let packets = runtime.take_game_packets(accept_packets, |len| {
            let size = PACKET_EVENT_BYTES + len;
            let fits = count + 2 <= max_items && (count == 0 || bytes + size <= max_bytes);
            if fits {
                count += 2;
                bytes += size;
            }
            fits
        });
        for (session_id, packet) in packets {
            items.push(Item {
                kind: ItemKind::Event,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "game_session_packet",
                    "session_id": session_id,
                    "delivery": packet.delivery.as_str(),
                }))
                .expect("packet event encodes"),
            });
            items.push(Item {
                kind: ItemKind::Bytes,
                data: packet.bytes.to_vec(),
            });
        }
        // 視聴中の live の映像（session ごとに最新の 1 枚。packet と同じく未解放の batch の上限に数える）。
        #[cfg(feature = "vp8")]
        for (session_id, (timestamp_us, audio_clock_us, hops, frame)) in
            self.live.take_frames(|len| {
                let size = PACKET_EVENT_BYTES + len;
                let fits = accept_packets
                    && count + 2 <= max_items
                    && (count == 0 || bytes + size <= max_bytes);
                if fits {
                    count += 2;
                    bytes += size;
                }
                fits
            })
        {
            items.push(Item {
                kind: ItemKind::Event,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "live_video_frame",
                    "session_id": session_id,
                    "width": frame.width,
                    "height": frame.height,
                    "timestamp_us": timestamp_us,
                    "audio_clock_us": audio_clock_us,
                    "hops": hops,
                }))
                .expect("live frame event encodes"),
            });
            items.push(Item {
                kind: ItemKind::Bytes,
                data: frame.i420,
            });
        }
        Ok(items)
    }

    /// 新しい要求を断り、実行前と実行中の読み取りを `interrupted` で、実行中の書き込みを完了か
    /// `operation_outcome_unknown` で終え、runtime と profile の所有を解放する。結果は close の後も poll できる。
    pub fn close(&mut self) -> Status {
        let Some(runtime) = self.runtime.take() else {
            return Status::Closed;
        };
        self.shared.queues().closed = true;
        let _ = self.shutdown.send(true);
        let shared = self.shared.clone();
        let started = self.started.borrow().clone();
        runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + CLOSE_GRACE;
            let writes_done = async {
                loop {
                    let settled = shared.settled.notified();
                    if shared.queues().pending.is_empty() {
                        break;
                    }
                    settled.await;
                }
            };
            if tokio::time::timeout_at(deadline, writes_done)
                .await
                .is_err()
            {
                let mut queues = shared.queues();
                let unknown: Vec<String> = queues.pending.keys().cloned().collect();
                for request_id in unknown {
                    queues.pending.remove(&request_id);
                    let response = ResponseEnvelope::failure_parts(
                        "",
                        request_id.clone(),
                        self.profile.clone(),
                        ProtocolError::new(
                            error_code::OPERATION_OUTCOME_UNKNOWN,
                            "the handle closed before the write finished",
                        ),
                    );
                    queues
                        .responses
                        .push_back((request_id, encode(&response), None));
                }
            }
            if let Some(Ok(started)) = started {
                let _ =
                    tokio::time::timeout_at(deadline, started.dispatcher.finish_operations()).await;
                if let Some(session) = &started.session {
                    session.shutdown().await;
                }
            }
        });
        // 起動の task と event の転送を止めてから、profile の lock を外す。
        runtime.shutdown_timeout(Duration::from_secs(1));
        drop(
            self.lease
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take(),
        );
        Status::Ok
    }
}

impl Drop for SdkClient {
    fn drop(&mut self) {
        self.close();
    }
}

struct RequestTask {
    shared: Arc<Shared>,
    started: watch::Receiver<StartResult>,
    shutdown: watch::Receiver<bool>,
    running: Arc<Semaphore>,
    profile: String,
    publish: bool,
    #[cfg(feature = "vp8")]
    live: Arc<crate::live::Live>,
}

/// ゲームの Player から発行させない command。
const PUBLISH_COMMAND: &str = "publish_game_asset";

impl RequestTask {
    async fn run(
        mut self,
        request: RequestEnvelope,
        secret: Option<SecretInput>,
        mut cancelled: oneshot::Receiver<()>,
    ) -> (Vec<u8>, Option<Vec<u8>>) {
        let fail = |code: &str, message: &str| {
            (failure(&request, ProtocolError::new(code, message)), None)
        };
        if request.command == PUBLISH_COMMAND && !self.publish {
            return fail(
                error_code::AUTHORIZATION_FAILED,
                "game assets are published from the development Sidecar or the CLI, not from a game",
            );
        }
        // 期限は受け付けた時から数える（起動と実行の枠の待ちを含む）。
        let deadline = match request.timeout_ms() {
            Ok(timeout) => tokio::time::Instant::now() + Duration::from_millis(timeout),
            Err(error) => return (failure(&request, error), None),
        };
        let mut shutdown = self.shutdown.clone();
        // 実行前: 起動の完了と実行の枠を待つ。取消・close・期限で抜ける。
        let waiting = async {
            let started = self
                .started
                .wait_for(Option::is_some)
                .await
                .ok()
                .and_then(|started| started.clone());
            let permit = self.running.clone().acquire_owned().await.ok();
            (started, permit)
        };
        let (started, _permit) = tokio::select! {
            biased;
            _ = &mut cancelled => return fail(CANCELLED, "the request was cancelled before it started"),
            _ = shutdown.wait_for(|closed| *closed) => return fail(error_code::INTERRUPTED, "the handle closed before the request started"),
            _ = tokio::time::sleep_until(deadline) => return fail(error_code::TIMEOUT, "command timed out"),
            ready = waiting => ready,
        };
        let started = match started {
            Some(Ok(started)) => started,
            Some(Err(error)) => return (failure(&request, error), None),
            None => {
                return fail(
                    error_code::INTERRUPTED,
                    "the handle closed before startup finished",
                );
            }
        };
        if let Some(pending) = self.shared.queues().pending.get_mut(&request.request_id) {
            pending.cancel = None;
        }
        // 登録に無い command は dispatcher が not_found で返すので、読み取りとして扱ってよい。
        let write = matches!(
            started
                .dispatcher
                .registry()
                .get(&request.command)
                .map(|registration| registration.metadata.effect),
            Some(CommandEffect::Write | CommandEffect::Destructive)
        );
        let dispatch = tokio::time::timeout_at(
            deadline,
            started
                .dispatcher
                .dispatch(request.clone(), secret, &self.profile, None),
        );
        let reply = if write {
            dispatch.await
        } else {
            tokio::select! {
                reply = dispatch => reply,
                _ = self.shutdown.wait_for(|closed| *closed) => {
                    return fail(error_code::INTERRUPTED, "the handle closed during the read");
                }
            }
        };
        match reply {
            Err(_) => fail(error_code::TIMEOUT, "command timed out"),
            Ok(DispatchReply::Unary(response, secret)) => {
                #[cfg(feature = "vp8")]
                if response.ok {
                    self.follow_live(&started, &request);
                }
                (
                    encode(&response),
                    secret.map(|secret| secret.expose().to_vec()),
                )
            }
            // event は Ready から poll で届けるので、購読の command は受け付けない。
            Ok(DispatchReply::Events { .. }) => fail(
                error_code::VALIDATION_FAILED,
                "events are delivered by poll; streaming commands are not accepted",
            ),
        }
    }
}

#[cfg(feature = "vp8")]
impl RequestTask {
    /// 視聴を始めた・止めた session の映像の復号を、runtime の視聴に合わせて始める・止める。
    fn follow_live(&self, started: &Started, request: &RequestEnvelope) {
        let session_id = request.payload["session_id"].as_str().unwrap_or_default();
        match request.command.as_str() {
            "watch_live_media" => {
                let runtime = started
                    .session
                    .as_ref()
                    .and_then(|session| session.host())
                    .map(|host| host.runtime());
                let topic = request.payload["topic"].as_str().unwrap_or_default();
                if let Some(runtime) = runtime {
                    self.live
                        .watch(runtime, topic.to_owned(), session_id.to_owned());
                }
            }
            "stop_watching_live_media" => self.live.stop_watching(session_id),
            _ => {}
        }
    }
}

/// `[sample rate u32][channel 数 u16][interleave した f32]` のゲームの音を配信の送り待ちへ置く。
fn push_live_audio(runtime: &DesktopRuntime, session_id: &str, bytes: &[u8]) -> Status {
    let (Some(rate), Some(channels)) = (
        bytes
            .get(0..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        bytes.get(4..6).map(|b| u16::from_le_bytes([b[0], b[1]])),
    ) else {
        return Status::InvalidArgument;
    };
    let samples = &bytes[6..];
    if bytes.len() > MAX_LIVE_AUDIO_BYTES
        || channels == 0
        || !samples.len().is_multiple_of(4 * usize::from(channels))
    {
        return Status::InvalidArgument;
    }
    let samples: Vec<f32> = samples
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    if runtime.push_live_game_audio(session_id, rate, channels, &samples) {
        Status::Ok
    } else {
        Status::Closed
    }
}

/// 起動の完了と host の出入りを知らせ、host の runtime event を待ち行列へ写す。
async fn pump_events(
    shared: Arc<Shared>,
    session: Option<Arc<ClientSession>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let changed = || {
        serde_json::to_vec(&serde_json::json!({"type": "startup_status_changed"}))
            .expect("event encodes")
    };
    shared.push_event(changed());
    let Some(session) = session else { return };
    let mut current: Option<Arc<ClientHost>> = None;
    let mut receiver = None;
    loop {
        let host = session.host();
        let same = match (&host, &current) {
            (Some(host), Some(current)) => Arc::ptr_eq(host, current),
            (None, None) => true,
            _ => false,
        };
        if !same {
            receiver = host.as_ref().map(|host| host.subscribe_events());
            current = host;
            shared.push_event(changed());
        }
        let next = async {
            match receiver.as_mut() {
                Some(receiver) => receiver.next().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = shutdown.wait_for(|closed| *closed) => return,
            _ = shared.host_changed.notified() => {}
            event = next => match event {
                Some(event) => {
                    if let Ok(event) = serde_json::to_vec(&event) {
                        shared.push_event(event);
                    }
                }
                // host が止まった。次の host が現れるまで待つ。
                None => receiver = None,
            },
        }
    }
}

fn failure(request: &RequestEnvelope, error: ProtocolError) -> Vec<u8> {
    encode(&ResponseEnvelope::failure(request, error))
}

fn encode(response: &ResponseEnvelope) -> Vec<u8> {
    serde_json::to_vec(response).expect("response envelope encodes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared {
            queues: Mutex::new(Queues::default()),
            faulted: AtomicBool::new(false),
            host_changed: Notify::new(),
            settled: Notify::new(),
        }
    }

    #[test]
    fn events_beyond_the_limit_drop_the_oldest_and_count_them() {
        let shared = shared();
        for index in 0..MAX_EVENTS + 10 {
            shared.push_event(index.to_string().into_bytes());
        }
        let queues = shared.queues();
        assert_eq!(queues.events.len(), MAX_EVENTS);
        assert_eq!(queues.lost, 10);
        assert_eq!(queues.events.front().unwrap(), b"10");
    }

    /// F-ABI-13: 捨てた件数は `events_lost` で、残った event より先に届く。応答は捨てない。
    #[test]
    fn lost_events_are_reported_before_the_remaining_ones() {
        let registry = kukuri_cli::registry::CommandRegistry::new(Vec::new()).unwrap();
        let client = SdkClient::open_with("p".into(), async move {
            Ok(Started {
                dispatcher: Dispatcher::new(registry),
                session: None,
            })
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        client
            .shared
            .queues()
            .responses
            .push_back((String::new(), b"{}".to_vec(), None));
        client.shared.queues().reserved += 1;
        for index in 0..MAX_EVENTS + 10 {
            client
                .shared
                .push_event(format!("{{\"n\":{index}}}").into_bytes());
        }
        let items = client.poll(3, usize::MAX, true).unwrap();
        assert_eq!(items[0].kind, ItemKind::Response);
        let lost: serde_json::Value = serde_json::from_slice(&items[1].data).unwrap();
        assert_eq!(lost["type"], "events_lost");
        assert_eq!(
            lost["count"], 11,
            "the startup notice and the 10 oldest were dropped"
        );
        assert_eq!(items[2].data, b"{\"n\":10}");
    }

    /// 1 件だけで max_bytes を超えるものも、その poll の最初なら単独で返す（ADR 0070 §3）。
    #[test]
    fn an_item_larger_than_max_bytes_is_returned_alone() {
        let registry = kukuri_cli::registry::CommandRegistry::new(Vec::new()).unwrap();
        let client = SdkClient::open_with("p".into(), async move {
            Ok(Started {
                dispatcher: Dispatcher::new(registry),
                session: None,
            })
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        client.poll(64, usize::MAX, true).unwrap();
        client.shared.push_event(vec![b'x'; 4096]);
        client.shared.push_event(b"small".to_vec());
        let first = client.poll(64, 1024, true).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].data.len(), 4096);
        assert_eq!(client.poll(64, 1024, true).unwrap()[0].data, b"small");
    }
}

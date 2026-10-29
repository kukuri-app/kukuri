//! Windows の開発用 Sidecar（ADR 0070 §8.2）。Sidecar の process が `SdkClient` を持ち、named pipe 越しに
//! send・cancel・poll を中継する。待ち行列・取消・終了の意味は FFI と同じ `SdkClient` が担う。
//!
//! pipe は profile directory から名前を決め、同じ名前の pipe を先に作られていたら起動しない。remote の client を拒否し、
//! DACL は Sidecar を起動した user だけに読み書きを許す。接続は一度に一つで、最初の frame（hello）で ABI の版と
//! profile を照合してから要求を受ける。TCP の listen はしない。

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use kukuri_cli::protocol::{ProtocolError, ResponseEnvelope, error_code};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    sync::watch,
};

use crate::{ABI_VERSION, Item, ItemKind, SdkClient, Status};

/// frame 1 つの上限。応答と event の JSON（1 MiB）より大きな poll の結果も入るよう、未解放の batch の上限に揃える。
const MAX_FRAME_BYTES: usize = 64 << 20;
const ERROR_PIPE_BUSY: i32 = 231;
/// 使用中の pipe を開き直す回数（5 ms ごと、合わせて 100 ms まで）。
const PIPE_BUSY_RETRIES: usize = 20;

/// profile directory から決める pipe の名前。大文字小文字と区切りの違いで別の Sidecar にならないよう正規化する。
pub fn pipe_name(profile_dir: &Path) -> String {
    let normalized = profile_dir
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    let hash = blake3::hash(normalized.as_bytes()).to_hex();
    format!(r"\\.\pipe\kukuri-sdk-{}", &hash[..32])
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Op {
    Hello {
        abi_version: u32,
        profile: String,
        profile_dir: PathBuf,
    },
    /// 続く frame は要求、`secret` なら秘密の入力。
    Send {
        secret: bool,
    },
    Cancel {
        request_id: String,
    },
    Poll {
        max_items: usize,
        max_bytes: usize,
    },
    /// Sidecar を止める（`kukuri-sdk-sidecar stop`）。
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
struct Reply {
    status: i32,
    /// hello を断った理由の code。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    /// send: 書き込みの command か（分からなければ書き込みとして扱う）。
    #[serde(default)]
    write: bool,
    /// poll: 続く frame の種類。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    kinds: Vec<u32>,
}

impl Reply {
    fn status(status: Status) -> Self {
        Self {
            status: status as i32,
            code: None,
            write: false,
            kinds: Vec::new(),
        }
    }
}

fn status_from(code: i32) -> Status {
    match code {
        0 => Status::Ok,
        1 => Status::InvalidArgument,
        2 => Status::Backpressure,
        3 => Status::Closed,
        5 => Status::AbiMismatch,
        _ => Status::Faulted,
    }
}

fn kind_from(code: u32) -> Option<ItemKind> {
    match code {
        1 => Some(ItemKind::Response),
        2 => Some(ItemKind::Secret),
        3 => Some(ItemKind::Event),
        _ => None,
    }
}

fn request_id_of(json: &[u8]) -> Option<(String, String)> {
    let value: Value = serde_json::from_slice(json).ok()?;
    Some((
        value.get("request_id")?.as_str()?.to_owned(),
        value
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    ))
}

async fn read_frame(pipe: &mut NamedPipeServer) -> std::io::Result<Vec<u8>> {
    let len = pipe.read_u32_le().await? as usize;
    if len > MAX_FRAME_BYTES {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut frame = vec![0; len];
    pipe.read_exact(&mut frame).await?;
    Ok(frame)
}

async fn write_frame(pipe: &mut NamedPipeServer, frame: &[u8]) -> std::io::Result<()> {
    pipe.write_u32_le(frame.len() as u32).await?;
    pipe.write_all(frame).await
}

async fn write_reply(pipe: &mut NamedPipeServer, reply: &Reply) -> std::io::Result<()> {
    write_frame(pipe, &serde_json::to_vec(reply).expect("reply encodes")).await
}

/// `client` を named pipe で公開する。`stop` か shutdown の要求で戻る。戻った後に呼出側が `client.close()` する。
pub async fn serve(
    client: &SdkClient,
    profile_dir: &Path,
    mut stop: watch::Receiver<bool>,
    ready: impl FnOnce(),
) -> std::io::Result<()> {
    let name = pipe_name(profile_dir);
    let security = security::CurrentUserOnly::new()?;
    let create = |first: bool| {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        // SAFETY: security attributes は `security` が serve の間持ち続ける。
        unsafe { options.create_with_security_attributes_raw(&name, security.as_ptr()) }
    };
    let mut server = create(true)?;
    ready();
    loop {
        tokio::select! {
            _ = stop.wait_for(|stopped| *stopped) => return Ok(()),
            connected = server.connect() => connected?,
        }
        // 次の接続を受ける pipe をすぐ用意する（切断の直後に接続し直す client が使用中で断られないように）。
        let mut pipe = std::mem::replace(&mut server, create(false)?);
        let shutdown = {
            let mut attachment =
                std::pin::pin!(attach(&mut pipe, client, profile_dir, stop.clone()));
            loop {
                tokio::select! {
                    shutdown = &mut attachment => break shutdown,
                    connected = server.connect() => {
                        connected?;
                        // 接続は一度に一つ。接続中に来た別の client は同じ profile の二重の所有として断る。
                        let busy = std::mem::replace(&mut server, create(false)?);
                        tokio::spawn(refuse_busy(busy));
                    }
                }
            }
        };
        let _ = pipe.disconnect();
        if shutdown {
            return Ok(());
        }
    }
}

async fn refuse_busy(mut pipe: NamedPipeServer) {
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), read_frame(&mut pipe)).await;
    let mut reply = Reply::status(Status::InvalidArgument);
    reply.code = Some(error_code::PROFILE_IN_USE.into());
    let _ = write_reply(&mut pipe, &reply).await;
    let _ = pipe.disconnect();
}

/// 一つの接続を中継する。shutdown を受けたら true。
async fn attach(
    pipe: &mut NamedPipeServer,
    client: &SdkClient,
    profile_dir: &Path,
    mut stop: watch::Receiver<bool>,
) -> bool {
    let hello = match read_frame(pipe).await {
        Ok(frame) => serde_json::from_slice::<Op>(&frame).ok(),
        Err(_) => return false,
    };
    let refused = match &hello {
        Some(Op::Hello { abi_version, .. }) if *abi_version != ABI_VERSION => {
            Some((Status::AbiMismatch, error_code::PROTOCOL_MISMATCH))
        }
        Some(Op::Hello {
            profile,
            profile_dir: dir,
            ..
        }) if profile != client.profile() || pipe_name(dir) != pipe_name(profile_dir) => {
            Some((Status::InvalidArgument, error_code::AUTHORIZATION_FAILED))
        }
        Some(Op::Hello { .. }) => None,
        _ => Some((Status::InvalidArgument, error_code::INVALID_REQUEST)),
    };
    if let Some((status, code)) = refused {
        let mut reply = Reply::status(status);
        reply.code = Some(code.into());
        let _ = write_reply(pipe, &reply).await;
        return false;
    }
    if write_reply(pipe, &Reply::status(Status::Ok)).await.is_err() {
        return false;
    }
    // 新しい受け手は状態を読み直すので、接続していない間に溜まった event は渡さない。
    client.clear_events();
    let mut outstanding: Vec<String> = Vec::new();
    let shutdown = loop {
        let frame = tokio::select! {
            _ = stop.wait_for(|stopped| *stopped) => break false,
            frame = read_frame(pipe) => frame,
        };
        let Ok(op) = frame.map(|frame| serde_json::from_slice::<Op>(&frame)) else {
            break false;
        };
        let reply = match op {
            Ok(Op::Send { secret }) => {
                let Ok(request) = read_frame(pipe).await else {
                    break false;
                };
                let secret = if secret {
                    match read_frame(pipe).await {
                        Ok(secret) => Some(secret),
                        Err(_) => break false,
                    }
                } else {
                    None
                };
                let status = client.send(&request, secret.as_deref());
                let mut reply = Reply::status(status);
                if let (Status::Ok, Some((request_id, command))) = (status, request_id_of(&request))
                {
                    reply.write = client.is_write(&command).unwrap_or(true);
                    outstanding.push(request_id);
                }
                write_reply(pipe, &reply).await
            }
            Ok(Op::Cancel { request_id }) => {
                write_reply(pipe, &Reply::status(client.cancel(&request_id))).await
            }
            Ok(Op::Poll {
                max_items,
                max_bytes,
            }) => match client.poll(max_items, max_bytes) {
                Ok(items) => {
                    for item in &items {
                        if item.kind == ItemKind::Response
                            && let Some((request_id, _)) = request_id_of(&item.data)
                        {
                            outstanding.retain(|pending| *pending != request_id);
                        }
                    }
                    let mut reply = Reply::status(Status::Ok);
                    reply.kinds = items.iter().map(|item| item.kind as u32).collect();
                    let mut result = write_reply(pipe, &reply).await;
                    for item in &items {
                        if result.is_ok() {
                            result = write_frame(pipe, &item.data).await;
                        }
                    }
                    result
                }
                Err(status) => write_reply(pipe, &Reply::status(status)).await,
            },
            Ok(Op::Shutdown) => {
                let _ = write_reply(pipe, &Reply::status(Status::Ok)).await;
                break true;
            }
            Ok(Op::Hello { .. }) | Err(_) => break false,
        };
        if reply.is_err() {
            break false;
        }
    };
    // 切断した受け手の要求の応答は捨てる。実行中の書き込みは止めず、結果は受け手の側で成否不明になる。
    client.abandon(&outstanding);
    shutdown
}

/// DLL の中の Sidecar の client。同期の file I/O で pipe と往復する（1 往復はローカルで数十 µs）。
pub struct SidecarClient {
    pipe: Option<std::fs::File>,
    /// 応答を待っている要求（request_id → (command, 書き込みか)）。
    outstanding: HashMap<String, (String, bool)>,
    /// 切断・close で作った結果。次の poll で返す。
    local: std::collections::VecDeque<Item>,
    /// pipe が使えないときの理由の code。
    unavailable: &'static str,
    profile: String,
    closed: bool,
}

impl SidecarClient {
    pub fn connect(profile: String, profile_dir: &Path) -> Self {
        let mut client = Self {
            pipe: None,
            outstanding: HashMap::new(),
            local: Default::default(),
            unavailable: error_code::DAEMON_UNAVAILABLE,
            profile,
            closed: false,
        };
        let name = pipe_name(profile_dir);
        let open = || {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
        };
        let mut opened = open();
        // Sidecar が接続を受けてから次の pipe を用意するまでの短い間は使用中になる。少しだけ待って開き直す。
        for _ in 0..PIPE_BUSY_RETRIES {
            match &opened {
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    opened = open();
                }
                _ => break,
            }
        }
        let Ok(mut pipe) = opened else {
            return client;
        };
        let hello = Op::Hello {
            abi_version: ABI_VERSION,
            profile: client.profile.clone(),
            profile_dir: profile_dir.to_path_buf(),
        };
        let accepted = write_sync(
            &mut pipe,
            &serde_json::to_vec(&hello).expect("hello encodes"),
        )
        .and_then(|()| read_reply(&mut pipe));
        match accepted {
            Ok(reply) if reply.status == Status::Ok as i32 => client.pipe = Some(pipe),
            Ok(reply) if reply.code.as_deref() == Some(error_code::AUTHORIZATION_FAILED) => {
                client.unavailable = error_code::AUTHORIZATION_FAILED;
            }
            // 別の client が接続している。同じ profile の二重の所有と同じ扱い。
            Ok(reply) if reply.code.as_deref() == Some(error_code::PROFILE_IN_USE) => {
                client.unavailable = error_code::PROFILE_IN_USE;
            }
            Ok(reply) if reply.status == Status::AbiMismatch as i32 => {
                client.unavailable = error_code::PROTOCOL_MISMATCH;
            }
            _ => {}
        }
        client
    }

    fn failure(&self, request_id: &str, command: &str, code: &str) -> Item {
        let response = ResponseEnvelope::failure_parts(
            command,
            request_id,
            self.profile.clone(),
            ProtocolError::new(code, "the sidecar is not reachable"),
        );
        Item {
            kind: ItemKind::Response,
            data: serde_json::to_vec(&response).expect("response encodes"),
        }
    }

    /// pipe を失った。応答を待っている読み取りは `interrupted`、書き込みは `operation_outcome_unknown` で終える。
    fn disconnect(&mut self, code: &'static str) {
        self.pipe = None;
        self.unavailable = code;
        let outstanding: Vec<_> = self.outstanding.drain().collect();
        for (request_id, (command, write)) in outstanding {
            let code = if write {
                error_code::OPERATION_OUTCOME_UNKNOWN
            } else {
                error_code::INTERRUPTED
            };
            let item = self.failure(&request_id, &command, code);
            self.local.push_back(item);
        }
    }

    fn rpc(&mut self, op: &Op, frames: &[&[u8]]) -> Option<Reply> {
        let pipe = self.pipe.as_mut()?;
        let result = (|| {
            write_sync(pipe, &serde_json::to_vec(op).expect("op encodes"))?;
            for frame in frames {
                write_sync(pipe, frame)?;
            }
            read_reply(pipe)
        })();
        match result {
            Ok(reply) => Some(reply),
            Err(_) => {
                self.disconnect(error_code::DAEMON_UNAVAILABLE);
                None
            }
        }
    }

    pub fn send(&mut self, request: &[u8], secret: Option<&[u8]>) -> Status {
        if self.closed {
            return Status::Closed;
        }
        let Some((request_id, command)) = request_id_of(request) else {
            return Status::InvalidArgument;
        };
        if self.outstanding.contains_key(&request_id) {
            return Status::InvalidArgument;
        }
        let mut frames = vec![request];
        frames.extend(secret);
        let was_connected = self.pipe.is_some();
        match self.rpc(
            &Op::Send {
                secret: secret.is_some(),
            },
            &frames,
        ) {
            Some(reply) => {
                let status = status_from(reply.status);
                if status == Status::Ok {
                    self.outstanding.insert(request_id, (command, reply.write));
                }
                status
            }
            None => {
                // 送る途中で切れた要求は、届いたかどうかも読み取りか書き込みかも分からないので成否不明とする。
                let code = if was_connected {
                    error_code::OPERATION_OUTCOME_UNKNOWN
                } else {
                    self.unavailable
                };
                let item = self.failure(&request_id, &command, code);
                self.local.push_back(item);
                Status::Ok
            }
        }
    }

    pub fn cancel(&mut self, request_id: &str) -> Status {
        if !self.outstanding.contains_key(request_id) {
            return Status::InvalidArgument;
        }
        self.rpc(
            &Op::Cancel {
                request_id: request_id.into(),
            },
            &[],
        )
        .map_or(Status::Ok, |reply| status_from(reply.status))
    }

    pub fn poll(&mut self, max_items: usize, max_bytes: usize) -> Result<Vec<Item>, Status> {
        let mut items: Vec<Item> = Vec::new();
        while items.len() < max_items.max(1)
            && let Some(item) = self.local.pop_front()
        {
            items.push(item);
        }
        if !items.is_empty() || self.pipe.is_none() {
            return Ok(items);
        }
        let Some(reply) = self.rpc(
            &Op::Poll {
                max_items,
                max_bytes,
            },
            &[],
        ) else {
            return Ok(self.local.drain(..).collect());
        };
        if reply.status != Status::Ok as i32 {
            return Err(status_from(reply.status));
        }
        for kind in reply.kinds {
            let frame = self.pipe.as_mut().map(read_sync);
            let Some(Ok(data)) = frame else {
                self.disconnect(error_code::DAEMON_UNAVAILABLE);
                return Ok(self.local.drain(..).collect());
            };
            let Some(kind) = kind_from(kind) else {
                continue;
            };
            if kind == ItemKind::Response
                && let Some((request_id, _)) = request_id_of(&data)
            {
                self.outstanding.remove(&request_id);
            }
            items.push(Item { kind, data });
        }
        Ok(items)
    }

    /// 接続を切る（Sidecar の runtime は止めない）。応答を待っている要求は disconnect と同じ結果で終える。
    pub fn close(&mut self) -> Status {
        if self.closed {
            return Status::Closed;
        }
        self.closed = true;
        self.disconnect(error_code::DAEMON_UNAVAILABLE);
        Status::Ok
    }
}

/// `kukuri-sdk-sidecar stop`: 動いている Sidecar を止める。
pub fn request_shutdown(profile: String, profile_dir: &Path) -> Result<(), &'static str> {
    let mut client = SidecarClient::connect(profile, profile_dir);
    if client.pipe.is_none() {
        return Err(client.unavailable);
    }
    client
        .rpc(&Op::Shutdown, &[])
        .map(|_| ())
        .ok_or(error_code::DAEMON_UNAVAILABLE)
}

fn write_sync(pipe: &mut std::fs::File, frame: &[u8]) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(4 + frame.len());
    buffer.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    buffer.extend_from_slice(frame);
    pipe.write_all(&buffer)
}

fn read_sync(pipe: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
    let mut len = [0; 4];
    pipe.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut frame = vec![0; len];
    pipe.read_exact(&mut frame)?;
    Ok(frame)
}

fn read_reply(pipe: &mut std::fs::File) -> std::io::Result<Reply> {
    serde_json::from_slice(&read_sync(pipe)?).map_err(std::io::Error::other)
}

mod security {
    //! Sidecar を起動した user だけに読み書きを許す DACL（ADR 0070 §8.2）。

    use std::ffi::c_void;

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    pub struct CurrentUserOnly {
        attributes: SECURITY_ATTRIBUTES,
        descriptor: PSECURITY_DESCRIPTOR,
    }

    // SAFETY: descriptor は作った後に変更せず、drop で一度だけ解放する。
    unsafe impl Send for CurrentUserOnly {}
    unsafe impl Sync for CurrentUserOnly {}

    impl CurrentUserOnly {
        pub fn new() -> std::io::Result<Self> {
            let sid = current_user_sid()?;
            // D:P = 継承しない DACL、GA = 読み書き全般を current user にだけ許す。
            let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: sddl は NUL 終端、descriptor は書ける pointer。
            let converted = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if converted == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                attributes: SECURITY_ATTRIBUTES {
                    nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor,
                    bInheritHandle: 0,
                },
                descriptor,
            })
        }

        pub fn as_ptr(&self) -> *mut c_void {
            (&raw const self.attributes).cast_mut().cast()
        }
    }

    impl Drop for CurrentUserOnly {
        fn drop(&mut self) {
            // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW が LocalAlloc で確保した。
            unsafe { LocalFree(self.descriptor) };
        }
    }

    fn current_user_sid() -> std::io::Result<String> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: 自分の process の token を読むだけ。
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0u8; 256];
        let mut needed = 0;
        // SAFETY: buffer は needed bytes まで書ける。
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut needed,
            )
        };
        let result = if read == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            // SAFETY: TokenUser の buffer は TOKEN_USER から始まる。
            let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
            let mut text: *mut u16 = std::ptr::null_mut();
            // SAFETY: Sid は token の buffer の中を指す。
            if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                // SAFETY: text は NUL 終端の UTF-16。
                let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
                let sid =
                    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
                unsafe { LocalFree(text.cast()) };
                Ok(sid)
            }
        };
        // SAFETY: OpenProcessToken が開いた handle。
        unsafe { CloseHandle(token) };
        result
    }
}

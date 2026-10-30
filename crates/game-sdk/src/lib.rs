//! kukuri Game SDK の C ABI（ADR 0070 §3）。Unity などの外部 process が、独立した profile の kukuri の client を
//! process の中で起動・操作・停止する。関数は network を待たず（close を除く）、panic を ABI の外へ出さない。

mod client;
#[cfg(windows)]
pub mod sidecar;

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, Once},
};

pub use client::{
    BINARY_DATAGRAM, BINARY_STREAM, CANCELLED, Item, ItemKind, MAX_EVENTS, MAX_PENDING,
    MAX_POLL_ITEMS, Mode, SdkClient, SdkConfig, Started, Status,
};

/// C# が読み込み直後に照合する ABI の版（2: `kukuri_sdk_send_binary` と batch の bytes の項目。#1979）。
pub const ABI_VERSION: u32 = 2;
/// poll 済みで未解放の batch の合計の上限。達したら新しい packet を受け取らない（ADR 0070 §3、F-ABI-16）。
pub const MAX_UNRELEASED_BYTES: usize = 64 << 20;

#[repr(C)]
pub struct KukuriItem {
    pub kind: u32,
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
pub struct KukuriBatch {
    pub items: *const KukuriItem,
    pub count: usize,
}

/// poll で渡す batch。`view` を先頭に置き、呼出側にはその pointer を渡す。
#[repr(C)]
struct OwnedBatch {
    view: KukuriBatch,
    items: Vec<KukuriItem>,
    data: Vec<Vec<u8>>,
}

/// handle の要求の行き先。FFI は process の中の client、Sidecar は named pipe の先の client。
pub enum Backend {
    Local(SdkClient),
    #[cfg(windows)]
    Sidecar(sidecar::SidecarClient),
}

impl Backend {
    fn send(&mut self, request: &[u8], secret: Option<&[u8]>) -> Status {
        match self {
            Self::Local(client) => client.send(request, secret),
            #[cfg(windows)]
            Self::Sidecar(client) => client.send(request, secret),
        }
    }

    fn cancel(&mut self, request_id: &str) -> Status {
        match self {
            Self::Local(client) => client.cancel(request_id),
            #[cfg(windows)]
            Self::Sidecar(client) => client.cancel(request_id),
        }
    }

    fn send_binary(&mut self, kind: u32, target: &str, bytes: &[u8]) -> Status {
        match self {
            Self::Local(client) => client.send_binary(kind, target, bytes),
            #[cfg(windows)]
            Self::Sidecar(client) => client.send_binary(kind, target, bytes),
        }
    }

    fn poll(
        &mut self,
        max_items: usize,
        max_bytes: usize,
        accept: bool,
    ) -> Result<Vec<Item>, Status> {
        match self {
            Self::Local(client) => client.poll(max_items, max_bytes, accept),
            #[cfg(windows)]
            Self::Sidecar(client) => client.poll(max_items, max_bytes, accept),
        }
    }

    fn close(&mut self) -> Status {
        match self {
            Self::Local(client) => client.close(),
            #[cfg(windows)]
            Self::Sidecar(client) => client.close(),
        }
    }
}

#[derive(Default)]
struct Batches {
    /// 解放されていない batch の address と bytes。別の handle の batch・解放済みの batch を見分ける。
    owned: HashMap<usize, usize>,
    bytes: usize,
}

pub struct KukuriHandle {
    client: Mutex<Backend>,
    batches: Mutex<Batches>,
}

impl KukuriHandle {
    /// 起動の処理を差し替えた handle（試験で dispatcher を注入する）。
    pub fn from_client(client: SdkClient) -> *mut Self {
        Self::from_backend(Backend::Local(client))
    }

    pub fn from_backend(client: Backend) -> *mut Self {
        Box::into_raw(Box::new(Self {
            client: Mutex::new(client),
            batches: Mutex::default(),
        }))
    }

    fn client(&self) -> std::sync::MutexGuard<'_, Backend> {
        self.client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn batches(&self) -> std::sync::MutexGuard<'_, Batches> {
        self.batches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn guard(call: impl FnOnce() -> Status) -> i32 {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or(Status::Faulted) as i32
}

/// panic の既定の出力は payload（要求の中身を含みうる）を書くので、固定の文だけにする（ADR 0049 の daemon と同じ）。
fn quiet_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| std::panic::set_hook(Box::new(|_| eprintln!("kukuri sdk task panicked"))));
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return (len == 0).then_some(&[]);
    }
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

#[unsafe(no_mangle)]
pub extern "C" fn kukuri_sdk_abi_version() -> u32 {
    ABI_VERSION
}

/// `config` は `{"profile": 名前, "profile_dir": 絶対 path}` の UTF-8 JSON。runtime の起動は背景で進み、
/// 完了を `startup_status_changed` の event で知らせる。
///
/// # Safety
/// `config` は `len` bytes 読め、`out` は書ける pointer であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_open(
    abi_version: u32,
    config: *const u8,
    len: usize,
    out: *mut *mut KukuriHandle,
) -> i32 {
    guard(|| {
        if out.is_null() {
            return Status::InvalidArgument;
        }
        unsafe { *out = std::ptr::null_mut() };
        if abi_version != ABI_VERSION {
            return Status::AbiMismatch;
        }
        let Some(config) = (unsafe { bytes(config, len) }) else {
            return Status::InvalidArgument;
        };
        let Ok(config) = serde_json::from_slice::<SdkConfig>(config) else {
            return Status::InvalidArgument;
        };
        if config.profile.trim().is_empty() || !config.profile_dir.is_absolute() {
            return Status::InvalidArgument;
        }
        let backend = match config.mode {
            // runtime の task は FFI の mode だけが DLL の中で動かす。
            Mode::Ffi => {
                quiet_panics();
                match SdkClient::open(config) {
                    Ok(client) => Backend::Local(client),
                    Err(_) => return Status::Faulted,
                }
            }
            #[cfg(windows)]
            Mode::Sidecar => Backend::Sidecar(sidecar::SidecarClient::connect(
                config.profile,
                &config.profile_dir,
            )),
            #[cfg(not(windows))]
            Mode::Sidecar => return Status::InvalidArgument,
        };
        unsafe { *out = KukuriHandle::from_backend(backend) };
        Status::Ok
    })
}

/// 要求（`RequestEnvelope` の JSON）を受け付けて即座に戻る。`secret` は秘密の入力（無ければ null）。
///
/// # Safety
/// `handle` は open が返した解放前の handle、`request` と `secret` はそれぞれの長さだけ読めること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_send(
    handle: *mut KukuriHandle,
    request: *const u8,
    len: usize,
    secret: *const u8,
    secret_len: usize,
) -> i32 {
    guard(|| {
        let (Some(handle), Some(request)) =
            (unsafe { handle.as_ref() }, unsafe { bytes(request, len) })
        else {
            return Status::InvalidArgument;
        };
        let secret = if secret.is_null() {
            None
        } else {
            unsafe { bytes(secret, secret_len) }
        };
        handle.client().send(request, secret)
    })
}

/// session の packet を送り待ちに置いて即座に戻る。`kind` は 1 が datagram、2 が stream、`target` は session ID。
/// 上限を超える大きさ・不明な kind は `INVALID_ARGUMENT`、閉じた session は `CLOSED`、stream の送り待ちが上限に
/// 達したら `BACKPRESSURE`（受け付けない）。datagram は送り待ちが溢れたら古いものを捨てて `OK`。
///
/// # Safety
/// `handle` は解放前の handle、`target` と `bytes` はそれぞれの長さだけ読めること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_send_binary(
    handle: *mut KukuriHandle,
    kind: u32,
    target: *const u8,
    target_len: usize,
    data: *const u8,
    len: usize,
) -> i32 {
    guard(|| {
        let (Some(handle), Some(target), Some(data)) = (
            unsafe { handle.as_ref() },
            unsafe { bytes(target, target_len) },
            unsafe { bytes(data, len) },
        ) else {
            return Status::InvalidArgument;
        };
        let Ok(target) = std::str::from_utf8(target) else {
            return Status::InvalidArgument;
        };
        handle.client().send_binary(kind, target, data)
    })
}

/// 実行前の要求を取り消す。
///
/// # Safety
/// `handle` は解放前の handle、`request_id` は `len` bytes 読めること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_cancel(
    handle: *mut KukuriHandle,
    request_id: *const u8,
    len: usize,
) -> i32 {
    guard(|| {
        let (Some(handle), Some(request_id)) = (unsafe { handle.as_ref() }, unsafe {
            bytes(request_id, len)
        }) else {
            return Status::InvalidArgument;
        };
        let Ok(request_id) = std::str::from_utf8(request_id) else {
            return Status::InvalidArgument;
        };
        handle.client().cancel(request_id)
    })
}

/// 届いているものを上限まで batch に入れる。何も届いていなければ `*out` は null。
///
/// # Safety
/// `handle` は解放前の handle、`out` は書ける pointer であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_poll(
    handle: *mut KukuriHandle,
    max_items: usize,
    max_bytes: usize,
    out: *mut *const KukuriBatch,
) -> i32 {
    guard(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return Status::InvalidArgument;
        };
        if out.is_null() {
            return Status::InvalidArgument;
        }
        unsafe { *out = std::ptr::null() };
        let accept = handle.batches().bytes < MAX_UNRELEASED_BYTES;
        let items = match handle.client().poll(max_items, max_bytes, accept) {
            Ok(items) => items,
            Err(status) => return status,
        };
        if items.is_empty() {
            return Status::Ok;
        }
        let mut owned = Box::new(OwnedBatch {
            view: KukuriBatch {
                items: std::ptr::null(),
                count: 0,
            },
            items: Vec::with_capacity(items.len()),
            data: Vec::with_capacity(items.len()),
        });
        for item in items {
            owned.items.push(KukuriItem {
                kind: item.kind as u32,
                data: item.data.as_ptr(),
                len: item.data.len(),
            });
            owned.data.push(item.data);
        }
        owned.view = KukuriBatch {
            items: owned.items.as_ptr(),
            count: owned.items.len(),
        };
        let size = owned.data.iter().map(Vec::len).sum::<usize>();
        let batch = Box::into_raw(owned);
        let mut batches = handle.batches();
        batches.owned.insert(batch as usize, size);
        batches.bytes += size;
        unsafe { *out = batch.cast::<KukuriBatch>() };
        Status::Ok
    })
}

/// poll で受けた batch を解放する。解放済み・別の handle の batch は `INVALID_ARGUMENT` で、何も解放しない。
///
/// # Safety
/// `handle` は解放前の handle であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_batch_release(
    handle: *mut KukuriHandle,
    batch: *const KukuriBatch,
) -> i32 {
    guard(|| {
        let Some(handle) = (unsafe { handle.as_ref() }) else {
            return Status::InvalidArgument;
        };
        let mut batches = handle.batches();
        let Some(size) = batches.owned.remove(&(batch as usize)) else {
            return Status::InvalidArgument;
        };
        batches.bytes -= size;
        drop(batches);
        drop(unsafe { Box::from_raw(batch.cast_mut().cast::<OwnedBatch>()) });
        Status::Ok
    })
}

/// 新しい要求を断って runtime と profile の所有を解放する（最大で約 5 秒待つ）。未完了の要求の結果は close の後も
/// poll できる。handle の memory は `kukuri_sdk_free` で解放する。
///
/// # Safety
/// `handle` は解放前の handle であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_close(handle: *mut KukuriHandle) -> i32 {
    guard(|| match unsafe { handle.as_ref() } {
        Some(handle) => handle.client().close(),
        None => Status::InvalidArgument,
    })
}

/// handle と解放されていない batch を解放する（close していなければ close する）。以後 handle は使えない。
///
/// # Safety
/// `handle` は open が返した解放前の handle で、他の thread が同時に使っていないこと。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_sdk_free(handle: *mut KukuriHandle) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return;
        }
        let handle = unsafe { Box::from_raw(handle) };
        let batches = std::mem::take(&mut handle.batches().owned);
        for batch in batches.into_keys() {
            drop(unsafe { Box::from_raw(batch as *mut OwnedBatch) });
        }
        drop(handle);
    }));
}

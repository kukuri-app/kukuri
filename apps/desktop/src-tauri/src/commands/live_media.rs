use std::sync::Mutex;
use std::time::Duration;

use kukuri_desktop_runtime::{
    LiveBroadcastStatus, LiveMediaRequest, LiveVideoSource, LiveVolumeRequest,
    SetLiveBroadcastRequest,
};
use tauri::webview::{PermissionKind, PermissionResponse};

use crate::state::{CommandError, DesktopState, map_error};

/// 配信中の映像の source(webview の camera・画面の取り込みの許可に使う。ADR 0067 §9、#1034)。
#[derive(Default)]
pub struct LiveCaptureGate(Mutex<Option<LiveVideoSource>>);

impl LiveCaptureGate {
    /// 配信を始めた source の取り込みだけを許す。それ以外は platform の既定のまま(Windows は確認を出し、Linux は
    /// 拒否する)。
    pub fn respond(&self, kind: PermissionKind) -> PermissionResponse {
        match (kind, *self.0.lock().expect("live capture gate poisoned")) {
            (PermissionKind::Camera, Some(LiveVideoSource::Camera))
            | (PermissionKind::DisplayCapture, Some(LiveVideoSource::Screen)) => {
                PermissionResponse::Allow
            }
            _ => PermissionResponse::Default,
        }
    }

    fn set(&self, source: Option<LiveVideoSource>) {
        *self.0.lock().expect("live capture gate poisoned") = source;
    }
}

/// WebKitGTK は既定で `getUserMedia`・`getDisplayMedia` を持たない。許可は `LiveCaptureGate` が決める。
#[cfg(target_os = "linux")]
pub fn enable_media_stream<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    use tauri::Manager;
    use webkit2gtk::{SettingsExt, WebViewExt};
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    window.with_webview(|platform| {
        if let Some(settings) = platform.inner().settings() {
            settings.set_enable_media_stream(true);
            let _ = enable_track_processing(&settings);
        }
    })
}

/// WebKitGTKのWorker用取得APIだけを有効にする。2.42未満は既存のpreview経路を使う。
#[cfg(target_os = "linux")]
fn enable_track_processing(settings: &webkit2gtk::Settings) -> Option<()> {
    use gtk::glib::translate::ToGlibPtr;
    use std::ffi::{CStr, c_char, c_int, c_void};
    type Ptr = *mut c_void;
    let settings_pointer: *mut webkit2gtk::ffi::WebKitSettings = settings.to_glib_none().0;
    // APIはWebKitFeature.h/WebKitSettings.hのC ABI。全symbolを解決してから所有listを作る。
    unsafe {
        let library = libloading::os::unix::Library::this();
        let all = library
            .get::<unsafe extern "C" fn() -> Ptr>(b"webkit_settings_get_all_features\0")
            .ok()?;
        let length = library
            .get::<unsafe extern "C" fn(Ptr) -> usize>(b"webkit_feature_list_get_length\0")
            .ok()?;
        let get = library
            .get::<unsafe extern "C" fn(Ptr, usize) -> Ptr>(b"webkit_feature_list_get\0")
            .ok()?;
        let identifier = library
            .get::<unsafe extern "C" fn(Ptr) -> *const c_char>(b"webkit_feature_get_identifier\0")
            .ok()?;
        let set = library
            .get::<unsafe extern "C" fn(Ptr, Ptr, c_int)>(b"webkit_settings_set_feature_enabled\0")
            .ok()?;
        let unref = library
            .get::<unsafe extern "C" fn(Ptr)>(b"webkit_feature_list_unref\0")
            .ok()?;
        let list = all();
        for index in 0..length(list) {
            let feature = get(list, index);
            if CStr::from_ptr(identifier(feature)).to_bytes() == b"MediaStreamTrackProcessing" {
                set(settings_pointer.cast(), feature, 1);
            }
        }
        unref(list);
    }
    Some(())
}

/// live の配信を始める・source と mic を切り替える。画面は返った上限で camera か画面を取り込み、VP8 に圧縮して
/// `push_live_video` へ渡す。
#[tauri::command]
pub async fn set_live_broadcast(
    state: tauri::State<'_, DesktopState>,
    gate: tauri::State<'_, LiveCaptureGate>,
    request: SetLiveBroadcastRequest,
) -> Result<LiveBroadcastStatus, CommandError> {
    let source = request.source;
    let status = state
        .runtime()
        .set_live_broadcast(request)
        .await
        .map_err(map_error)?;
    gate.set(Some(source));
    Ok(status)
}

/// 圧縮した映像の chunk(raw の body)。header に session id・keyframe(`1`)・取り込みからの経過(μs)。
#[tauri::command]
pub async fn push_live_video(
    state: tauri::State<'_, DesktopState>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), CommandError> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err(CommandError::from(
            "live video must be raw bytes".to_string(),
        ));
    };
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let (Some(session_id), Some(keyframe), Some(age)) = (
        header("x-session-id"),
        header("x-keyframe"),
        header("x-age-us").and_then(|age| age.parse::<u64>().ok()),
    ) else {
        return Err(CommandError::from(
            "live video headers are missing".to_string(),
        ));
    };
    state
        .runtime()
        .push_live_video(
            session_id,
            keyframe == "1",
            Duration::from_micros(age),
            data.clone(),
        )
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn stop_live_broadcast(
    state: tauri::State<'_, DesktopState>,
    gate: tauri::State<'_, LiveCaptureGate>,
    request: LiveMediaRequest,
) -> Result<(), CommandError> {
    gate.set(None);
    state
        .runtime()
        .stop_live_broadcast(request)
        .await
        .map_err(map_error)
}

/// 配信者の端末へ購読し、音声を再生する。映像は `next_live_video` で取る。
#[tauri::command]
pub async fn watch_live_media(
    state: tauri::State<'_, DesktopState>,
    request: LiveMediaRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .watch_live_media(request)
        .await
        .map_err(map_error)
}

/// 送りかけの映像と視聴の状態(raw の応答。届くまで最大 1 秒待つ)。
#[tauri::command]
pub async fn next_live_video(
    state: tauri::State<'_, DesktopState>,
    request: LiveMediaRequest,
) -> Result<tauri::ipc::Response, CommandError> {
    state
        .runtime()
        .next_live_video(request)
        .await
        .map(tauri::ipc::Response::new)
        .map_err(map_error)
}

#[tauri::command]
pub async fn stop_watching_live_media(
    state: tauri::State<'_, DesktopState>,
    request: LiveMediaRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .stop_watching_live_media(request)
        .await
        .map_err(map_error)
}

/// 視聴中の live の音量(#1839 AC-4)。この端末だけの設定。
#[tauri::command]
pub async fn set_live_volume(
    state: tauri::State<'_, DesktopState>,
    request: LiveVolumeRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .set_live_volume(request)
        .await
        .map_err(map_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    // #1034 T1: 配信を始めた source の取り込みだけを許し、止めたら platform の既定へ戻す。
    #[test]
    fn capture_is_allowed_only_for_the_broadcast_source() {
        let gate = LiveCaptureGate::default();
        let responses = |gate: &LiveCaptureGate| {
            [
                PermissionKind::Camera,
                PermissionKind::DisplayCapture,
                PermissionKind::Microphone,
            ]
            .map(|kind| gate.respond(kind))
        };
        use PermissionResponse::{Allow, Default};
        assert_eq!(responses(&gate), [Default, Default, Default]);
        gate.set(Some(LiveVideoSource::Camera));
        assert_eq!(responses(&gate), [Allow, Default, Default]);
        gate.set(Some(LiveVideoSource::Screen));
        assert_eq!(responses(&gate), [Default, Allow, Default]);
        gate.set(None);
        assert_eq!(responses(&gate), [Default, Default, Default]);
    }
}

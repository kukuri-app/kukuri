//! Updater IPC boundary. Only the backend selects targets and retains verified bytes.
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, ipc::Channel};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

const DEB_TARGET: &str = "linux-x86_64-deb";
const STORE_MANAGED_UPDATE: &str = "update_managed_by_microsoft_store";
const PLAY_MANAGED_UPDATE: &str = "update_managed_by_google_play";

/// AndroidはGoogle Playだけで配る（#1193 D3）。updater pluginを登録しないため、その状態へ触れる前に拒否する。
fn require_self_managed_updates() -> Result<(), String> {
    match managed_update(cfg!(target_os = "android"), cfg!(feature = "microsoft-store")) {
        Some(reason) => Err(reason.into()),
        None => Ok(()),
    }
}

/// 更新を管理する配布元（Google Play・Microsoft Store）の拒否の理由。host の test でも Android の分岐を確かめる。
fn managed_update(android: bool, microsoft_store: bool) -> Option<&'static str> {
    if android {
        Some(PLAY_MANAGED_UPDATE)
    } else if microsoft_store {
        Some(STORE_MANAGED_UPDATE)
    } else {
        None
    }
}

struct Pending {
    id: u32,
    update: Update,
    verified: Option<Vec<u8>>,
}

#[derive(Default)]
struct Session {
    next_id: u32,
    pending: Option<Pending>,
    installed: bool,
}

#[derive(Default)]
pub(crate) struct AppUpdateState(Mutex<Session>);

#[derive(Serialize)]
pub(crate) struct UpdateMetadata {
    id: u32,
    version: String,
}

fn is_deb() -> bool {
    matches!(
        tauri::utils::platform::bundle_type(),
        Some(tauri::utils::config::BundleType::Deb)
    )
}

fn validate_deb_manifest(
    raw: &serde_json::Value,
    version: &str,
    url: &str,
    signature: &str,
) -> Result<(), String> {
    let entry = &raw["platforms"][DEB_TARGET];
    let expected = format!("kukuri_{version}_amd64.deb");
    if entry["url"].as_str() != Some(url)
        || entry["signature"].as_str() != Some(signature)
        || !url
            .split('?')
            .next()
            .unwrap_or_default()
            .ends_with(&format!("/{expected}"))
    {
        return Err("deb_update_manifest_mismatch".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn check_app_update<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<UpdateMetadata>, String> {
    require_self_managed_updates()?;
    let state = app.state::<AppUpdateState>();
    let mut session = state.0.try_lock().map_err(|_| "update_busy")?;
    if session.installed
        || session
            .pending
            .as_ref()
            .is_some_and(|pending| pending.verified.is_some())
    {
        return Err("update_already_downloaded".into());
    }
    session.pending = None;
    let mut builder = app.updater_builder();
    if is_deb() {
        // An explicit target disables upstream's AppImage fallback.
        builder = builder.target(DEB_TARGET);
    }
    let update = builder
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let Some(update) = update else {
        return Ok(None);
    };
    if is_deb() {
        validate_deb_manifest(
            &update.raw_json,
            &update.version,
            update.download_url.as_str(),
            &update.signature,
        )?;
    }
    session.next_id = session
        .next_id
        .checked_add(1)
        .ok_or("update_session_exhausted")?;
    let id = session.next_id;
    let version = update.version.clone();
    session.pending = Some(Pending {
        id,
        update,
        verified: None,
    });
    Ok(Some(UpdateMetadata { id, version }))
}

#[tauri::command]
pub(crate) async fn download_app_update<R: Runtime>(
    app: AppHandle<R>,
    id: u32,
    on_event: Channel<serde_json::Value>,
) -> Result<(), String> {
    require_self_managed_updates()?;
    let state = app.state::<AppUpdateState>();
    let mut session = state.0.try_lock().map_err(|_| "update_busy")?;
    let pending = session
        .pending
        .as_mut()
        .filter(|pending| pending.id == id)
        .ok_or("update_stale_session")?;
    if pending.verified.is_some() {
        return Err("update_already_downloaded".into());
    }
    let mut started = false;
    let bytes = pending
        .update
        .download(
            |length, total| {
                if !started {
                    started = true;
                    let _ = on_event.send(
                        serde_json::json!({"event": "Started", "data": {"contentLength": total}}),
                    );
                }
                let _ = on_event.send(
                    serde_json::json!({"event": "Progress", "data": {"chunkLength": length}}),
                );
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    // Upstream's finish callback precedes crypto verification. Emit Finished only after success.
    pending.verified = Some(bytes);
    let _ = on_event.send(serde_json::json!({"event": "Finished"}));
    Ok(())
}

#[tauri::command]
pub(crate) async fn install_app_update<R: Runtime>(
    app: AppHandle<R>,
    id: u32,
) -> Result<(), String> {
    require_self_managed_updates()?;
    install_checked(app, id, is_deb()).await
}

async fn install_checked<R: Runtime>(app: AppHandle<R>, id: u32, deb: bool) -> Result<(), String> {
    let state = app.state::<AppUpdateState>();
    let mut session = state.0.try_lock().map_err(|_| "update_busy")?;
    let operations = app.state::<crate::restore_lifecycle::DesktopOperationState>();
    let _operation = operations.switch_guard.lock().await;
    crate::desktop_lifecycle::require_running(&app).map_err(|_| "update_app_stopping")?;
    let pending = session
        .pending
        .as_mut()
        .filter(|pending| pending.id == id)
        .ok_or("update_stale_session")?;
    let bytes = pending.verified.take().ok_or("update_not_verified")?;
    let update = pending.update.clone();
    // Consume the authorization attempt even on failure; no implicit install/authentication retry.
    let result = tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "linux")]
        if deb {
            return crate::deb_update::install(&bytes, &update.version);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = deb;
        update.install(bytes).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "update_install_task_failed")?;
    session.installed = result.is_ok();
    result
}

pub(crate) fn require_installed<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    require_self_managed_updates()?;
    let state = app.state::<AppUpdateState>();
    let session = state.0.try_lock().map_err(|_| "update_busy")?;
    if session.installed {
        Ok(())
    } else {
        Err("update_not_installed".into())
    }
}

#[cfg(test)]
#[path = "app_update_tests.rs"]
mod boundary_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_feature_owns_the_update_boundary() {
        // Android（Google Play）の分岐は host の build を通らないので、判定そのものも確かめる（#1199 AC-3）。
        assert_eq!(managed_update(true, false), Some(PLAY_MANAGED_UPDATE));
        assert_eq!(managed_update(true, true), Some(PLAY_MANAGED_UPDATE));
        assert_eq!(managed_update(false, true), Some(STORE_MANAGED_UPDATE));
        assert_eq!(managed_update(false, false), None);
        let result = require_self_managed_updates();
        if cfg!(target_os = "android") {
            assert_eq!(result.unwrap_err(), PLAY_MANAGED_UPDATE);
        } else if cfg!(feature = "microsoft-store") {
            assert_eq!(result.unwrap_err(), STORE_MANAGED_UPDATE);
        } else {
            result.expect("direct distributions keep the self-managed updater");
        }
    }

    #[test]
    fn deb_manifest_requires_exact_format_version_and_embedded_signature() {
        let url = "https://example.invalid/kukuri_0.1.9_amd64.deb";
        let valid =
            serde_json::json!({"platforms": {DEB_TARGET: {"url": url, "signature": "sig"}}});
        assert!(validate_deb_manifest(&valid, "0.1.9", url, "sig").is_ok());
        for raw in [
            serde_json::json!({"platforms": {"linux-x86_64": {"url": url, "signature": "sig"}}}),
            serde_json::json!({"url": url, "signature": "sig"}),
            serde_json::json!({"platforms": {DEB_TARGET: {"url": url, "signature": "different"}}}),
        ] {
            assert!(validate_deb_manifest(&raw, "0.1.9", url, "sig").is_err());
        }
        assert!(validate_deb_manifest(&valid, "0.2.0", url, "sig").is_err());
        let appimage = url.replace(".deb", ".AppImage");
        let raw =
            serde_json::json!({"platforms": {DEB_TARGET: {"url": appimage, "signature": "sig"}}});
        assert!(validate_deb_manifest(&raw, "0.1.9", &appimage, "sig").is_err());
    }
}

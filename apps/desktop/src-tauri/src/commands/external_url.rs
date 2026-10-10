use std::future::Future;

use tauri::{AppHandle, Manager, Url};

use crate::{
    desktop_lifecycle::DesktopLifecycle,
    state::{CommandError, DesktopStartupState, DesktopStartupStatus},
};

const OPEN_FAILED: &str = "external browser launch could not be confirmed";
const INVALID_URL: &str = "only absolute HTTP(S) URLs without credentials are supported";

fn validate_url(value: &str) -> Result<Url, &'static str> {
    let authority = value
        .split("//")
        .nth(1)
        .unwrap_or_default()
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if value.chars().any(|c| c.is_control() || c.is_whitespace())
        || authority.is_empty()
        || authority.contains('@')
        || !(value.to_ascii_lowercase().starts_with("https://")
            || value.to_ascii_lowercase().starts_with("http://"))
        || value.contains('\\')
    {
        return Err(INVALID_URL);
    }
    let url = Url::parse(value).map_err(|_| INVALID_URL)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(INVALID_URL);
    }
    Ok(url)
}

async fn open_with<F, Fut>(
    value: &str,
    status: &DesktopStartupStatus,
    stopping: bool,
    launch: F,
) -> Result<(), &'static str>
where
    F: FnOnce(Url) -> Fut,
    Fut: Future<Output = Result<(), &'static str>>,
{
    // Also guard the sink itself; do not depend on the frontend or plugin ACL.
    if stopping || !matches!(status, DesktopStartupStatus::Ready) {
        return Err("external browser launch requires a running Ready desktop");
    }
    let url = validate_url(value)?;
    launch(url).await
}

#[cfg(target_os = "linux")]
async fn launch_browser(url: Url) -> Result<(), &'static str> {
    launch_portal(url, None, std::time::Duration::from_secs(60)).await
}

#[cfg(target_os = "linux")]
async fn launch_portal(
    url: Url,
    connection: Option<zbus::Connection>,
    timeout: std::time::Duration,
) -> Result<(), &'static str> {
    // The portal launches outside the AppImage environment. Never pass its
    // LD_LIBRARY_PATH/GIO_MODULE_DIR to an xdg-open/browser child process.
    let request = async {
        let uri = ashpd::Uri::parse(url.as_str()).map_err(|_| OPEN_FAILED)?;
        ashpd::desktop::open_uri::OpenFileRequest::default()
            .connection(connection)
            .send_uri(&uri)
            .await
            .map_err(|_| OPEN_FAILED)?
            .response()
            .map_err(|_| OPEN_FAILED)
    };
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| OPEN_FAILED)?
}

#[cfg(windows)]
async fn launch_browser(url: Url) -> Result<(), &'static str> {
    // This feature uses ShellExecuteExW directly, not cmd.exe or a shell string.
    tauri::async_runtime::spawn_blocking(move || open::that_detached(url.as_str()))
        .await
        .map_err(|_| OPEN_FAILED)?
        .map_err(|_| OPEN_FAILED)
}

#[cfg(target_os = "android")]
async fn launch_browser(url: Url) -> Result<(), &'static str> {
    use jni::{JValue, jni_sig, jni_str, objects::JObject};

    tauri::async_runtime::spawn_blocking(move || {
        let context = ndk_context::android_context();
        // SAFETY: tao の process 所有の JavaVM / Application Context。local refs は attach の frame 内だけ。
        let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast()) };
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let value = env.new_string(url.as_str())?;
            let uri = env
                .call_static_method(
                    jni_str!("android/net/Uri"),
                    jni_str!("parse"),
                    jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                    &[JValue::Object(&value)],
                )?
                .l()?;
            let action = env.new_string("android.intent.action.VIEW")?;
            let intent = env.new_object(
                jni_str!("android/content/Intent"),
                jni_sig!("(Ljava/lang/String;Landroid/net/Uri;)V"),
                &[JValue::Object(&action), JValue::Object(&uri)],
            )?;
            // Application Context から起動するため FLAG_ACTIVITY_NEW_TASK を付ける。
            env.call_method(
                &intent,
                jni_str!("addFlags"),
                jni_sig!("(I)Landroid/content/Intent;"),
                &[JValue::Int(0x10000000)],
            )?;
            let application = unsafe { JObject::from_raw(env, context.context().cast()) };
            env.call_method(
                &application,
                jni_str!("startActivity"),
                jni_sig!("(Landroid/content/Intent;)V"),
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        })
    })
    .await
    .map_err(|_| OPEN_FAILED)?
    .map_err(|_| OPEN_FAILED)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
async fn launch_browser(_url: Url) -> Result<(), &'static str> {
    Err(OPEN_FAILED)
}

#[tauri::command]
pub async fn open_external_url(app: AppHandle, url: String) -> Result<(), CommandError> {
    let status = app.state::<DesktopStartupState>().status();
    let stopping = app.state::<DesktopLifecycle>().requested();
    open_with(&url, &status, stopping, launch_browser)
        .await
        .map_err(|message| CommandError::from(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(target_os = "linux")]
    mod portal {
        use super::*;
        use std::{
            collections::HashMap,
            io::{BufRead, BufReader},
            process::{Child, Command, Stdio},
            sync::{Arc, atomic::AtomicU8},
            time::Duration,
        };
        use zbus::zvariant::{OwnedObjectPath, OwnedValue};

        struct Bus(Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        struct Portal {
            mode: Arc<AtomicU8>,
            hits: Arc<AtomicUsize>,
        }

        #[zbus::interface(name = "org.freedesktop.portal.OpenURI")]
        impl Portal {
            #[zbus(property, name = "version")]
            fn version(&self) -> u32 {
                5
            }

            #[zbus(name = "OpenURI")]
            async fn open_uri(
                &self,
                parent: &str,
                uri: &str,
                options: HashMap<String, OwnedValue>,
                #[zbus(connection)] connection: &zbus::Connection,
                #[zbus(header)] header: zbus::message::Header<'_>,
            ) -> zbus::fdo::Result<OwnedObjectPath> {
                assert_eq!(parent, "");
                assert_eq!(uri, "https://example.test/policy");
                self.hits.fetch_add(1, Ordering::SeqCst);
                let mode = self.mode.load(Ordering::SeqCst);
                if mode == 2 {
                    return Err(zbus::fdo::Error::AccessDenied("fixture".into()));
                }
                if mode == 3 {
                    std::future::pending::<()>().await;
                }
                let token = options["handle_token"].downcast_ref::<&str>().unwrap();
                let sender = header.sender().unwrap();
                let sender_path = sender.as_str().trim_start_matches(':').replace('.', "_");
                let path = OwnedObjectPath::try_from(format!(
                    "/org/freedesktop/portal/desktop/request/{sender_path}/{token}"
                ))
                .unwrap();
                connection
                    .emit_signal(
                        Some(sender.as_str()),
                        path.as_str(),
                        "org.freedesktop.portal.Request",
                        "Response",
                        &(u32::from(mode), HashMap::<String, OwnedValue>::new()),
                    )
                    .await
                    .unwrap();
                Ok(path)
            }
        }

        #[tokio::test]
        async fn blocking_dbus_client_remains_usable_inside_async_startup() {
            // The existing Secret Service client uses zbus's blocking API from
            // startup. A unified zbus/tokio feature must not turn an ordinary
            // connection failure into a nested-runtime panic.
            let address = format!(
                "unix:path=/tmp/kukuri-889-missing-bus-{}.socket",
                std::process::id()
            );
            let result = zbus::blocking::connection::Builder::address(address.as_str())
                .unwrap()
                .build();
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn actual_portal_protocol_handles_absence_cancel_denial_timeout_and_recovery() {
            let mut bus = Bus(Command::new("dbus-daemon")
                .args([
                    "--config-file",
                    concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/fixtures/external-url-bus.conf"
                    ),
                    "--nofork",
                    "--print-address=1",
                ])
                .stdout(Stdio::piped())
                .spawn()
                .expect("portal test requires dbus-daemon"));
            let mut address = String::new();
            BufReader::new(bus.0.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            let client = zbus::connection::Builder::address(address.trim())
                .unwrap()
                .build()
                .await
                .unwrap();
            let url = Url::parse("https://example.test/policy").unwrap();
            assert!(
                launch_portal(url.clone(), Some(client.clone()), Duration::from_secs(1))
                    .await
                    .is_err()
            );
            let mode = Arc::new(AtomicU8::new(0));
            let hits = Arc::new(AtomicUsize::new(0));
            let _service = zbus::connection::Builder::address(address.trim())
                .unwrap()
                .name("org.freedesktop.portal.Desktop")
                .unwrap()
                .serve_at(
                    "/org/freedesktop/portal/desktop",
                    Portal {
                        mode: mode.clone(),
                        hits: hits.clone(),
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            for state in [0, 1, 2, 3, 0] {
                mode.store(state, Ordering::SeqCst);
                let result = launch_portal(
                    url.clone(),
                    Some(client.clone()),
                    Duration::from_millis(200),
                )
                .await;
                assert_eq!(result.is_err(), state != 0, "portal state {state}");
            }
            assert_eq!(hits.load(Ordering::SeqCst), 5);
        }
    }

    #[test]
    fn permits_web_urls_and_preserves_path_query_fragment() {
        for value in [
            "https://example.test/policy?q=a%20b#section",
            "http://127.0.0.1:8080/rights",
            "https://example.test/?q=a&next=%22test%22",
        ] {
            assert_eq!(validate_url(value).unwrap().as_str(), value);
        }
    }

    #[test]
    fn invalid_urls_never_reach_os_sink() {
        tauri::async_runtime::block_on(async {
            let hits = AtomicUsize::new(0);
            for value in [
                "",
                "/relative",
                "#skip",
                "//example.test",
                "https:example.test",
                "file:///tmp/a",
                "javascript:alert(1)",
                "data:text/html,x",
                "mailto:a@example.test",
                "kukuri:topic/test",
                "C:\\app.exe",
                "https://user:secret@example.test",
                "https://example.test\n/path",
                " https://example.test",
                "https://example.test/a b",
                "https://",
                "https://example.test\\@other.test",
                "https:///example.test",
                "https://@example.test",
                "https://example.test/\0",
            ] {
                assert!(
                    open_with(value, &DesktopStartupStatus::Ready, false, |_| async {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
                    .is_err(),
                    "{value:?}"
                );
            }
            assert_eq!(hits.load(Ordering::SeqCst), 0);
        });
    }

    #[test]
    fn non_ready_and_stopping_never_reach_os_sink() {
        tauri::async_runtime::block_on(async {
            let hits = AtomicUsize::new(0);
            let consent = crate::state::consent_required_status(&Default::default());
            for (status, stopping) in [
                (DesktopStartupStatus::Initializing, false),
                (consent, false),
                (DesktopStartupStatus::Ready, true),
            ] {
                assert!(
                    open_with("https://example.test", &status, stopping, |_| async {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
                    .is_err()
                );
            }
            assert_eq!(hits.load(Ordering::SeqCst), 0);
        });
    }

    #[test]
    fn os_failure_is_returned_and_explicit_retry_can_recover() {
        tauri::async_runtime::block_on(async {
            let hits = AtomicUsize::new(0);
            for fail in [true, false] {
                let result = open_with(
                    "https://example.test",
                    &DesktopStartupStatus::Ready,
                    false,
                    |_| async {
                        hits.fetch_add(1, Ordering::SeqCst);
                        if fail { Err(OPEN_FAILED) } else { Ok(()) }
                    },
                )
                .await;
                assert_eq!(result.is_err(), fail);
            }
            assert_eq!(hits.load(Ordering::SeqCst), 2);
        });
    }
}

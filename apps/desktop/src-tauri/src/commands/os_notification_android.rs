use serde_json::json;
use tauri::{
    AppHandle, Manager, Wry,
    plugin::{PluginHandle, TauriPlugin},
};

struct Notifications(PluginHandle<Wry>);

pub(crate) fn plugin() -> TauriPlugin<Wry> {
    tauri::plugin::Builder::new("android-notifications")
        .setup(|app, api| {
            app.manage(Notifications(api.register_android_plugin(
                "app.kukuri.android",
                "NotificationPlugin",
            )?));
            Ok(())
        })
        .build()
}

pub(crate) async fn permission(app: AppHandle, request: bool) -> String {
    app.state::<Notifications>()
        .0
        .run_mobile_plugin_async::<String>(
            if request { "request" } else { "permission" },
            json!({}),
        )
        .await
        .unwrap_or_else(|_| "unavailable".into())
}

pub(crate) async fn show(
    app: AppHandle,
    id: String,
    title: String,
    body: Option<String>,
    silent: bool,
) -> Result<(), String> {
    app.state::<Notifications>()
        .0
        .run_mobile_plugin_async::<()>(
            "show",
            json!({ "id": id, "title": title, "body": body, "silent": silent }),
        )
        .await
        .map_err(|_| "os_notification_unavailable".into())
}

use std::collections::BTreeSet;

use kukuri_cli::registry::CommandRegistry;
use serde::Deserialize;
use syn::{Token, parse::Parser, punctuated::Punctuated, visit::Visit};

const TAURI_SOURCE: &str = include_str!("../../../apps/desktop/src-tauri/src/lib.rs");
const MANIFEST: &str = include_str!("../command-parity.json");

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    baseline: String,
    scope_revision: String,
    entries: Vec<Entry>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    tauri: String,
    cli: Option<String>,
    inventory: Option<String>,
    excluded: Option<String>,
    reason: Option<String>,
}

#[derive(Default)]
struct HandlerVisitor {
    registrations: Vec<String>,
    macros: usize,
}

impl<'ast> Visit<'ast> for HandlerVisitor {
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let path = path_name(&node.path);
        if path == "tauri::generate_handler" {
            self.macros += 1;
            let entries = Punctuated::<syn::Path, Token![,]>::parse_terminated
                .parse2(node.tokens.clone())
                .expect("generate_handler登録はRust pathの列挙であること");
            self.registrations.extend(entries.iter().map(path_name));
        }
        syn::visit::visit_macro(self, node);
    }
}

fn path_name(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn registrations(source: &str) -> Vec<String> {
    let syntax = syn::parse_file(source).expect("Tauri sourceを構文解析");
    let mut visitor = HandlerVisitor::default();
    visitor.visit_file(&syntax);
    assert_eq!(visitor.macros, 1, "登録点の増減は対応表の再確認が必要");
    visitor.registrations
}

fn manifest() -> Manifest {
    serde_json::from_str(MANIFEST).expect("対応表のJSON")
}

fn check_inventory(actual: &[String], entries: &[Entry]) -> Result<(), String> {
    let actual_set: BTreeSet<_> = actual.iter().collect();
    if actual.len() != actual_set.len() {
        return Err("GUI登録が重複".into());
    }
    let mapped_set: BTreeSet<_> = entries.iter().map(|entry| &entry.tauri).collect();
    if entries.len() != mapped_set.len() {
        return Err("対応表のGUI登録が重複".into());
    }
    if actual_set != mapped_set {
        return Err(format!(
            "GUIと対応表が不一致: 未分類={:?}, 削除済={:?}",
            actual_set.difference(&mapped_set).collect::<Vec<_>>(),
            mapped_set.difference(&actual_set).collect::<Vec<_>>()
        ));
    }
    for entry in entries {
        match (&entry.cli, &entry.excluded, &entry.reason, &entry.inventory) {
            (Some(cli), None, None, Some(inventory))
                if !cli.trim().is_empty()
                    && [
                        "INV-1", "INV-2", "INV-3", "INV-4", "INV-5", "INV-6", "INV-7",
                    ]
                    .contains(&inventory.as_str()) => {}
            (None, Some(kind), Some(reason), None)
                if !reason.trim().is_empty() && exclusion_allowed(&entry.tauri, kind) => {}
            _ => return Err(format!("無効または重複した分類: {}", entry.tauri)),
        }
    }
    Ok(())
}

fn exclusion_allowed(tauri: &str, kind: &str) -> bool {
    match kind {
        "os" => matches!(
            tauri,
            "commands::os_notification::show_os_notification"
                | "commands::os_notification::get_os_notification_permission"
                | "commands::os_notification::request_os_notification_permission"
                | "commands::background_notifications::set_os_notification_settings"
                | "desktop_lifecycle::restart_after_update"
                | "app_update::check_app_update"
                | "app_update::download_app_update"
                | "app_update::install_app_update"
                | "commands::external_url::open_external_url"
                | "commands::system_locale::get_system_locales"
                | "desktop_lifecycle::get_window_close_preference"
                | "desktop_lifecycle::set_window_close_preference"
                | "desktop_lifecycle::get_pending_window_close_request"
                | "desktop_lifecycle::respond_window_close_request"
        ),
        "frontend_state" => matches!(
            tauri,
            "commands::device_backup::get_pending_device_restore_frontend_state"
                | "commands::device_backup::acknowledge_pending_device_restore_frontend_state"
                | "commands::identity::get_profile_setup_required"
                | "commands::identity::save_initial_profile"
        ),
        // #978: GUI process内のin-memory診断(開発者モードのミラーとログ閲覧)。
        "gui_diagnostics" => matches!(
            tauri,
            "commands::developer_logs::set_developer_mode_enabled"
                | "commands::developer_logs::read_desktop_logs"
        ),
        // GUI表示中だけに所有する一時content previewとそのlease。
        "gui_content_preview" => matches!(
            tauri,
            "commands::link_preview::fetch_link_preview"
                | "commands::posts::get_blob_media_file"
                | "commands::posts::release_blob_media_file"
        ),
        // #1284: 表示中の投稿cardだけに作用する局所的な欠損blobの回復。
        "gui_content_recovery" => tauri == "commands::posts::retry_post_elements",
        "gui_visible_membership" => tauri == "commands::posts::bookmarked_post_ids",
        _ => false,
    }
}

fn check_registry(entries: &[Entry], names: &BTreeSet<String>) -> Result<(), String> {
    let mapped: Vec<_> = entries
        .iter()
        .filter_map(|entry| entry.cli.clone())
        .collect();
    let mapped_set: BTreeSet<_> = mapped.iter().cloned().collect();
    if mapped.len() != mapped_set.len() {
        return Err("CLI対応が重複".into());
    }
    let control: BTreeSet<_> = [
        "client.status",
        "events.watch",
        "protocol.commands",
        "protocol.schema",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let expected: BTreeSet<_> = mapped_set.union(&control).cloned().collect();
    if &expected != names {
        return Err(format!(
            "CLI登録簿が不一致: 未実装={:?}, 対応表なし={:?}",
            expected.difference(names).collect::<Vec<_>>(),
            names.difference(&expected).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

#[test]
fn baseline_inventory_is_classified_once() {
    let manifest = manifest();
    assert_eq!(
        manifest.baseline,
        "d372c91bdc963bda07308a359fe3baeca8e06150"
    );
    assert_eq!(
        manifest.scope_revision,
        "2026-09-25-1221-r5-d-private-index-grant-v1"
    );
    assert_eq!(manifest.entries.len(), 172);
    check_inventory(&registrations(TAURI_SOURCE), &manifest.entries).expect("全入口の分類");
}

#[test]
fn mapped_commands_and_runtime_registry_match() {
    let registry = CommandRegistry::builtin();
    let names = registry.schema_document()["commands"]
        .as_object()
        .expect("command一覧")
        .keys()
        .cloned()
        .collect();
    check_registry(&manifest().entries, &names).expect("実handlerとの対応");
}

#[test]
fn detects_added_removed_and_duplicate_gui_commands() {
    let entries = manifest().entries;
    let original = registrations(TAURI_SOURCE);
    let mut added = original.clone();
    added.push("commands::fixture::new_operation".into());
    assert!(check_inventory(&added, &entries).is_err());
    let mut removed = original.clone();
    removed.pop();
    assert!(check_inventory(&removed, &entries).is_err());
    let mut duplicate = original.clone();
    duplicate.push(original[0].clone());
    assert!(check_inventory(&duplicate, &entries).is_err());
    let mut duplicate_entry = entries.clone();
    duplicate_entry.push(entries[0].clone());
    assert!(check_inventory(&original, &duplicate_entry).is_err());
}

#[test]
fn detects_missing_cli_command_and_invalid_exclusions() {
    let mut entries = manifest().entries;
    let mut names: BTreeSet<_> = entries
        .iter()
        .filter_map(|entry| entry.cli.clone())
        .collect();
    names.extend(
        [
            "client.status",
            "events.watch",
            "protocol.commands",
            "protocol.schema",
        ]
        .map(String::from),
    );
    check_registry(&entries, &names).expect("正しい対応表fixture");
    names.remove("create_post");
    assert!(check_registry(&entries, &names).is_err());
    let excluded = entries
        .iter_mut()
        .find(|entry| entry.excluded.is_some())
        .expect("除外行");
    excluded.reason = Some(" ".into());
    assert!(check_inventory(&registrations(TAURI_SOURCE), &entries).is_err());
    let mut entries = manifest().entries;
    entries[0].cli = None;
    entries[0].inventory = None;
    entries[0].excluded = Some("os".into());
    entries[0].reason = Some("未実装をOS固有として除外してはいけない".into());
    assert!(check_inventory(&registrations(TAURI_SOURCE), &entries).is_err());
}

#[test]
fn parser_ignores_comments_and_strings_but_detects_real_fixture_registration() {
    let source = r#"
        fn main() {
            // tauri::generate_handler![commands::fake::comment]
            let _ = "tauri::generate_handler![commands::fake::string]";
            let _ = tauri::generate_handler![commands::fixture::registered,];
        }
    "#;
    assert_eq!(registrations(source), ["commands::fixture::registered"]);
}

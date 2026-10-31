use super::schema::{array, nullable, object};
use serde_json::{Value, json};

fn string() -> Value {
    json!({"type": "string"})
}
fn integer() -> Value {
    json!({"type": "integer"})
}
fn boolean() -> Value {
    json!({"type": "boolean"})
}
fn fields(properties: Value) -> Value {
    let keys = properties
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    object(properties.clone(), &keys)
}
fn game_player() -> Value {
    fields(json!({"game_player_id": string(), "public_key": string()}))
}
fn document() -> Value {
    fields(
        json!({"slug": string(), "currentVersion": integer(), "effectiveDate": string(), "authoritativeLanguage": string(),
        "materialChange": boolean(), "controllerName": string(), "contact": string(), "acceptedVersion": nullable(integer()),
        "acceptedAt": nullable(integer()), "acceptedLanguage": nullable(string()), "acceptedAppVersion": nullable(string())}),
    )
}
fn age() -> Value {
    fields(
        json!({"currentVersion": integer(), "attestedVersion": nullable(integer()), "attestedAt": nullable(integer())}),
    )
}
fn startup() -> Value {
    object(
        json!({"status": {"enum": ["initializing", "ready", "consent_required", "failed"]},
        "documents": array(document()), "age_attestation": age(),
        "error": fields(json!({"kind": {"enum": ["database_open", "database_migration", "profile_in_use", "profile_invalid", "subscription_state", "unknown"]},
            "message": string(), "detail": string(), "db_path": nullable(string())}))}),
        &["status"],
    )
}
fn account() -> Value {
    fields(
        json!({"id": string(), "pubkey": string(), "label": nullable(string()), "created_at": integer(), "last_used_at": integer()}),
    )
}
pub(super) fn input(name: &str) -> Value {
    match name {
        "get_desktop_startup_status"
        | "get_app_consent_status"
        | "list_accounts"
        | "get_account_display"
        | "cancel_device_backup"
        | "export_account_key"
        | "preview_account_key_import" => object(json!({}), &[]),
        "accept_app_consents" => fields(
            json!({"documents": array(fields(json!({"slug": string(), "version": integer()}))), "language": string(), "age_attested": boolean()}),
        ),
        "import_account_key" => object(json!({"label": nullable(string())}), &[]),
        "switch_account" | "logout_account" => fields(json!({"account_id": string()})),
        "start_game_player" | "get_game_player" => fields(json!({"application_id": string()})),
        "create_account" => fields(json!({"account_id": string(), "operation_id": string()})),
        "create_device_backup_command" | "preview_device_backup_command" => {
            fields(json!({"path": string()}))
        }
        "restore_device_backup_command" => object(
            json!({"path": string(), "replace_existing": boolean()}),
            &["path"],
        ),
        _ => unreachable!("登録済みlifecycle command"),
    }
}
pub(super) fn output(name: &str) -> Value {
    match name {
        "get_desktop_startup_status" | "accept_app_consents" => startup(),
        "get_app_consent_status" => fields(
            json!({"documents": array(document()), "ageAttestation": age(), "satisfied": boolean()}),
        ),
        "list_accounts" => {
            fields(json!({"active_account_id": string(), "accounts": array(account())}))
        }
        "export_account_key" => fields(json!({"public_key": string()})),
        "preview_account_key_import" => fields(
            json!({"version": integer(), "kdf": string(), "public_key": string(), "already_registered": boolean()}),
        ),
        "import_account_key" | "switch_account" | "logout_account" | "create_account" => account(),
        "start_game_player" => game_player(),
        "get_game_player" => nullable(game_player()),
        "get_account_display" => array(fields(
            json!({"id": string(), "name": nullable(string()), "display_name": nullable(string()), "picture": nullable(string()), "unavailable": boolean()}),
        )),
        "cancel_device_backup" => json!({"type": "null"}),
        "create_device_backup_command" => {
            fields(json!({"path": string(), "public_key": string(), "bytes": integer()}))
        }
        "preview_device_backup_command" => fields(
            json!({"public_key": string(), "account_label": nullable(string()), "created_at": integer(), "app_version": string(),
            "content_bytes": integer(), "existing_account_id": nullable(string()), "included": array(string()), "requires_reconsent": array(string())}),
        ),
        "restore_device_backup_command" => {
            fields(json!({"account": account(), "frontend_state": object(json!({}), &[])}))
        }
        _ => unreachable!("登録済みlifecycle command"),
    }
}

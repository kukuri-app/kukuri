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
/// 移行の状態（`state` で種類が決まる）。SDK が移行先のときは履歴を受けないが、移行元のときは相手が選べば履歴を送る。
fn transfer_status() -> Value {
    let role = json!({"enum": ["source", "target", "sync"]});
    let reasons = [
        "expired",
        "invalid",
        "unreachable",
        "rejected",
        "cancelled",
        "interrupted",
        "storage",
    ];
    let mut stopped = reasons.map(Value::from).to_vec();
    stopped.push(Value::Null);
    let mut schema = object(
        json!({"state": {"enum": ["idle", "waiting", "connecting", "confirming", "transferring", "history", "completed", "failed"]},
        "expires_at_ms": integer(), "role": role, "code": string(), "local_accepted": boolean(), "items": integer(),
        "account_id": nullable(string()), "posts": integer(), "unavailable": integer(),
        "history": nullable(fields(json!({"posts": integer(), "unavailable": integer(), "stopped": {"enum": stopped}}))),
        "reason": {"enum": reasons}}),
        &["state"],
    );
    schema["description"] = json!(
        "waiting: expires_at_ms。confirming: role・code・local_accepted。transferring: role・items。history: role・account_id・posts・unavailable。completed: role・account_id（移行先が受けたアカウント）・history。failed: role・reason。"
    );
    schema
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
        | "preview_account_key_import"
        | "open_account_transfer"
        | "create_account_transfer_invite"
        | "get_account_transfer_status"
        | "cancel_account_transfer" => object(json!({}), &[]),
        "decide_account_transfer" => fields(json!({"accept": boolean()})),
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
        "cancel_device_backup"
        | "open_account_transfer"
        | "decide_account_transfer"
        | "cancel_account_transfer" => json!({"type": "null"}),
        "get_account_transfer_status" => transfer_status(),
        "create_account_transfer_invite" => fields(json!({"expires_at_ms": integer()})),
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

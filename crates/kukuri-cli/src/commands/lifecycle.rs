use super::{command, command_error, decode, encode, host_guards, lifecycle_schema, runtime};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput, SecretOutput, error_code},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
    session::ClientSession,
};
use async_trait::async_trait;
use kukuri_desktop_runtime::{
    AcceptedAppConsentDocument, CreateDeviceBackupRequest, ExportAccountKeyRequest,
    PreviewDeviceBackupRequest, RestoreDeviceBackupRequest, SwitchAccountRequest,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

struct Handler {
    name: &'static str,
    session: Option<Arc<ClientSession>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentInput {
    documents: Vec<AcceptedAppConsentDocument>,
    language: String,
    age_attested: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeySecret {
    export: String,
    passphrase: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportInput {
    label: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupInput {
    path: String,
    #[serde(default)]
    replace_existing: bool,
}

fn secret_text(secret: Option<&SecretInput>) -> Result<String, ProtocolError> {
    let bytes = secret.ok_or_else(crate::session::failed)?.expose();
    std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| {
        ProtocolError::new(
            error_code::VALIDATION_FAILED,
            "Secret入力はUTF-8で指定してください",
        )
    })
}

#[async_trait]
impl CommandHandler for Handler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let session = self.session.as_ref().ok_or_else(|| {
            ProtocolError::new(
                error_code::ACTION_REQUIRED,
                "profile管理状態が利用できません",
            )
        })?;
        match self.name {
            "create_account" => encode(
                context
                    .host
                    .ok_or_else(crate::session::failed)?
                    .create_account(decode(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "logout_account" => {
                let request: SwitchAccountRequest = decode(payload)?;
                encode(
                    context
                        .host
                        .ok_or_else(crate::session::failed)?
                        .logout_account(&request.account_id)
                        .await
                        .map_err(command_error)?,
                )
            }
            "get_account_display" => encode(
                context
                    .host
                    .ok_or_else(crate::session::failed)?
                    .account_display()
                    .await
                    .map_err(command_error)?,
            ),
            "get_desktop_startup_status" => encode(session.status()),
            "get_app_consent_status" => {
                encode(kukuri_desktop_runtime::app_consent_status(&session.consent_db_path()).await)
            }
            "accept_app_consents" => {
                let request: ConsentInput = decode(payload)?;
                encode(
                    session
                        .accept_consents(request.documents, request.language, request.age_attested)
                        .await?,
                )
            }
            "list_accounts" => encode(
                kukuri_desktop_runtime::list_accounts(&session.app_data_dir)
                    .await
                    .map_err(command_error)?,
            ),
            "switch_account" => {
                let request: SwitchAccountRequest = decode(payload)?;
                encode(
                    context
                        .host
                        .ok_or_else(crate::session::failed)?
                        .switch_account(&request.account_id)
                        .await
                        .map_err(command_error)?,
                )
            }
            "export_account_key" => {
                let runtime = runtime(&context)?;
                let passphrase = secret_text(secret)?;
                let exported = tokio::task::spawn_blocking(move || {
                    runtime.export_account_key(ExportAccountKeyRequest { passphrase })
                })
                .await
                .map_err(|_| crate::session::failed())?
                .map_err(command_error)?;
                Ok(CommandOutput::Secret {
                    data: json!({"public_key": exported.public_key}),
                    secret: SecretOutput::new(exported.export.into_bytes()),
                })
            }
            "preview_account_key_import" => encode(
                kukuri_desktop_runtime::preview_account_key_import(
                    &session.app_data_dir,
                    &secret_text(secret)?,
                )
                .await
                .map_err(command_error)?,
            ),
            "import_account_key" => {
                let request: ImportInput = decode(payload)?;
                let secret: KeySecret =
                    serde_json::from_str(&secret_text(secret)?).map_err(|_| {
                        ProtocolError::new(
                            error_code::VALIDATION_FAILED,
                            "Secret入力にはexportとpassphraseのJSONを指定してください",
                        )
                    })?;
                encode(
                    context
                        .host
                        .ok_or_else(crate::session::failed)?
                        .import_account_key(secret.export, secret.passphrase, request.label)
                        .await
                        .map_err(command_error)?,
                )
            }
            "cancel_device_backup" => {
                session.operation.cancel_device_backup();
                encode(())
            }
            "create_device_backup_command"
            | "preview_device_backup_command"
            | "restore_device_backup_command" => {
                let request: BackupInput = decode(payload)?;
                if !Path::new(&request.path).is_absolute() {
                    return Err(ProtocolError::new(
                        error_code::VALIDATION_FAILED,
                        "backupには絶対pathを指定してください",
                    ));
                }
                let passphrase = secret_text(secret)?;
                match self.name {
                    "create_device_backup_command" => encode(
                        session
                            .create_backup(CreateDeviceBackupRequest {
                                path: request.path,
                                passphrase,
                                frontend_state: Default::default(),
                            })
                            .await?,
                    ),
                    "preview_device_backup_command" => {
                        let dir = session.app_data_dir.clone();
                        encode(
                            tokio::task::spawn_blocking(move || {
                                kukuri_desktop_runtime::preview_device_backup(
                                    &dir,
                                    &PreviewDeviceBackupRequest {
                                        path: request.path,
                                        passphrase,
                                    },
                                )
                            })
                            .await
                            .map_err(|_| crate::session::failed())?
                            .map_err(command_error)?,
                        )
                    }
                    _ => encode(
                        session
                            .restore_backup(RestoreDeviceBackupRequest {
                                path: request.path,
                                passphrase,
                                replace_existing: request.replace_existing,
                                apply_frontend_state: false,
                            })
                            .await?,
                    ),
                }
            }
            _ => unreachable!("登録済みlifecycle command"),
        }
    }
}

pub(super) fn registrations(session: Option<Arc<ClientSession>>) -> Vec<CommandRegistration> {
    use CommandEffect::{Destructive, Read, Write};
    [
        ("get_desktop_startup_status", Read, false, false, false),
        ("get_app_consent_status", Read, false, false, false),
        ("accept_app_consents", Write, false, false, false),
        ("export_account_key", Read, true, true, true),
        ("preview_account_key_import", Read, true, false, true),
        ("import_account_key", Write, true, false, true),
        ("list_accounts", Read, false, false, true),
        ("switch_account", Write, false, false, true),
        ("create_account", Write, false, false, true),
        ("logout_account", Write, false, false, true),
        ("get_account_display", Read, false, false, true),
        ("create_device_backup_command", Write, true, false, true),
        ("preview_device_backup_command", Read, true, false, true),
        (
            "restore_device_backup_command",
            Destructive,
            true,
            false,
            true,
        ),
        ("cancel_device_backup", Write, false, false, false),
    ]
    .into_iter()
    .map(|(name, effect, input, output, ready)| {
        let mut schema = lifecycle_schema::input(name);
        if input {
            schema["description"] = json!(match name {
                "import_account_key" =>
                    "Secret frame: exportとpassphraseを含むJSON。labelは通常payload。",
                "preview_account_key_import" => "Secret frame: 暗号化された鍵exportのUTF-8文字列。",
                _ => "Secret frame: passphraseのUTF-8文字列。改行もpassphraseの一部として扱う。",
            });
        }
        command(
            name,
            effect,
            input,
            output,
            if ready { host_guards() } else { vec![] },
            (schema, lifecycle_schema::output(name)),
            Arc::new(Handler {
                name,
                session: session.clone(),
            }),
        )
    })
    .collect()
}

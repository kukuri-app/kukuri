//! ゲームの開始で作る game player（ADR 0070 §4.1、#1996）。profile ごとに 1 件、`game-players.json` に保存し、network へは
//! 出さない。保存（`start_game_player`）は Write の command として dispatcher が直列に実行し、書込みは temp からの rename
//! なので、読取りには無いか完全な file だけが見える。

use std::path::Path;

use kukuri_desktop_runtime::{ClientHost, list_accounts, write_private_file_atomically};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::command_error;
use crate::protocol::{ProtocolError, error_code};

const FILE_NAME: &str = "game-players.json";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u32,
    application_id: String,
    game_player_id: String,
    account_id: String,
    public_key: String,
}

#[derive(Serialize)]
pub(super) struct GamePlayerView {
    game_player_id: String,
    public_key: String,
}

fn rejected(reason: &str, message: &str) -> ProtocolError {
    ProtocolError::new(error_code::CONFLICT, message).with_details(json!({"reason": reason}))
}

async fn read(dir: &Path) -> Result<Option<Binding>, ProtocolError> {
    match tokio::fs::read(dir.join(FILE_NAME)).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| crate::session::failed()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(crate::session::failed()),
    }
}

async fn write(dir: &Path, binding: &Binding) -> Result<(), ProtocolError> {
    let path = dir.join(FILE_NAME);
    let bytes = serde_json::to_vec_pretty(binding).map_err(|_| crate::session::failed())?;
    tokio::task::spawn_blocking(move || write_private_file_atomically(&path, &bytes))
        .await
        .map_err(|_| crate::session::failed())?
        .map_err(|_| crate::session::failed())
}

/// 保存済みの game player を、別のゲームのものなら断って返す。
async fn bound(dir: &Path, application_id: &str) -> Result<Option<Binding>, ProtocolError> {
    match read(dir).await? {
        Some(binding) if binding.application_id != application_id => Err(rejected(
            "game_player_application_mismatch",
            "この profile は別のゲームの game player を持っています",
        )),
        binding => Ok(binding),
    }
}

fn view(binding: Binding) -> GamePlayerView {
    GamePlayerView {
        game_player_id: binding.game_player_id,
        public_key: binding.public_key,
    }
}

pub(super) async fn get(
    dir: &Path,
    application_id: &str,
) -> Result<Option<GamePlayerView>, ProtocolError> {
    Ok(bound(dir, application_id).await?.map(view))
}

/// game player を作る。保存済みなら、結び付けたアカウントを active にして同じものを返す。
pub(super) async fn start(
    dir: &Path,
    host: &ClientHost,
    application_id: &str,
) -> Result<GamePlayerView, ProtocolError> {
    let accounts = list_accounts(dir).await.map_err(command_error)?;
    if let Some(binding) = bound(dir, application_id).await? {
        if accounts.active_account_id != binding.account_id {
            host.switch_account(&binding.account_id)
                .await
                .map_err(command_error)?;
        }
        return Ok(view(binding));
    }
    let active = accounts
        .accounts
        .iter()
        .find(|account| account.id == accounts.active_account_id)
        .ok_or_else(crate::session::failed)?;
    let binding = Binding {
        version: VERSION,
        application_id: application_id.to_owned(),
        game_player_id: uuid::Uuid::new_v4().to_string(),
        account_id: active.id.clone(),
        public_key: active.pubkey.clone(),
    };
    write(dir, &binding).await?;
    Ok(view(binding))
}

/// game player がある profile では、結び付けたアカウント以外へ切り替えない（統合は #1998）。
pub(super) async fn check_switch(dir: &Path, account_id: &str) -> Result<(), ProtocolError> {
    match read(dir).await? {
        Some(binding) if binding.account_id != account_id => Err(rejected(
            "integration_required",
            "ゲームのプレイヤーに結び付いたアカウントから切り替えるには統合が必要です",
        )),
        _ => Ok(()),
    }
}

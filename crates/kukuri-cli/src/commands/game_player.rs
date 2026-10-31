//! ゲームの開始で作る game player（ADR 0070 §4.1、#1996）。profile ごとに 1 件、`game-players.json` に保存し、network へは
//! 出さない。保存（`start_game_player`）は Write の command として dispatcher が直列に実行し、書込みは temp からの rename
//! なので、読取りには無いか完全な file だけが見える。
//!
//! Steam Cloud への保存（§4.3、#1997）は、ゲームで作ったアカウント（同意の直後に `game-account-origin.json` に記録する）の
//! 鍵と game player を `game-account.json` に書く。同期はゲームの Steam Auto-Cloud が行い、SDK は Steam を扱わない。

use std::path::Path;

use kukuri_core::KukuriKeys;
use kukuri_desktop_runtime::{
    AccountsSnapshot, ClientHost, list_accounts, write_private_file_atomically,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;

use super::command_error;
use crate::protocol::{ProtocolError, error_code};

const FILE_NAME: &str = "game-players.json";
const CLOUD_FILE: &str = "game-account.json";
const ORIGIN_FILE: &str = "game-account-origin.json";
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

/// Steam Cloud で同期する file。秘密鍵を持つので Debug を作らない。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudAccount {
    version: u32,
    application_id: String,
    game_player_id: String,
    secret_key: String,
}

/// ゲームで作ったアカウント。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    version: u32,
    account_id: String,
}

#[derive(Serialize)]
pub(super) struct GamePlayerView {
    game_player_id: String,
    public_key: String,
}

fn rejected(reason: &str, message: &str) -> ProtocolError {
    ProtocolError::new(error_code::CONFLICT, message).with_details(json!({"reason": reason}))
}

async fn load(dir: &Path, name: &str) -> Result<Option<Vec<u8>>, ProtocolError> {
    match tokio::fs::read(dir.join(name)).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(crate::session::failed()),
    }
}

async fn read_file<T: DeserializeOwned>(
    dir: &Path,
    name: &str,
) -> Result<Option<T>, ProtocolError> {
    load(dir, name)
        .await?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| crate::session::failed()))
        .transpose()
}

async fn read(dir: &Path) -> Result<Option<Binding>, ProtocolError> {
    read_file(dir, FILE_NAME).await
}

async fn write(dir: &Path, binding: &Binding) -> Result<(), ProtocolError> {
    write_file(dir, FILE_NAME, binding).await
}

async fn write_file(dir: &Path, name: &str, value: &impl Serialize) -> Result<(), ProtocolError> {
    let path = dir.join(name);
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| crate::session::failed())?;
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
    if let Some(restored) = restore(dir, host, application_id, &accounts).await? {
        return Ok(restored);
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

/// 同意で最初のアカウントを作った直後に、ゲームで作ったアカウントとして記録する（同意の前にアカウントが無かったときだけ）。
pub(super) async fn record_origin(dir: &Path) -> Result<(), ProtocolError> {
    let accounts = list_accounts(dir).await.map_err(command_error)?.accounts;
    if let [account] = accounts.as_slice() {
        let origin = Origin {
            version: VERSION,
            account_id: account.id.clone(),
        };
        write_file(dir, ORIGIN_FILE, &origin).await?;
    }
    Ok(())
}

enum Cloud {
    Off,
    On,
    Restorable,
    OtherAccount,
    Unreadable,
}

impl Cloud {
    fn name(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Restorable => "restorable",
            Self::OtherAccount => "other_account",
            Self::Unreadable => "unreadable",
        }
    }
}

/// `game-account.json`。無ければ `None`、読めなければ（壊れている・知らない版・別のゲーム）`Some(None)`。
async fn cloud_file(
    dir: &Path,
    application_id: &str,
) -> Result<Option<Option<(KukuriKeys, CloudAccount)>>, ProtocolError> {
    Ok(load(dir, CLOUD_FILE).await?.map(|bytes| {
        serde_json::from_slice::<CloudAccount>(&bytes)
            .ok()
            .filter(|file| file.version == VERSION && file.application_id == application_id)
            .and_then(|file| Some((KukuriKeys::parse(&file.secret_key).ok()?, file)))
    }))
}

/// `game-account.json` を手元の結び付きと、鍵と game player ID の両方で比べる。
async fn cloud(dir: &Path, application_id: &str) -> Result<Cloud, ProtocolError> {
    let (keys, file) = match cloud_file(dir, application_id).await? {
        None => return Ok(Cloud::Off),
        Some(None) => return Ok(Cloud::Unreadable),
        Some(Some(readable)) => readable,
    };
    let public_key = keys.public_key_hex();
    Ok(match bound(dir, application_id).await? {
        None => Cloud::Restorable,
        Some(binding)
            if binding.public_key == public_key
                && binding.game_player_id == file.game_player_id =>
        {
            Cloud::On
        }
        Some(_) => Cloud::OtherAccount,
    })
}

pub(super) async fn get_cloud(
    dir: &Path,
    application_id: &str,
) -> Result<&'static str, ProtocolError> {
    Ok(cloud(dir, application_id).await?.name())
}

/// 新しい端末で Steam が戻した file から、ゲームで作ったアカウントと game player で再開する（§4.3）。利用者が受けた
/// アカウントを選んでいるとき（active がゲームで作ったアカウントでない）と、読めない file からは戻さない。足す・記録する・
/// 結び付ける・切り替えるの順で、途中で止まっても次の開始で同じ file から続ける（戻したアカウントを記録済みなら続けてよい）。
async fn restore(
    dir: &Path,
    host: &ClientHost,
    application_id: &str,
    accounts: &AccountsSnapshot,
) -> Result<Option<GamePlayerView>, ProtocolError> {
    let Some(Some((keys, file))) = cloud_file(dir, application_id).await? else {
        return Ok(None);
    };
    let public_key = keys.public_key_hex();
    let restoring = accounts
        .accounts
        .iter()
        .find(|account| account.pubkey == public_key)
        .map(|account| account.id.as_str());
    let origin: Option<Origin> = read_file(dir, ORIGIN_FILE).await?;
    if !origin.is_some_and(|origin| {
        origin.account_id == accounts.active_account_id
            || Some(origin.account_id.as_str()) == restoring
    }) {
        return Ok(None);
    }
    let record = host
        .register_transferred_account(&keys)
        .await
        .map_err(command_error)?;
    let origin = Origin {
        version: VERSION,
        account_id: record.id.clone(),
    };
    write_file(dir, ORIGIN_FILE, &origin).await?;
    let binding = Binding {
        version: VERSION,
        application_id: application_id.to_owned(),
        game_player_id: file.game_player_id,
        account_id: record.id.clone(),
        public_key,
    };
    write(dir, &binding).await?;
    host.switch_account(&record.id)
        .await
        .map_err(command_error)?;
    Ok(Some(view(binding)))
}

fn not_started() -> ProtocolError {
    rejected("game_player_required", "始める前は保存できません")
}

/// Steam Cloud への保存を有効にする（ゲームで作ったアカウントの鍵と game player を書く）・やめる（その file を消す）。
pub(super) async fn set_cloud(
    dir: &Path,
    host: &ClientHost,
    application_id: &str,
    enabled: bool,
) -> Result<&'static str, ProtocolError> {
    let state = cloud(dir, application_id).await?;
    match (state, enabled) {
        (Cloud::Unreadable, _) => {
            return Err(rejected(
                "cloud_account_unreadable",
                "Steam Cloud の file を読めません",
            ));
        }
        (Cloud::OtherAccount, _) => {
            return Err(rejected(
                "integration_required",
                "Steam Cloud には別の端末で保存したアカウントがあります",
            ));
        }
        (Cloud::Restorable, true) => return Err(not_started()),
        (Cloud::On, false) => tokio::fs::remove_file(dir.join(CLOUD_FILE))
            .await
            .map_err(|_| crate::session::failed())?,
        (Cloud::Off, true) => {
            let binding = bound(dir, application_id).await?.ok_or_else(not_started)?;
            let origin: Option<Origin> = read_file(dir, ORIGIN_FILE).await?;
            if origin.is_none_or(|origin| origin.account_id != binding.account_id) {
                return Err(rejected(
                    "not_game_account",
                    "このゲームで作ったアカウントだけを Steam Cloud に保存できます",
                ));
            }
            let runtime = host.runtime();
            if runtime.local_author_pubkey() != binding.public_key {
                return Err(crate::session::failed());
            }
            let file = CloudAccount {
                version: VERSION,
                application_id: application_id.to_owned(),
                game_player_id: binding.game_player_id,
                secret_key: runtime.local_author_secret_hex(),
            };
            write_file(dir, CLOUD_FILE, &file).await?;
        }
        _ => {}
    }
    get_cloud(dir, application_id).await
}

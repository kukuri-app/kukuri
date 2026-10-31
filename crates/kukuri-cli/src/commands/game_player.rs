//! ゲームの開始で作る game player（ADR 0070 §4.1、#1996）。profile ごとに 1 件、`game-players.json` に保存し、network へは
//! 出さない。保存（`start_game_player`）は Write の command として dispatcher が直列に実行し、書込みは temp からの rename
//! なので、読取りには無いか完全な file だけが見える。
//!
//! Steam Cloud への保存（§4.3、#1997）は、ゲームで作ったアカウント（同意の直後に `game-account-origin.json` に記録する）の
//! 鍵と game player を `game-account.json` に書く。同期はゲームの Steam Auto-Cloud が行い、SDK は Steam を扱わない。
//!
//! 既存のアカウントへのまとめ（§4.2、#1998 AC-3）は、結び付きを B へ付け替え、A のフォロー・ブロック・ミュートを背景で
//! B に付け直す。付け直しの進んだ位置は `game-player-merge.json` に置き、終えたら消す。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kukuri_core::KukuriKeys;
use kukuri_desktop_runtime::{
    AccountsSnapshot, ClientHost, SocialCopyKind, SocialCopySource, account_db_path, list_accounts,
    write_private_file_atomically,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;

use super::command_error;
use crate::protocol::{ProtocolError, error_code};

const FILE_NAME: &str = "game-players.json";
const CLOUD_FILE: &str = "game-account.json";
const ORIGIN_FILE: &str = "game-account-origin.json";
const MERGE_FILE: &str = "game-player-merge.json";
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

/// 付け直しの途中。A と B の account ID と、進んだ位置（種類と、最後に読んだ相手）。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Merge {
    version: u32,
    from_account_id: String,
    to_account_id: String,
    kind: SocialCopyKind,
    after: Option<String>,
}

#[derive(Serialize)]
pub(super) struct GamePlayerView {
    game_player_id: String,
    public_key: String,
    copying: bool,
}

/// 付け直しの背景 task。profile ごとに 1 つで、動いている間は新しく始めない。
#[derive(Default)]
pub(crate) struct CopyTask(std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>);

impl CopyTask {
    fn ensure(&self, dir: &Path, host: &Arc<ClientHost>) {
        let mut task = self.0.lock().expect("copy task lock");
        if task.as_ref().is_none_or(|task| task.is_finished()) {
            *task = Some(tokio::spawn(copy(dir.to_owned(), host.clone())));
        }
    }

    pub(crate) fn abort(&self) {
        if let Some(task) = self.0.lock().expect("copy task lock").take() {
            task.abort();
        }
    }
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

fn view(binding: Binding, copying: bool) -> GamePlayerView {
    GamePlayerView {
        game_player_id: binding.game_player_id,
        public_key: binding.public_key,
        copying,
    }
}

/// 結び付きの B への付け直しの途中か。付け直しの file の B が結び付きと違えば、確定の前に止まった残りである。
async fn copying(dir: &Path, binding: &Binding) -> Result<bool, ProtocolError> {
    Ok(read_file::<Merge>(dir, MERGE_FILE)
        .await?
        .is_some_and(|merge| merge.to_account_id == binding.account_id))
}

pub(super) async fn get(
    dir: &Path,
    application_id: &str,
) -> Result<Option<GamePlayerView>, ProtocolError> {
    let Some(binding) = bound(dir, application_id).await? else {
        return Ok(None);
    };
    let copying = copying(dir, &binding).await?;
    Ok(Some(view(binding, copying)))
}

/// game player を作る。保存済みなら、結び付けたアカウントを active にして同じものを返し、付け直しの途中なら続きを進める。
pub(super) async fn start(
    dir: &Path,
    host: &Arc<ClientHost>,
    task: &CopyTask,
    application_id: &str,
) -> Result<GamePlayerView, ProtocolError> {
    let accounts = list_accounts(dir).await.map_err(command_error)?;
    if let Some(binding) = bound(dir, application_id).await? {
        if accounts.active_account_id != binding.account_id {
            host.switch_account(&binding.account_id)
                .await
                .map_err(command_error)?;
        }
        let copying = copying(dir, &binding).await?;
        if copying {
            task.ensure(dir, host);
        } else {
            remove(dir, MERGE_FILE).await?;
        }
        return Ok(view(binding, copying));
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
    Ok(view(binding, false))
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
    Ok(Some(view(binding, false)))
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
        (Cloud::On, false) => remove(dir, CLOUD_FILE).await?,
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

async fn remove(dir: &Path, name: &str) -> Result<(), ProtocolError> {
    match tokio::fs::remove_file(dir.join(name)).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(crate::session::failed()),
        _ => Ok(()),
    }
}

fn invalid(message: &str) -> ProtocolError {
    ProtocolError::new(error_code::VALIDATION_FAILED, message)
}

/// ゲームの結び付きを、profile にある別のアカウント B へ付け替えて B に切り替え、A のフォロー・ブロック・ミュートの
/// 付け直しを背景で始める（§4.2）。残す game player ID はゲームが渡す（既定は今の ID）。確定は結び付きの書き換えで決まる。
pub(super) async fn merge(
    dir: &Path,
    host: &Arc<ClientHost>,
    task: &CopyTask,
    application_id: &str,
    account_id: &str,
    game_player_id: Option<&str>,
) -> Result<GamePlayerView, ProtocolError> {
    let binding = bound(dir, application_id)
        .await?
        .ok_or_else(|| rejected("game_player_required", "始める前はまとめられません"))?;
    if copying(dir, &binding).await? {
        return Err(rejected(
            "merge_in_progress",
            "前のまとめの付け直しが終わっていません",
        ));
    }
    let game_player_id = match game_player_id {
        Some(id) => uuid::Uuid::parse_str(id)
            .map_err(|_| invalid("game player ID は UUID で指定してください"))?
            .to_string(),
        None => binding.game_player_id.clone(),
    };
    let target = list_accounts(dir)
        .await
        .map_err(command_error)?
        .accounts
        .into_iter()
        .find(|account| account.id == account_id && account.id != binding.account_id)
        .ok_or_else(|| invalid("まとめる先のアカウントがこの profile にありません"))?;
    // まとめた先の鍵は Steam に置かない（§4.3）。
    if matches!(cloud(dir, application_id).await?, Cloud::On) {
        remove(dir, CLOUD_FILE).await?;
    }
    let merge = Merge {
        version: VERSION,
        from_account_id: binding.account_id,
        to_account_id: target.id.clone(),
        kind: SocialCopyKind::Follows,
        after: None,
    };
    write_file(dir, MERGE_FILE, &merge).await?;
    let next = Binding {
        version: VERSION,
        application_id: application_id.to_owned(),
        game_player_id,
        account_id: target.id,
        public_key: target.pubkey,
    };
    if let Err(error) = write(dir, &next).await {
        remove(dir, MERGE_FILE).await?;
        return Err(error);
    }
    host.switch_account(&next.account_id)
        .await
        .map_err(command_error)?;
    task.ensure(dir, host);
    Ok(view(next, true))
}

async fn copy(dir: PathBuf, host: Arc<ClientHost>) {
    // 失敗は位置を進めずに止め、次の `start_game_player` で同じ位置から続ける。
    let _ = copy_social(&dir, &host).await;
}

/// B の移行の反映が終わってから、A の DB を種類ごと・相手の順に 1 page ずつ付け直し、page ごとに位置を保存する。
async fn copy_social(dir: &Path, host: &ClientHost) -> Result<(), ProtocolError> {
    let lost = |_| crate::session::failed();
    let Some(mut merge) = read_file::<Merge>(dir, MERGE_FILE).await? else {
        return Ok(());
    };
    let runtime = host.runtime();
    while !runtime.account_transfer_merged().await.map_err(lost)? {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let accounts = list_accounts(dir).await.map_err(command_error)?;
    let from = accounts
        .accounts
        .iter()
        .find(|account| account.id == merge.from_account_id)
        .ok_or_else(crate::session::failed)?;
    let source = SocialCopySource::open(&account_db_path(dir, &from.id), &from.pubkey)
        .await
        .map_err(lost)?;
    loop {
        match runtime
            .copy_social_page(&source, merge.kind, merge.after.as_deref())
            .await
            .map_err(lost)?
        {
            Some(last) => merge.after = Some(last),
            None => match merge.kind.next() {
                Some(kind) => {
                    merge.kind = kind;
                    merge.after = None;
                }
                None => return remove(dir, MERGE_FILE).await,
            },
        }
        write_file(dir, MERGE_FILE, &merge).await?;
    }
}

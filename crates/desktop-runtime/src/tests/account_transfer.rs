//! #1211 AC-1（1c）: 移行の接続と確認を runtime の command から通す。誤ったリンクでは確認に進まない。
//! AC-2（2c・2d）: 確認の後に必須 bundle を送り、移行先はアカウントの置き場へ保存して確定で登録する。反映は受けた
//! アカウントの runtime の起動時に行い、受信途中・反映途中の再起動、同じ移行のやり直し、手元の新しい版を扱う。
//! AC-3 の履歴は `account_transfer_history.rs`。

use super::*;
use crate::accounts::transfer::TransferSink;
use crate::accounts::{account_db_path, ensure_accounts_initialized, list_accounts};
use crate::requests::{DecideAccountTransferRequest, OpenAccountTransferRequest};
use kukuri_core::{
    AccountSyncItem, AccountSyncItemKey, AccountTransferFailure, AccountTransferHistory,
    AccountTransferItem, AccountTransferStatus, KukuriKeys, Pubkey,
};
use kukuri_iroh_node::AccountBundleSink;

const MODE: IdentityStorageMode = IdentityStorageMode::FileOnly;

pub(super) async fn wait_for(
    runtime: &DesktopRuntime,
    done: impl Fn(&AccountTransferStatus) -> bool,
) {
    timeout(Duration::from_secs(20), async {
        while !done(&runtime.account_transfer_status().await.unwrap()) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("account transfer status did not settle");
}

pub(super) async fn runtime_at(db: impl AsRef<Path>) -> DesktopRuntime {
    DesktopRuntime::new_with_config_and_identity(db, TransportNetworkConfig::loopback(), MODE)
        .await
        .unwrap()
}

/// 移行先: アカウントの layout を持つ端末の host（最初のアカウントを使っている）。
pub(super) async fn target_host(dir: &Path) -> Arc<ClientHost> {
    let db = ensure_accounts_initialized(dir, MODE).await.unwrap();
    ClientHost::from_runtime(dir.to_path_buf(), Arc::new(runtime_at(&db).await))
        .await
        .unwrap()
}

/// 移行元の招待で移行先の host が接続し、両端末で承認して、両方の完了を待つ。受けたアカウントの ID を返す。
pub(super) async fn transfer(source: &DesktopRuntime, host: &Arc<ClientHost>) -> String {
    match transfer_with(source, host, None).await {
        AccountTransferStatus::Completed {
            account_id: Some(id),
            ..
        } => id,
        status => panic!("unexpected {status:?}"),
    }
}

/// `history` の範囲の履歴（AC-3）を選んで移行し、両方の完了を待つ。移行先の完了の状態を返す。
pub(super) async fn transfer_with(
    source: &DesktopRuntime,
    host: &Arc<ClientHost>,
    history: Option<AccountTransferHistory>,
) -> AccountTransferStatus {
    let link = source.create_account_transfer_invite().await.unwrap().link;
    host.open_account_transfer(OpenAccountTransferRequest { link, history })
        .await
        .unwrap();
    let target = host.runtime();
    for runtime in [source, &*target] {
        wait_for(runtime, |status| {
            matches!(status, AccountTransferStatus::Confirming { .. })
        })
        .await;
        runtime
            .decide_account_transfer(DecideAccountTransferRequest { accept: true })
            .await
            .unwrap();
    }
    let completed =
        |status: &AccountTransferStatus| matches!(status, AccountTransferStatus::Completed { .. });
    wait_for(&target, completed).await;
    wait_for(source, completed).await;
    target.account_transfer_status().await.unwrap()
}

fn staging_files(db: &Path) -> Vec<String> {
    let name = db.file_stem().unwrap().to_string_lossy().to_string();
    let mut files: Vec<_> = fs::read_dir(db.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|file| file.starts_with(&format!("{name}.account-transfer")))
        .collect();
    files.sort();
    files
}

pub(super) async fn eventually(what: &str, mut done: impl AsyncFnMut() -> bool) {
    timeout(Duration::from_secs(20), async {
        while !done().await {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

#[tokio::test]
async fn account_transfer_commands_reject_a_broken_link() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let source = runtime_at(dir.path().join("source.db")).await;
    let host = target_host(&dir.path().join("target")).await;
    let link = source.create_account_transfer_invite().await.unwrap().link;
    let truncated = OpenAccountTransferRequest {
        link: link[..link.len() - 8].to_string(),
        history: None,
    };
    assert!(host.open_account_transfer(truncated).await.is_err());
    assert_eq!(
        host.runtime().account_transfer_status().await.unwrap(),
        AccountTransferStatus::Idle
    );
    source.cancel_account_transfer().await.unwrap();
    assert_eq!(
        source.account_transfer_status().await.unwrap(),
        AccountTransferStatus::Idle
    );
    source.shutdown().await;
    host.shutdown().await;
}

/// #1211 AC-5: 移行元のアカウント（登録簿と鍵）は、移行の後も残る。同じ DB から起動し直しても、同じ公開鍵で使える。
#[tokio::test]
async fn the_source_keeps_its_account_after_the_transfer() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let source_dir = dir.path().join("source");
    let source_host = target_host(&source_dir).await;
    let pubkey = source_host.runtime().local_author_pubkey();
    let target = target_host(&dir.path().join("target")).await;
    transfer(&source_host.runtime(), &target).await;
    let accounts = list_accounts(&source_dir).await.unwrap();
    let kept = accounts
        .accounts
        .iter()
        .find(|record| record.pubkey == pubkey)
        .expect("the source account stays registered");
    assert_eq!(accounts.active_account_id, kept.id);
    let db = account_db_path(&source_dir, &kept.id);
    source_host.shutdown().await;
    assert_eq!(runtime_at(&db).await.local_author_pubkey(), pubkey);
    target.shutdown().await;
}

/// 2c・2d: 新規のアカウントは保存の確定で 1 件だけ登録され（やり直しても増えない）、使っているアカウントには何も
/// 反映しない。受けたアカウントの runtime の起動で、表示例外・profile・参加中の channel が反映され、置き場は消える。
#[tokio::test]
async fn a_transferred_account_is_registered_once_and_merged_when_it_starts() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let source = runtime_at(dir.path().join("source.db")).await;
    let author = KukuriKeys::generate().public_key_hex();
    source
        .app_service
        .set_trust_always_visible(&author, true)
        .await
        .unwrap();
    source
        .set_my_profile(SetMyProfileRequest {
            name: Some("moved".into()),
            display_name: None,
            about: None,
            picture_upload: None,
            clear_picture: false,
            nip05: None,
        })
        .await
        .unwrap();
    let topic = "kukuri:topic:account-transfer-runtime";
    let channel = source
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "moved".into(),
            audience_kind: Default::default(),
        })
        .await
        .unwrap()
        .channel_id;
    let target_dir = dir.path().join("target");
    let host = target_host(&target_dir).await;

    let id = transfer(&source, &host).await;
    let pubkey = source.local_author_pubkey();
    let registered = || async {
        list_accounts(&target_dir)
            .await
            .unwrap()
            .accounts
            .into_iter()
            .filter(|record| record.pubkey == pubkey)
            .count()
    };
    assert_eq!(registered().await, 1);
    // 同じ移行のやり直しで、二重に登録しない。
    assert_eq!(transfer(&source, &host).await, id);
    assert_eq!(registered().await, 1);
    // 使っているアカウント（別の公開鍵）には何も反映しない。
    let active = host.runtime();
    assert!(
        active
            .app_service
            .list_trust_always_visible()
            .await
            .unwrap()
            .is_empty()
    );
    let db = account_db_path(&target_dir, &id);
    assert!(!staging_files(&db).is_empty());

    // 受けたアカウントの runtime の起動で反映する。
    let moved = runtime_at(&db).await;
    eventually("the bundle is merged", async || {
        moved.app_service.list_trust_always_visible().await.unwrap() == vec![author.clone()]
            && moved.get_my_profile().await.unwrap().name.as_deref() == Some("moved")
            && moved
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.into(),
                    cursor: None,
                })
                .await
                .unwrap()
                .items
                .iter()
                .any(|joined| joined.channel_id == channel)
    })
    .await;
    eventually("the staging is removed", async || {
        staging_files(&db).is_empty()
    })
    .await;
    moved.shutdown().await;
    source.shutdown().await;
    host.shutdown().await;
}

fn bundle_item(
    keys: &KukuriKeys,
    author: &str,
    updated_at: i64,
    visible: bool,
) -> AccountTransferItem {
    let item = AccountSyncItem::edit(
        AccountSyncItemKey::TrustAlwaysVisible {
            author: Pubkey::from(author),
        },
        updated_at,
        visible.then_some(serde_json::Value::Bool(true)),
    );
    AccountTransferItem {
        key: item.key.docs_key(),
        sealed: keys
            .derive_account_sync()
            .seal(&keys.public_key(), &item)
            .unwrap(),
    }
}

/// 2c・2d（保存先の境界の故障）: 別のアカウントの item は保存を確定せず消す。取消・停止で止めた受信の置き場は消す。
/// 再起動などで残った受信途中の置き場は反映せず、起動か次の受信で消す。反映の途中で止まった置き場は、続きの chunk
/// から反映する。手元の新しい版を古い bundle で戻さない。
#[tokio::test]
async fn staged_bundles_resume_and_never_roll_back_newer_versions() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let host = target_host(dir.path()).await;
    let sink = TransferSink {
        host: Arc::downgrade(&host),
    };
    let keys = KukuriKeys::generate();
    let secret = keys.export_secret_hex();
    let db = account_db_path(dir.path(), &account_id_for(&keys));
    let authors: Vec<_> = (0..3)
        .map(|_| KukuriKeys::generate().public_key_hex())
        .collect();

    // 別のアカウントの封は検証で拒否し、確定せずに消す。
    let mut staging = sink.begin(&secret).await.unwrap();
    staging
        .stage(vec![bundle_item(&keys, &authors[0], 1, true)])
        .await
        .unwrap();
    let foreign = bundle_item(&KukuriKeys::generate(), &authors[1], 1, true);
    assert_eq!(
        staging.stage(vec![foreign]).await,
        Err(AccountTransferFailure::Invalid)
    );
    staging.abort().await;
    assert!(staging_files(&db).is_empty());

    // 取消・停止で移行の task ごと止めた（確定も破棄もせずに落とした）置き場も消す。
    let mut staging = sink.begin(&secret).await.unwrap();
    staging
        .stage(vec![bundle_item(&keys, &authors[0], 1, true)])
        .await
        .unwrap();
    drop(staging);
    eventually("the cancelled staging is removed", async || {
        staging_files(&db).is_empty()
    })
    .await;

    // 再起動などで残った受信途中の置き場（ここでは片付けずに残す）は反映せず、そのアカウントの起動か次の受信が消す。
    let other = KukuriKeys::generate();
    let other_db = account_db_path(dir.path(), &account_id_for(&other));
    let mut staging = sink.begin(&other.export_secret_hex()).await.unwrap();
    staging
        .stage(vec![bundle_item(&other, &authors[0], 1, true)])
        .await
        .unwrap();
    std::mem::forget(staging);
    assert_eq!(staging_files(&other_db).len(), 2);
    let started = runtime_at(&other_db).await;
    eventually("the unfinished staging is removed at start", async || {
        staging_files(&other_db).is_empty()
    })
    .await;
    assert!(
        started
            .app_service
            .list_trust_always_visible()
            .await
            .unwrap()
            .is_empty()
    );
    started.shutdown().await;
    let mut staging = sink.begin(&secret).await.unwrap();
    staging
        .stage(vec![bundle_item(&keys, &authors[0], 1, true)])
        .await
        .unwrap();
    std::mem::forget(staging);
    assert_eq!(staging_files(&db).len(), 2);
    let mut staging = sink.begin(&secret).await.unwrap();
    assert_eq!(staging_files(&db).len(), 1, "only the new manifest");
    for author in &authors {
        staging
            .stage(vec![bundle_item(&keys, author, 1, true)])
            .await
            .unwrap();
    }
    let id = staging.commit().await.unwrap();
    assert_eq!(
        list_accounts(dir.path())
            .await
            .unwrap()
            .accounts
            .iter()
            .filter(|record| record.id == id)
            .count(),
        1
    );

    // 1 つ目の chunk を反映して消した後に止まった（manifest は進む前）とする。
    fs::remove_file(db.with_extension("account-transfer-0.json")).unwrap();
    let moved = runtime_at(&db).await;
    eventually("the remaining chunks are merged", async || {
        staging_files(&db).is_empty()
    })
    .await;
    assert_eq!(
        moved.app_service.list_trust_always_visible().await.unwrap(),
        {
            let mut rest = authors[1..].to_vec();
            rest.sort();
            rest
        }
    );

    // 手元で後から解除した版は、古い bundle で戻さない。bundle だけにある版は採る。
    moved
        .app_service
        .set_trust_always_visible(&authors[1], false)
        .await
        .unwrap();
    moved.shutdown().await;
    let mut staging = sink.begin(&secret).await.unwrap();
    staging
        .stage(vec![
            bundle_item(&keys, &authors[0], 1, true),
            bundle_item(&keys, &authors[1], 1, true),
        ])
        .await
        .unwrap();
    assert_eq!(staging.commit().await.unwrap(), id);
    let moved = runtime_at(&db).await;
    eventually("the second bundle is merged", async || {
        staging_files(&db).is_empty()
    })
    .await;
    let mut expected = vec![authors[0].clone(), authors[2].clone()];
    expected.sort();
    assert_eq!(
        moved.app_service.list_trust_always_visible().await.unwrap(),
        expected
    );
    moved.shutdown().await;
    host.shutdown().await;
}

pub(super) fn account_id_for(keys: &KukuriKeys) -> String {
    crate::accounts::account_id_for_pubkey(&keys.public_key_hex()).unwrap()
}

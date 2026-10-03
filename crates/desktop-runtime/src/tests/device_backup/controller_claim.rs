//! #1219 AC-4: backup から復元した端末は、その account の最初の起動で自分の channel の鍵更新を引き取る（ADR 0018 §8）。

use super::*;

use kukuri_app_api::PrivateChannelController;
use kukuri_store::{PrivateChannelFilter, PrivateChannelKeyStore};

use crate::runtime::controller_claim_marker;
use crate::tests::protected_migration::{TOPIC, backup_and_restore, open_runtime};

async fn controllers(runtime: &DesktopRuntime) -> Vec<PrivateChannelController> {
    let owner = runtime.author_keys.public_key_hex();
    runtime
        .sqlite
        .list_joined_private_channels(PrivateChannelFilter::Owner(&owner), "", 8)
        .await
        .expect("channels")
        .into_iter()
        .map(|row| {
            serde_json::from_str(row.controller.as_deref().expect("record")).expect("controller")
        })
        .collect()
}

async fn device_id(runtime: &DesktopRuntime) -> String {
    runtime
        .get_sync_status()
        .await
        .expect("status")
        .discovery
        .local_endpoint_id
}

/// 復元の準備が印を置き、最初の起動の背景 task が自分の channel をすべて次の世代で引き取って、印を消す。1 件目を
/// 引き取った後に止まった起動（印は残る）の次の起動でも、残りを引き取り、引き取り済みの channel の世代は進めない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_account_takes_over_its_channels_at_the_first_start() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let source = tempdir().expect("source dir");
    let db = ensure_accounts_initialized(source.path(), IdentityStorageMode::FileOnly)
        .await
        .expect("source account");
    let runtime = open_runtime(&db).await;
    for label in ["first", "second"] {
        runtime
            .create_private_channel(CreatePrivateChannelRequest {
                topic: TOPIC.into(),
                label: label.into(),
                audience_kind: ChannelAudienceKind::InviteOnly,
            })
            .await
            .expect("private channel");
    }
    runtime
        .finish_protected_migration()
        .await
        .expect("finish migration");
    let source_device = device_id(&runtime).await;
    runtime.shutdown().await;
    drop(runtime);

    let target = tempdir().expect("target dir");
    let (_, restored_db) = backup_and_restore(source.path(), &db, target.path()).await;
    let marker = controller_claim_marker(&restored_db);
    assert!(marker.is_file(), "the restore marks the account");
    // 1 件目を引き取った後に止まった起動。印を外して起動の背景の引取りを止め、1 件だけ引き取る。
    std::fs::remove_file(&marker).expect("hold the claim");
    let stopped = open_runtime(&restored_db).await;
    let device = device_id(&stopped).await;
    assert_ne!(device, source_device, "the restored device has its own ID");
    stopped
        .app_service
        .claim_private_channel_controllers("", 1)
        .await
        .expect("first page");
    stopped.shutdown().await;
    drop(stopped);
    std::fs::write(&marker, []).expect("the marker stays");

    let restored = open_runtime(&restored_db).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while marker.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the first start takes over and removes the marker");
    let taken = PrivateChannelController {
        device_id: device,
        generation: 2,
        transfer_to: None,
    };
    assert_eq!(controllers(&restored).await, vec![taken.clone(), taken]);
    restored.shutdown().await;
}

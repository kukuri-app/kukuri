//! #1211 AC-1（1c）: 移行の接続と確認を runtime の command から通す。誤ったリンクでは確認済みにならず、
//! 確認済みになっても account の鍵は変わらない（転送は AC-2）。

use super::*;
use crate::requests::{DecideAccountTransferRequest, OpenAccountTransferRequest};
use kukuri_core::AccountTransferStatus;

async fn wait_for(runtime: &DesktopRuntime, done: impl Fn(&AccountTransferStatus) -> bool) {
    timeout(Duration::from_secs(20), async {
        while !done(&runtime.account_transfer_status().await.unwrap()) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("account transfer status did not settle");
}

#[tokio::test]
async fn account_transfer_commands_confirm_without_changing_accounts() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let runtime = async |name: &str| {
        DesktopRuntime::new_with_config_and_identity(
            dir.path().join(format!("{name}.db")),
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
        .await
        .unwrap()
    };
    let source = runtime("source").await;
    let target = runtime("target").await;
    let accounts = (source.local_author_pubkey(), target.local_author_pubkey());

    let link = source.create_account_transfer_invite().await.unwrap().link;
    let truncated = OpenAccountTransferRequest {
        link: link[..link.len() - 8].to_string(),
    };
    assert!(target.open_account_transfer(truncated).await.is_err());
    assert_eq!(
        target.account_transfer_status().await.unwrap(),
        AccountTransferStatus::Idle
    );

    target
        .open_account_transfer(OpenAccountTransferRequest { link })
        .await
        .unwrap();
    for runtime in [&source, &target] {
        wait_for(runtime, |status| {
            matches!(status, AccountTransferStatus::Confirming { .. })
        })
        .await;
        runtime
            .decide_account_transfer(DecideAccountTransferRequest { accept: true })
            .await
            .unwrap();
    }
    for runtime in [&source, &target] {
        wait_for(runtime, |status| {
            matches!(status, AccountTransferStatus::Confirmed { .. })
        })
        .await;
    }
    assert_eq!(
        (source.local_author_pubkey(), target.local_author_pubkey()),
        accounts
    );
    source.cancel_account_transfer().await.unwrap();
    assert_eq!(
        source.account_transfer_status().await.unwrap(),
        AccountTransferStatus::Idle
    );
    source.shutdown().await;
    target.shutdown().await;
}

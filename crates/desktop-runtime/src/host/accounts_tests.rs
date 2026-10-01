use super::*;
use crate::accounts::{add_account, ensure_accounts_initialized};
use crate::identity::IdentityStorageMode;

async fn fixture(dir: &Path) -> (Arc<ClientHost>, Arc<DesktopRuntime>, AccountRecord) {
    let mode = IdentityStorageMode::FileOnly;
    let db = ensure_accounts_initialized(dir, mode).await.unwrap();
    let a = Arc::new(
        DesktopRuntime::new_with_config_and_identity(
            &db,
            kukuri_transport::TransportNetworkConfig::loopback(),
            mode,
        )
        .await
        .unwrap(),
    );
    let host = ClientHost::from_runtime(dir.to_path_buf(), a.clone())
        .await
        .unwrap();
    let b = add_account(dir, mode, &kukuri_core::KukuriKeys::generate(), None, false)
        .await
        .unwrap();
    (host, a, b)
}

async fn install_next(host: &ClientHost, b: &AccountRecord) -> Arc<DesktopRuntime> {
    let runtime = Arc::new(
        DesktopRuntime::new_with_config_and_identity(
            account_db_path(&host.app_data_dir, &b.id),
            kukuri_transport::TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
        .await
        .unwrap(),
    );
    host.replace_runtime(runtime.clone()).await.unwrap();
    runtime
}

#[tokio::test]
async fn failure_before_registry_commit_restores_runtime_without_polluting_history() {
    let dir = tempfile::tempdir().unwrap();
    let (host, a, b) = fixture(dir.path()).await;
    let original_id = list_accounts(dir.path()).await.unwrap().active_account_id;
    let original_registry = std::fs::read(dir.path().join("accounts.json")).unwrap();
    let next = install_next(&host, &b).await;
    assert!(
        host.finish_account_change(
            a.clone(),
            &original_id,
            b,
            false,
            Err(anyhow::anyhow!("injected pre-rename failure"))
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(dir.path().join("accounts.json")).unwrap(),
        original_registry
    );
    assert!(Arc::ptr_eq(&host.runtime(), &a));
    assert!(next.iroh_stack.current.lock().await.is_none());
    host.shutdown().await;
}

#[tokio::test]
async fn failure_after_registry_commit_never_reactivates_the_old_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let (host, a, b) = fixture(dir.path()).await;
    let original_id = list_accounts(dir.path()).await.unwrap().active_account_id;
    let next = install_next(&host, &b).await;
    set_active_account(dir.path(), &b.id).await.unwrap();
    let result = host
        .finish_account_change(
            a.clone(),
            &original_id,
            b.clone(),
            false,
            Err(anyhow::anyhow!("injected directory-sync failure")),
        )
        .await
        .unwrap();
    assert_eq!(result.id, b.id);
    assert!(Arc::ptr_eq(&host.runtime(), &next));
    assert!(a.iroh_stack.current.lock().await.is_none());
    assert_eq!(
        list_accounts(dir.path()).await.unwrap().active_account_id,
        b.id
    );
    host.shutdown().await;
}

#[tokio::test]
async fn unknown_commit_result_stops_both_runtimes_instead_of_reporting_success() {
    let dir = tempfile::tempdir().unwrap();
    let (host, a, b) = fixture(dir.path()).await;
    let original_id = list_accounts(dir.path()).await.unwrap().active_account_id;
    let next = install_next(&host, &b).await;
    std::fs::write(dir.path().join("accounts.json"), b"corrupt").unwrap();
    assert!(
        host.finish_account_change(
            a.clone(),
            &original_id,
            b,
            false,
            Err(anyhow::anyhow!("injected unknown commit"))
        )
        .await
        .is_err()
    );
    assert!(host.is_stopped());
    assert!(a.iroh_stack.current.lock().await.is_none());
    assert!(next.iroh_stack.current.lock().await.is_none());
}

#[tokio::test]
async fn creation_queued_behind_lifecycle_guard_cannot_mutate_after_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (host, _, _) = fixture(dir.path()).await;
    let original = std::fs::read(dir.path().join("accounts.json")).unwrap();
    let request = crate::CreateAccountRequest {
        account_id: list_accounts(dir.path()).await.unwrap().active_account_id,
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let guard = host.operation_guard.lock().await;
    let queued_host = host.clone();
    let queued = tokio::spawn(async move { queued_host.create_account(request).await });
    tokio::task::yield_now().await;
    assert!(!queued.is_finished());
    host.shutdown_started.store(true, Ordering::Release);
    drop(guard);
    assert!(queued.await.unwrap().is_err());
    assert_eq!(
        std::fs::read(dir.path().join("accounts.json")).unwrap(),
        original
    );
    assert!(!dir.path().join("account-transitions").exists());
    host.runtime().shutdown().await;
}

#[tokio::test]
async fn queued_initial_profile_save_is_rejected_after_account_switch() {
    let dir = tempfile::tempdir().unwrap();
    let (host, a, b) = fixture(dir.path()).await;
    let original_id = list_accounts(dir.path()).await.unwrap().active_account_id;
    let next = Arc::new(
        DesktopRuntime::new_with_config_and_identity(
            account_db_path(dir.path(), &b.id),
            kukuri_transport::TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
        .await
        .unwrap(),
    );
    let guard = host.operation_guard.lock().await;
    let queued_host = host.clone();
    let queued = tokio::spawn(async move {
        queued_host
            .save_initial_profile(crate::InitialProfileRequest {
                account_id: original_id,
                profile: crate::SetMyProfileRequest {
                    display_name: Some("Must not save".into()),
                    ..Default::default()
                },
            })
            .await
    });
    tokio::task::yield_now().await;
    host.replace_runtime_locked(next.clone()).await.unwrap();
    set_active_account(dir.path(), &b.id).await.unwrap();
    let registry = std::fs::read(dir.path().join("accounts.json")).unwrap();
    drop(guard);
    assert!(queued.await.unwrap().is_err());
    assert_eq!(
        std::fs::read(dir.path().join("accounts.json")).unwrap(),
        registry
    );
    assert_eq!(next.get_my_profile().await.unwrap().updated_at, 0);
    assert_eq!(a.get_my_profile().await.unwrap().updated_at, 0);
    a.shutdown().await;
    host.shutdown().await;
}

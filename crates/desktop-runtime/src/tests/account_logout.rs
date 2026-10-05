use super::*;
use crate::accounts::create::{commit_account_creation, prepare_account_creation};
use crate::accounts::{add_account, ensure_accounts_initialized, lifecycle::*};
use crate::identity::load_existing_keys;

const MODE: IdentityStorageMode = IdentityStorageMode::FileOnly;

#[tokio::test]
async fn explicit_creation_preserves_accounts_and_reuses_operation_on_retry() {
    let dir = tempdir().unwrap();
    ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let original = list_accounts(dir.path()).await.unwrap().active_account_id;
    let request = CreateAccountRequest {
        account_id: original.clone(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let first = prepare_account_creation(dir.path(), MODE, &request)
        .await
        .unwrap();
    let retry = prepare_account_creation(dir.path(), MODE, &request)
        .await
        .unwrap();
    assert_eq!(first, retry);
    assert_eq!(list_accounts(dir.path()).await.unwrap().accounts.len(), 1);
    commit_account_creation(dir.path(), &retry).await.unwrap();
    let snapshot = list_accounts(dir.path()).await.unwrap();
    assert_eq!(snapshot.accounts.len(), 2);
    assert_eq!(snapshot.active_account_id, first.next.id);
    assert_eq!(
        prepare_account_creation(dir.path(), MODE, &request)
            .await
            .unwrap()
            .next,
        first.next
    );
    let logout = prepare_logout(dir.path(), MODE, &first.next.id)
        .await
        .unwrap();
    assert_eq!(logout.next.id, original);
    assert!(
        profile_setup_required(dir.path(), &first.next.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn completed_creation_cannot_reactivate_a_logged_out_account_after_other_creations() {
    let dir = tempdir().unwrap();
    ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let a = list_accounts(dir.path()).await.unwrap().active_account_id;
    let x = CreateAccountRequest {
        account_id: a.clone(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let b = prepare_account_creation(dir.path(), MODE, &x)
        .await
        .unwrap();
    commit_account_creation(dir.path(), &b).await.unwrap();
    let y = CreateAccountRequest {
        account_id: b.next.id.clone(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let c = prepare_account_creation(dir.path(), MODE, &y)
        .await
        .unwrap();
    commit_account_creation(dir.path(), &c).await.unwrap();
    set_active_account(dir.path(), &b.next.id).await.unwrap();
    let logout = prepare_logout(dir.path(), MODE, &b.next.id).await.unwrap();
    commit_logout(dir.path(), &logout).await.unwrap();
    set_active_account(dir.path(), &a).await.unwrap();
    let before = fs::read(dir.path().join("accounts.json")).unwrap();
    assert!(
        prepare_account_creation(dir.path(), MODE, &x)
            .await
            .is_err()
    );
    assert_eq!(fs::read(dir.path().join("accounts.json")).unwrap(), before);
}

#[tokio::test]
async fn logout_returns_to_previous_account_and_retains_local_data() {
    let dir = tempdir().unwrap();
    let first_db = ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let a = list_accounts(dir.path()).await.unwrap().active_account_id;
    let b_keys = KukuriKeys::generate();
    let b = add_account(dir.path(), MODE, &b_keys, None, false)
        .await
        .unwrap();
    set_active_account(dir.path(), &b.id).await.unwrap();
    let b_db = account_db_path(dir.path(), &b.id);
    fs::write(&b_db, b"retained database").unwrap();
    let prepared = prepare_logout(dir.path(), MODE, &b.id).await.unwrap();
    assert_eq!(prepared.next.id, a);
    assert_eq!(
        list_accounts(dir.path()).await.unwrap().active_account_id,
        b.id
    );
    commit_logout(dir.path(), &prepared).await.unwrap();
    let snapshot = list_accounts(dir.path()).await.unwrap();
    assert_eq!(snapshot.active_account_id, a);
    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(fs::read(&b_db).unwrap(), b"retained database");
    assert_eq!(
        ensure_accounts_initialized(dir.path(), MODE).await.unwrap(),
        first_db
    );
    let restored = add_account(dir.path(), MODE, &b_keys, None, false)
        .await
        .unwrap();
    assert_eq!(restored.id, b.id);
    assert_eq!(fs::read(&b_db).unwrap(), b"retained database");
}

#[tokio::test]
async fn last_logout_preparation_reuses_generated_account_across_retry() {
    let dir = tempdir().unwrap();
    let original_db = ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let original = list_accounts(dir.path()).await.unwrap().active_account_id;
    let before = load_existing_keys(&original_db, MODE)
        .await
        .unwrap()
        .unwrap()
        .public_key_hex();
    let first = prepare_logout(dir.path(), MODE, &original).await.unwrap();
    let retry = prepare_logout(dir.path(), MODE, &original).await.unwrap();
    assert_eq!(first.next, retry.next);
    assert_ne!(first.next.pubkey, before);
    commit_logout(dir.path(), &retry).await.unwrap();
    assert!(prepare_logout(dir.path(), MODE, &original).await.is_err());
    assert_eq!(list_accounts(dir.path()).await.unwrap().accounts.len(), 1);
    assert_eq!(
        ensure_accounts_initialized(dir.path(), MODE).await.unwrap(),
        account_db_path(dir.path(), &first.next.id)
    );
    assert_eq!(
        load_existing_keys(&original_db, MODE)
            .await
            .unwrap()
            .unwrap()
            .public_key_hex(),
        before
    );
    assert!(
        profile_setup_required(dir.path(), &first.next.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn logout_rejects_stale_target_without_mutating_registry() {
    let dir = tempdir().unwrap();
    ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let before = fs::read(dir.path().join("accounts.json")).unwrap();
    assert!(prepare_logout(dir.path(), MODE, "unknown").await.is_err());
    assert_eq!(fs::read(dir.path().join("accounts.json")).unwrap(), before);
}

#[tokio::test]
async fn missing_registered_identity_does_not_generate_a_replacement_on_logout_or_restart() {
    let dir = tempdir().unwrap();
    ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let a = list_accounts(dir.path()).await.unwrap().active_account_id;
    let b = add_account(dir.path(), MODE, &KukuriKeys::generate(), None, false)
        .await
        .unwrap();
    set_active_account(dir.path(), &b.id).await.unwrap();
    let a_db = account_db_path(dir.path(), &a);
    fs::remove_file(a_db.with_extension("identity-key")).unwrap();
    fs::remove_file(a_db.with_extension("identity-store")).unwrap();
    let before = fs::read(dir.path().join("accounts.json")).unwrap();
    assert!(prepare_logout(dir.path(), MODE, &b.id).await.is_err());
    assert_eq!(fs::read(dir.path().join("accounts.json")).unwrap(), before);
    set_active_account(dir.path(), &a).await.unwrap();
    assert!(ensure_accounts_initialized(dir.path(), MODE).await.is_err());
    assert!(load_existing_keys(&a_db, MODE).await.unwrap().is_none());
}

#[tokio::test]
async fn prepared_last_logout_reselects_an_imported_account_before_commit() {
    let dir = tempdir().unwrap();
    ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let target = list_accounts(dir.path()).await.unwrap().active_account_id;
    let generated = prepare_logout(dir.path(), MODE, &target).await.unwrap();
    let imported = add_account(dir.path(), MODE, &KukuriKeys::generate(), None, false)
        .await
        .unwrap();
    let next = prepare_logout(dir.path(), MODE, &target).await.unwrap();
    assert_eq!(next.next.id, imported.id);
    assert!(!next.generated);
    assert!(commit_logout(dir.path(), &generated).await.is_err());
    commit_logout(dir.path(), &next).await.unwrap();
    let remaining = list_accounts(dir.path()).await.unwrap().accounts;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, imported.id);
    assert_eq!(remaining[0].pubkey, imported.pubkey);
    assert!(remaining[0].last_used_at >= imported.last_used_at);
}

#[tokio::test]
async fn commit_reconciliation_distinguishes_applied_old_and_unknown_registry() {
    let dir = tempdir().unwrap();
    ensure_accounts_initialized(dir.path(), MODE).await.unwrap();
    let target = list_accounts(dir.path()).await.unwrap().active_account_id;
    let prepared = prepare_logout(dir.path(), MODE, &target).await.unwrap();
    assert!(
        !reconcile_account_commit(dir.path(), &target, &prepared.next.id, true)
            .await
            .unwrap()
    );
    commit_logout(dir.path(), &prepared).await.unwrap();
    assert!(
        reconcile_account_commit(dir.path(), &target, &prepared.next.id, true)
            .await
            .unwrap()
    );
    fs::write(dir.path().join("accounts.json"), b"corrupt").unwrap();
    assert!(
        reconcile_account_commit(dir.path(), &target, &prepared.next.id, true)
            .await
            .is_err()
    );
}

async fn profile_test_host(dir: &Path) -> Arc<ClientHost> {
    let db = ensure_accounts_initialized(dir, MODE).await.unwrap();
    let runtime =
        DesktopRuntime::new_with_config_and_identity(&db, TransportNetworkConfig::loopback(), MODE)
            .await
            .unwrap();
    ClientHost::from_runtime(dir.to_path_buf(), Arc::new(runtime))
        .await
        .unwrap()
}

#[tokio::test]
async fn host_switch_rejects_missing_registered_keys_without_changing_identity() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let host = profile_test_host(dir.path()).await;
    let before = list_accounts(dir.path()).await.unwrap();
    let author = host.runtime().get_my_profile().await.unwrap().pubkey;
    let b = add_account(dir.path(), MODE, &KukuriKeys::generate(), None, false)
        .await
        .unwrap();
    let db = account_db_path(dir.path(), &b.id);
    fs::remove_file(db.with_extension("identity-key")).unwrap();
    fs::remove_file(db.with_extension("identity-store")).unwrap();
    assert!(host.switch_account(&b.id).await.is_err());
    assert_eq!(
        list_accounts(dir.path()).await.unwrap().active_account_id,
        before.active_account_id
    );
    assert_eq!(
        host.runtime().get_my_profile().await.unwrap().pubkey,
        author
    );
    assert!(load_existing_keys(&db, MODE).await.unwrap().is_none());
    host.shutdown().await;
}

#[tokio::test]
async fn initial_profile_journal_recovers_without_new_envelope_and_honors_edited_retry() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let host = profile_test_host(dir.path()).await;
    let account_id = list_accounts(dir.path()).await.unwrap().active_account_id;
    let request = InitialProfileRequest {
        account_id: account_id.clone(),
        profile: SetMyProfileRequest {
            display_name: Some("First saved name".into()),
            ..Default::default()
        },
    };
    host.save_initial_profile(request.clone()).await.unwrap();
    let journal = account_db_path(dir.path(), &account_id).with_extension("initial-profile.json");
    let first = fs::read(&journal).unwrap();
    let registry_path = dir.path().join("accounts.json");
    let mut registry: serde_json::Value =
        serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
    registry["profile_setup"] = serde_json::json!([account_id]);
    fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
    host.shutdown().await;
    drop(host);
    let restarted = profile_test_host(dir.path()).await;
    assert!(!restarted.profile_setup_required(&account_id).await.unwrap());
    assert_eq!(fs::read(&journal).unwrap(), first);
    let recovered = restarted
        .save_initial_profile(request.clone())
        .await
        .unwrap();
    assert_eq!(recovered.display_name.as_deref(), Some("First saved name"));
    assert_eq!(fs::read(&journal).unwrap(), first);
    let edited = restarted
        .save_initial_profile(InitialProfileRequest {
            profile: SetMyProfileRequest {
                display_name: Some("Edited after error".into()),
                ..Default::default()
            },
            ..request
        })
        .await
        .unwrap();
    assert_eq!(edited.display_name.as_deref(), Some("Edited after error"));
    assert_ne!(fs::read(&journal).unwrap(), first);
    assert!(
        restarted
            .save_initial_profile(InitialProfileRequest {
                account_id: "another-account".into(),
                profile: Default::default()
            })
            .await
            .is_err()
    );
    restarted.shutdown().await;
}

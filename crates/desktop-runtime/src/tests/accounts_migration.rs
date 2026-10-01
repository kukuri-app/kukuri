use super::*;

use crate::accounts::{
    account_db_path, account_id_for_pubkey, add_account, ensure_accounts_initialized,
    list_accounts, set_active_account,
};
use crate::identity::{load_existing_keys, persist_keys};

const MODE: IdentityStorageMode = IdentityStorageMode::FileOnly;

async fn active_keys(db_path: &Path) -> KukuriKeys {
    load_existing_keys(db_path, MODE)
        .await
        .expect("load account identity")
        .expect("account identity present")
}

#[tokio::test]
async fn fresh_install_creates_first_account_and_is_stable_across_restarts() {
    let dir = tempdir().expect("tempdir");

    let db_path = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("initialize accounts");
    let keys = active_keys(&db_path).await;
    let expected_id = account_id_for_pubkey(keys.public_key_hex().as_str()).expect("account id");
    assert_eq!(db_path, account_db_path(dir.path(), expected_id.as_str()));

    let snapshot = list_accounts(dir.path()).await.expect("list accounts");
    assert_eq!(snapshot.active_account_id, expected_id);
    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(snapshot.accounts[0].pubkey, keys.public_key_hex());

    let restarted = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("re-initialize accounts");
    assert_eq!(restarted, db_path);
    assert_eq!(
        active_keys(&restarted).await.export_secret_hex(),
        keys.export_secret_hex()
    );
}

#[tokio::test]
async fn flat_layout_migrates_identity_db_and_siblings() {
    let dir = tempdir().expect("tempdir");
    let flat_db = dir.path().join("kukuri.db");
    let keys = KukuriKeys::generate();
    persist_keys(&flat_db, MODE, &keys)
        .await
        .expect("seed flat identity");
    fs::write(&flat_db, b"sqlite").expect("seed flat db");
    fs::write(flat_db.with_extension("discovery.json"), b"{}").expect("seed discovery config");
    fs::write(
        flat_db.with_extension("subscriptions.json"),
        br#"{"version":1,"subscriptions":[]}"#,
    )
    .expect("seed desired subscriptions");
    fs::create_dir_all(flat_db.with_extension("iroh-data")).expect("seed iroh data dir");
    fs::write(
        flat_db.with_extension("iroh-data").join("blob.bin"),
        b"blob",
    )
    .expect("seed iroh blob");
    let capability_file = dir.path().join(format!(
        "kukuri.private-channel-capabilities-{}",
        blake3::hash(b"registry").to_hex()
    ));
    fs::write(&capability_file, b"capabilities").expect("seed capability file");

    let db_path = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("migrate accounts");

    let migrated = active_keys(&db_path).await;
    assert_eq!(migrated.export_secret_hex(), keys.export_secret_hex());
    assert_eq!(fs::read(&db_path).expect("migrated db"), b"sqlite");
    assert_eq!(
        fs::read(db_path.with_extension("discovery.json")).expect("migrated discovery"),
        b"{}"
    );
    assert_eq!(
        fs::read(db_path.with_extension("subscriptions.json"))
            .expect("migrated desired subscriptions"),
        br#"{"version":1,"subscriptions":[]}"#
    );
    assert_eq!(
        fs::read(db_path.with_extension("iroh-data").join("blob.bin")).expect("migrated blob"),
        b"blob"
    );
    let account_dir = db_path.parent().expect("account dir");
    assert_eq!(
        fs::read(account_dir.join(capability_file.file_name().expect("capability file name")))
            .expect("migrated capability file"),
        b"capabilities"
    );

    // 旧 flat レイアウトの実体は消えている。
    assert!(!flat_db.exists());
    assert!(!flat_db.with_extension("identity-key").exists());
    assert!(!flat_db.with_extension("identity-store").exists());
    assert!(!flat_db.with_extension("subscriptions.json").exists());
    assert!(!capability_file.exists());

    let snapshot = list_accounts(dir.path()).await.expect("list accounts");
    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(snapshot.accounts[0].pubkey, keys.public_key_hex());
}

#[tokio::test]
async fn orphan_flat_db_without_identity_gets_a_generated_identity() {
    let dir = tempdir().expect("tempdir");
    let flat_db = dir.path().join("kukuri.db");
    fs::write(&flat_db, b"sqlite").expect("seed orphan flat db");

    let db_path = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("migrate orphan db");

    assert_eq!(fs::read(&db_path).expect("migrated db"), b"sqlite");
    assert!(!flat_db.exists());
    let snapshot = list_accounts(dir.path()).await.expect("list accounts");
    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(
        active_keys(&db_path).await.public_key_hex(),
        snapshot.accounts[0].pubkey
    );
}

#[tokio::test]
async fn interrupted_migration_resumes_without_losing_identity() {
    let dir = tempdir().expect("tempdir");
    let flat_db = dir.path().join("kukuri.db");
    let keys = KukuriKeys::generate();
    persist_keys(&flat_db, MODE, &keys)
        .await
        .expect("seed flat identity");
    fs::write(&flat_db, b"sqlite").expect("seed flat db");

    let db_path = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("migrate accounts");

    // registry 書き込み後・ファイル移動/掃除の途中でクラッシュした状態を再現する:
    // 旧レイアウトに identity と db が残っている。
    persist_keys(&flat_db, MODE, &keys)
        .await
        .expect("recreate flat identity");
    fs::rename(&db_path, &flat_db).expect("move db back to flat layout");

    let resumed = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("resume migration");

    assert_eq!(resumed, db_path);
    assert_eq!(
        active_keys(&resumed).await.export_secret_hex(),
        keys.export_secret_hex()
    );
    assert_eq!(fs::read(&resumed).expect("resumed db"), b"sqlite");
    assert!(!flat_db.exists());
    assert!(!flat_db.with_extension("identity-key").exists());
    let snapshot = list_accounts(dir.path()).await.expect("list accounts");
    assert_eq!(snapshot.accounts.len(), 1, "resume must not add accounts");
}

#[tokio::test]
async fn stray_flat_identity_after_migration_is_registered_as_inactive_account() {
    let dir = tempdir().expect("tempdir");
    let first_db = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("initialize accounts");
    let first_keys = active_keys(&first_db).await;

    let stray_keys = KukuriKeys::generate();
    let flat_db = dir.path().join("kukuri.db");
    persist_keys(&flat_db, MODE, &stray_keys)
        .await
        .expect("seed stray flat identity");

    let active_db = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("absorb stray identity");

    assert_eq!(active_db, first_db, "active account must not change");
    let snapshot = list_accounts(dir.path()).await.expect("list accounts");
    assert_eq!(snapshot.accounts.len(), 2);
    assert_eq!(
        snapshot.active_account_id,
        account_id_for_pubkey(first_keys.public_key_hex().as_str()).expect("first id")
    );
    let stray_id = account_id_for_pubkey(stray_keys.public_key_hex().as_str()).expect("stray id");
    let stray_db = account_db_path(dir.path(), stray_id.as_str());
    assert_eq!(
        active_keys(&stray_db).await.export_secret_hex(),
        stray_keys.export_secret_hex()
    );
    assert!(!flat_db.with_extension("identity-key").exists());
}

#[tokio::test]
async fn add_account_registers_new_identity_and_can_activate_it() {
    let dir = tempdir().expect("tempdir");
    let first_db = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("initialize accounts");
    let first_keys = active_keys(&first_db).await;

    let imported = KukuriKeys::generate();
    let record = add_account(dir.path(), MODE, &imported, Some("sub".into()), true)
        .await
        .expect("add account");

    assert_eq!(record.pubkey, imported.public_key_hex());
    assert_eq!(record.label.as_deref(), Some("sub"));
    let snapshot = list_accounts(dir.path()).await.expect("list accounts");
    assert_eq!(snapshot.accounts.len(), 2);
    assert_eq!(snapshot.active_account_id, record.id);

    let imported_db = account_db_path(dir.path(), record.id.as_str());
    assert_eq!(
        active_keys(&imported_db).await.export_secret_hex(),
        imported.export_secret_hex()
    );
    // 既存アカウントの identity は無傷。
    assert_eq!(
        active_keys(&first_db).await.export_secret_hex(),
        first_keys.export_secret_hex()
    );
}

#[tokio::test]
async fn add_account_rejects_duplicate_pubkey_and_keeps_registry_intact() {
    let dir = tempdir().expect("tempdir");
    let first_db = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("initialize accounts");
    let first_keys = active_keys(&first_db).await;

    let error = add_account(dir.path(), MODE, &first_keys, None, true)
        .await
        .expect_err("duplicate pubkey must be rejected");
    assert!(
        error.to_string().contains("already exists"),
        "unexpected error: {error}"
    );

    let snapshot = list_accounts(dir.path()).await.expect("list accounts");
    assert_eq!(snapshot.accounts.len(), 1);
    assert_eq!(
        active_keys(&first_db).await.export_secret_hex(),
        first_keys.export_secret_hex()
    );
}

#[tokio::test]
async fn set_active_account_switches_and_rejects_unknown_ids() {
    let dir = tempdir().expect("tempdir");
    let first_db = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("initialize accounts");
    let first_id =
        account_id_for_pubkey(active_keys(&first_db).await.public_key_hex().as_str()).expect("id");
    let second = KukuriKeys::generate();
    let second_record = add_account(dir.path(), MODE, &second, None, false)
        .await
        .expect("add second account");
    assert_eq!(
        list_accounts(dir.path())
            .await
            .expect("list accounts")
            .active_account_id,
        first_id
    );

    let switched = set_active_account(dir.path(), second_record.id.as_str())
        .await
        .expect("switch");
    assert_eq!(switched.id, second_record.id);
    assert_eq!(
        list_accounts(dir.path())
            .await
            .expect("list accounts")
            .active_account_id,
        second_record.id
    );

    set_active_account(dir.path(), "0000000000000000")
        .await
        .expect_err("unknown id must fail");
    assert_eq!(
        list_accounts(dir.path())
            .await
            .expect("list accounts")
            .active_account_id,
        second_record.id,
        "failed switch must not change the active account"
    );
}

#[test]
fn account_key_request_debug_redacts_passphrase() {
    let export_request = ExportAccountKeyRequest {
        passphrase: "hunter2 secret".into(),
    };
    let debug = format!("{export_request:?}");
    assert!(!debug.contains("hunter2"), "unexpected debug: {debug}");
    assert!(debug.contains("<redacted>"), "unexpected debug: {debug}");

    let import_request = ImportAccountKeyRequest {
        export: "kukuri-account-key.v1.abc".into(),
        passphrase: "hunter2 secret".into(),
        label: Some("sub".into()),
    };
    let debug = format!("{import_request:?}");
    assert!(!debug.contains("hunter2"), "unexpected debug: {debug}");
    assert!(debug.contains("<redacted>"), "unexpected debug: {debug}");
}

#[tokio::test]
async fn import_preview_reports_registration_state_without_secrets() {
    let dir = tempdir().expect("tempdir");
    let db_path = ensure_accounts_initialized(dir.path(), MODE)
        .await
        .expect("initialize accounts");
    let keys = active_keys(&db_path).await;
    let passphrase = "correct horse battery staple";

    let registered_export =
        kukuri_core::encrypt_account_key_export(&keys, passphrase).expect("export active key");
    let registered = crate::accounts::preview_account_key_import(dir.path(), &registered_export)
        .await
        .expect("preview registered key");
    assert!(registered.already_registered);
    assert_eq!(registered.public_key, keys.public_key_hex());

    let foreign_keys = KukuriKeys::generate();
    let foreign_export =
        kukuri_core::encrypt_account_key_export(&foreign_keys, passphrase).expect("export");
    let foreign = crate::accounts::preview_account_key_import(dir.path(), &foreign_export)
        .await
        .expect("preview foreign key");
    assert!(!foreign.already_registered);

    // preview はどの経路にも秘密 hex を含まない。
    let serialized = serde_json::to_string(&registered).expect("serialize preview");
    assert!(!serialized.contains(keys.export_secret_hex().as_str()));
    let debug = format!("{registered:?}");
    assert!(!debug.contains(keys.export_secret_hex().as_str()));
}

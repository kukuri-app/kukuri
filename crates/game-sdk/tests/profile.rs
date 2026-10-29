//! SDK の profile と既存のアカウント・同意の入口（ADR 0070 §4 と §10 の F-PRO。#1975 AC-2）。
//! 同意の後は実際の runtime を起動するので、CLI の process 試験と同じく keyring を使わず loopback だけで動かす。
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    path::Path,
    sync::Once,
    time::{Duration, Instant},
};

use kukuri_desktop_runtime::{ClientProfile, ClientProfileKind, ProfileLease};
use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{
    PROFILE, error_code, finish, open_real, poll_once, request, response_for, send, send_secret,
};

fn isolated_environment() {
    static SET: Once = Once::new();
    SET.call_once(|| unsafe {
        for name in [
            "KUKURI_APP_DATA_DIR",
            "KUKURI_INSTANCE",
            "KUKURI_DISCOVERY_SEEDS",
            "KUKURI_ADVERTISE_PORT",
        ] {
            std::env::remove_var(name);
        }
        std::env::set_var("KUKURI_DISABLE_KEYRING", "1");
        std::env::set_var("KUKURI_DISCOVERY_MODE", "static_peer");
        std::env::set_var("KUKURI_BIND_ADDR", "127.0.0.1:0");
        std::env::set_var("KUKURI_ADVERTISE_HOST", "127.0.0.1");
    });
}

/// 応答の data を返す。失敗なら error を見せて止まる。
fn call(handle: *mut KukuriHandle, id: &str, command: &str, payload: Value) -> Value {
    assert_eq!(send(handle, &request(id, command, payload)), 0, "{command}");
    let response = response_for(handle, id, Duration::from_secs(60));
    assert_eq!(response["ok"], true, "{command}: {}", response["error"]);
    response["data"].clone()
}

fn wait_for_startup(handle: *mut KukuriHandle) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !poll_once(handle, 64)
        .unwrap()
        .iter()
        .any(|item| item.kind == 3 && item.json["type"] == "startup_status_changed")
    {
        assert!(Instant::now() < deadline, "startup did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 表示された現行の文書と 18 歳以上の申告に同意する（ゲームが同意の画面で行う操作）。
fn consent(handle: *mut KukuriHandle) {
    let status = call(
        handle,
        "consent-status",
        "get_app_consent_status",
        json!({}),
    );
    let documents: Vec<Value> = status["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|doc| json!({"slug": doc["slug"], "version": doc["currentVersion"]}))
        .collect();
    let accepted = call(
        handle,
        "consent",
        "accept_app_consents",
        json!({"documents": documents, "language": "ja", "age_attested": true}),
    );
    assert_eq!(accepted["status"], "ready");
}

fn public_key(handle: *mut KukuriHandle, id: &str) -> Value {
    call(handle, id, "get_sync_status", json!({}))["local_author_pubkey"].clone()
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_new_profile_waits_for_consent_without_building_the_runtime() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let handle = open_real(dir.path());
    wait_for_startup(handle);
    let status = call(handle, "status", "get_desktop_startup_status", json!({}));
    assert_eq!(status["status"], "consent_required");
    // 同意の前は runtime の command を断る。
    assert_eq!(
        send(handle, &request("sync", "get_sync_status", json!({}))),
        0
    );
    assert_eq!(
        error_code(&response_for(handle, "sync", Duration::from_secs(30))),
        "consent_required"
    );
    // runtime（DB・node）を組み立てていないので network の I/O は起きない。profile の所有の印だけがある。
    assert_eq!(
        entries(dir.path()),
        [".kukuri-profile.json", ".kukuri-profile.lock"]
    );
    finish(handle);
}

#[test]
fn consent_creates_an_account_that_a_restart_keeps() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let handle = open_real(dir.path());
    wait_for_startup(handle);
    consent(handle);
    let created = public_key(handle, "key");
    assert!(
        created.as_str().is_some_and(|key| key.len() == 64),
        "{created}"
    );
    finish(handle);

    let reopened = open_real(dir.path());
    wait_for_startup(reopened);
    let deadline = Instant::now() + Duration::from_secs(30);
    while call(reopened, "ready", "get_desktop_startup_status", json!({}))["status"] != "ready" {
        assert!(
            Instant::now() < deadline,
            "the consented profile did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(public_key(reopened, "key-again"), created);
    finish(reopened);
}

#[test]
fn an_encrypted_export_imports_the_same_identity_and_a_wrong_passphrase_changes_nothing() {
    isolated_environment();
    let (donor_dir, game_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (donor, game) = (open_real(donor_dir.path()), open_real(game_dir.path()));
    wait_for_startup(donor);
    wait_for_startup(game);
    consent(donor);
    consent(game);
    let donor_key = public_key(donor, "donor-key");
    let passphrase = "game-sdk-passphrase-sentinel";

    // 鍵の export は秘密の入力（passphrase）と秘密の出力（暗号化した export）で行う。
    let mut export =
        serde_json::from_slice::<Value>(&request("export", "export_account_key", json!({})))
            .unwrap();
    export["accepts_secret_output"] = json!(true);
    assert_eq!(
        send_secret(donor, export.to_string().as_bytes(), passphrase.as_bytes()),
        0
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let exported = loop {
        let items = poll_once(donor, 64).unwrap();
        if let Some(position) = items
            .iter()
            .position(|item| item.kind == 1 && item.json["request_id"] == "export")
        {
            assert_eq!(items[position].json["ok"], true, "{}", items[position].json);
            assert!(!String::from_utf8_lossy(&items[position].raw).contains("sentinel"));
            assert_eq!(
                items[position + 1].kind,
                2,
                "the export follows as a secret item"
            );
            break String::from_utf8(items[position + 1].raw.clone()).unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };

    let before = call(game, "before", "list_accounts", json!({}));
    let game_key = public_key(game, "game-key");
    let wrong = json!({"export": exported, "passphrase": "not-the-passphrase"}).to_string();
    assert_eq!(
        send_secret(
            game,
            &request("wrong", "import_account_key", json!({})),
            wrong.as_bytes()
        ),
        0
    );
    let refused = response_for(game, "wrong", Duration::from_secs(60));
    assert_eq!(refused["ok"], false);
    assert_eq!(call(game, "unchanged", "list_accounts", json!({})), before);
    assert_eq!(public_key(game, "unchanged-key"), game_key);

    let secret = json!({"export": exported, "passphrase": passphrase}).to_string();
    assert_eq!(
        send_secret(
            game,
            &request("import", "import_account_key", json!({"label": "kukuri"})),
            secret.as_bytes()
        ),
        0
    );
    let imported = response_for(game, "import", Duration::from_secs(60));
    assert_eq!(imported["ok"], true, "{}", imported["error"]);
    assert!(!imported.to_string().contains("sentinel"));
    call(
        game,
        "switch",
        "switch_account",
        json!({"account_id": imported["data"]["id"]}),
    );
    assert_eq!(public_key(game, "imported-key"), donor_key);

    // 秘密の値は応答・event のどこにも出ない（残りの event も確かめる）。
    for item in poll_once(game, 256)
        .unwrap()
        .into_iter()
        .chain(poll_once(donor, 256).unwrap())
    {
        let text = String::from_utf8_lossy(&item.raw);
        assert!(
            !text.contains("sentinel") && !text.contains(&exported),
            "{text}"
        );
    }
    finish(game);
    finish(donor);
}

#[test]
fn kukuri_app_profiles_and_unmarked_directories_are_not_opened() {
    isolated_environment();
    let cli = tempfile::tempdir().unwrap();
    drop(
        ProfileLease::acquire(ClientProfile {
            name: PROFILE.into(),
            app_data_dir: cli.path().to_path_buf(),
            kind: ClientProfileKind::Cli,
        })
        .unwrap(),
    );
    let unmarked = tempfile::tempdir().unwrap();
    std::fs::write(unmarked.path().join("kukuri.db"), b"existing state").unwrap();
    for (dir, code) in [
        (cli.path(), "profile_kind_mismatch"),
        (unmarked.path(), "legacy_profile_unclassified"),
    ] {
        let before = entries(dir);
        let handle = open_real(dir);
        assert_eq!(
            send(handle, &request("open", "client.status", json!({}))),
            0
        );
        assert_eq!(
            error_code(&response_for(handle, "open", Duration::from_secs(30))),
            code
        );
        finish(handle);
        let mut after = entries(dir);
        after.retain(|name| name != ".kukuri-profile.lock");
        let mut expected = before.clone();
        expected.retain(|name| name != ".kukuri-profile.lock");
        assert_eq!(after, expected, "the profile is not changed");
    }
}

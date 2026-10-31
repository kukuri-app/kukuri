//! Steam Cloud へのゲームのアカウントの保存（ADR 0070 §4.3。#1997 AC-2）。SDK は Steam を扱わず、ゲームで作ったアカウントの
//! 鍵と game player を profile の `game-account.json` に書く・消すだけなので、その file と状態の値を確かめる。
#![allow(clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{
    call, consent, error_code, finish, isolated_environment, open_real, request, response_for,
    send, wait_for_startup,
};

const GAME: &str = "com.example.cyber-disc-arena";
const CLOUD_FILE: &str = "game-account.json";

fn id(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn opened(dir: &Path) -> *mut KukuriHandle {
    let handle = open_real(dir);
    wait_for_startup(handle);
    handle
}

fn state(handle: *mut KukuriHandle) -> Value {
    call(
        handle,
        &id("get"),
        "get_cloud_account",
        json!({"application_id": GAME}),
    )["state"]
        .clone()
}

fn set(handle: *mut KukuriHandle, enabled: bool) -> Value {
    call(
        handle,
        &id("set"),
        "set_cloud_account",
        json!({"application_id": GAME, "enabled": enabled}),
    )["state"]
        .clone()
}

/// 断られた `set_cloud_account` の `details.reason`。
fn refused(handle: *mut KukuriHandle, enabled: bool) -> String {
    let request_id = id("refused");
    assert_eq!(
        send(
            handle,
            &request(
                &request_id,
                "set_cloud_account",
                json!({"application_id": GAME, "enabled": enabled}),
            ),
        ),
        0
    );
    let response = response_for(handle, &request_id, Duration::from_secs(60));
    assert_eq!(error_code(&response), "conflict", "{response}");
    response["error"]["details"]["reason"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn start(handle: *mut KukuriHandle) -> Value {
    call(
        handle,
        &id("start"),
        "start_game_player",
        json!({"application_id": GAME}),
    )
}

#[test]
fn the_game_account_is_saved_for_steam_cloud_and_removed_when_turned_off() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let handle = opened(dir.path());
    consent(handle);
    assert_eq!(state(handle), "off");
    assert_eq!(refused(handle, true), "game_player_required");

    // ゲスト（保存なし）で始めた後から有効にしても、game player ID は変わらない。
    let player = start(handle);
    assert_eq!(set(handle, true), "on");
    let file: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(CLOUD_FILE)).unwrap()).unwrap();
    assert_eq!(file["application_id"], GAME);
    assert_eq!(file["game_player_id"], player["game_player_id"]);
    assert_eq!(file["secret_key"].as_str().unwrap().len(), 64);
    assert_eq!(set(handle, true), "on");
    finish(handle);

    let handle = opened(dir.path());
    assert_eq!(start(handle), player);
    assert_eq!(state(handle), "on");
    assert_eq!(set(handle, false), "off");
    assert!(!dir.path().join(CLOUD_FILE).exists());
    assert_eq!(start(handle), player);
    finish(handle);
}

#[test]
fn a_file_saved_elsewhere_or_unreadable_is_left_alone() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let handle = opened(dir.path());
    consent(handle);
    let player = start(handle);
    assert_eq!(set(handle, true), "on");
    let path = dir.path().join(CLOUD_FILE);

    // 別の鍵、または同じ鍵で別の game player の file は、別の端末で保存したもの。
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (field, value) in [
        ("secret_key", "11".repeat(32)),
        (
            "game_player_id",
            "00000000-0000-4000-8000-000000000009".to_owned(),
        ),
    ] {
        let mut elsewhere = saved.clone();
        elsewhere[field] = json!(value);
        let elsewhere = serde_json::to_vec(&elsewhere).unwrap();
        std::fs::write(&path, &elsewhere).unwrap();
        assert_eq!(state(handle), "other_account");
        for enabled in [true, false] {
            assert_eq!(refused(handle, enabled), "integration_required");
        }
        assert_eq!(std::fs::read(&path).unwrap(), elsewhere);
    }

    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(state(handle), "unreadable");
    for enabled in [true, false] {
        assert_eq!(refused(handle, enabled), "cloud_account_unreadable");
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
    assert_eq!(start(handle), player);
    finish(handle);
}

//! ゲームの開始の入口と game player（ADR 0070 §4.1。#1996 AC-2）。同意の後は実際の runtime を起動するので、keyring を
//! 使わず loopback だけで動かす。
#![allow(clippy::unwrap_used)]

mod support;

use std::time::{Duration, Instant};

use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{
    call, consent, error_code, finish, isolated_environment, open_real, request, response_for,
    send, wait_for_startup,
};

const GAME: &str = "com.example.cyber-disc-arena";

fn public_key(handle: *mut KukuriHandle) -> Value {
    call(handle, "key", "get_sync_status", json!({}))["local_author_pubkey"].clone()
}

fn reopen(dir: &std::path::Path) -> *mut KukuriHandle {
    let handle = open_real(dir);
    wait_for_startup(handle);
    let deadline = Instant::now() + Duration::from_secs(30);
    while call(handle, "ready", "get_desktop_startup_status", json!({}))["status"] != "ready" {
        assert!(
            Instant::now() < deadline,
            "the consented profile did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    handle
}

/// 断られた要求の `details.reason`。
fn refused(handle: *mut KukuriHandle, id: &str, command: &str, payload: Value) -> String {
    assert_eq!(send(handle, &request(id, command, payload)), 0);
    let response = response_for(handle, id, Duration::from_secs(60));
    assert_eq!(error_code(&response), "conflict", "{response}");
    response["error"]["details"]["reason"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn starting_keeps_the_same_player_and_key_across_restarts_and_refuses_another_game() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let handle = open_real(dir.path());
    wait_for_startup(handle);
    consent(handle);
    let created = public_key(handle);
    let game = json!({"application_id": GAME});
    assert_eq!(
        call(handle, "none", "get_game_player", game.clone()),
        Value::Null
    );
    finish(handle);

    // 「はじめる」の前に閉じて再起動しても、同じゲーム用のアカウントで開始の画面に戻る（新しいアカウントを作らない）。
    let handle = reopen(dir.path());
    assert_eq!(public_key(handle), created);
    assert_eq!(
        call(handle, "accounts", "list_accounts", json!({}))["accounts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        call(handle, "still-none", "get_game_player", game.clone()),
        Value::Null
    );

    let player = call(handle, "start", "start_game_player", game.clone());
    assert_eq!(player["public_key"], created);
    assert!(
        player["game_player_id"]
            .as_str()
            .is_some_and(|id| id.len() == 36 && id.matches('-').count() == 4),
        "{player}"
    );
    assert_ne!(player["game_player_id"], created);
    assert_eq!(
        call(handle, "again", "start_game_player", game.clone()),
        player
    );
    // 1 つの profile は 1 つのゲームの game player だけを持つ。
    let other = json!({"application_id": "com.example.another-game"});
    for (id, command) in [
        ("other-get", "get_game_player"),
        ("other-start", "start_game_player"),
    ] {
        assert_eq!(
            refused(handle, id, command, other.clone()),
            "game_player_application_mismatch"
        );
    }
    finish(handle);

    let handle = reopen(dir.path());
    assert_eq!(
        call(handle, "saved", "get_game_player", game.clone()),
        player
    );
    assert_eq!(call(handle, "resume", "start_game_player", game), player);
    assert_eq!(public_key(handle), created);
    finish(handle);
}

#[test]
fn a_started_player_does_not_switch_to_another_account() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let handle = open_real(dir.path());
    wait_for_startup(handle);
    consent(handle);
    let game = json!({"application_id": GAME});
    let player = call(handle, "start", "start_game_player", game.clone());
    let bound = call(handle, "accounts", "list_accounts", json!({}))["active_account_id"].clone();

    // profile に別のアカウントを足す（作ると active が移る）。
    let other = call(
        handle,
        "create",
        "create_account",
        json!({"account_id": bound, "operation_id": "00000000-0000-4000-8000-000000000001"}),
    )["id"]
        .clone();
    assert_ne!(other, bound);
    // 結び付けたアカウントへは切り替えられる。別のアカウントへは断り、active を変えない。
    call(
        handle,
        "switch-back",
        "switch_account",
        json!({"account_id": bound}),
    );
    assert_eq!(
        refused(
            handle,
            "switch-away",
            "switch_account",
            json!({"account_id": other})
        ),
        "integration_required"
    );
    assert_eq!(
        call(handle, "kept", "list_accounts", json!({}))["active_account_id"],
        bound
    );
    assert_eq!(public_key(handle), player["public_key"]);
    // active が移っても、開始は結び付けたアカウントで再開する。
    call(
        handle,
        "create-again",
        "create_account",
        json!({"account_id": bound, "operation_id": "00000000-0000-4000-8000-000000000002"}),
    );
    assert_eq!(call(handle, "resume", "start_game_player", game), player);
    assert_eq!(
        call(handle, "active", "list_accounts", json!({}))["active_account_id"],
        bound
    );
    assert_eq!(public_key(handle), player["public_key"]);
    finish(handle);
}

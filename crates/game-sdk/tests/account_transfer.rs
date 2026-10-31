//! 既存の kukuri アカウントを移行で受けて始める入口（ADR 0070 §4.1。#1996 AC-3）。desktop の runtime を移行元にし、
//! SDK は「貼り付け」の経路（リンクを秘密の入力で渡す）で受ける。keyring を使わず loopback だけで動かす。期限切れは
//! 拒否・取消と同じ失敗の経路で、`kukuri-iroh-node` の移行の試験が確かめている。
#![allow(clippy::unwrap_used)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kukuri_desktop_runtime::{DecideAccountTransferRequest, DesktopRuntime};
use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{
    call, consent, error_code, finish, isolated_environment, open_real, request, response_for,
    send, send_secret, wait_for_startup,
};

const GAME: &str = "com.example.cyber-disc-arena";

fn id(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn status(handle: *mut KukuriHandle) -> Value {
    call(
        handle,
        &id("status"),
        "get_account_transfer_status",
        json!({}),
    )
}

fn wait_state(handle: *mut KukuriHandle, state: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let current = status(handle);
        if current["state"] == state {
            return current;
        }
        assert!(Instant::now() < deadline, "{state}: {current}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn paste(handle: *mut KukuriHandle, link: &str) {
    let id = id("open");
    assert_eq!(
        send_secret(
            handle,
            &request(&id, "open_account_transfer", json!({})),
            link.as_bytes()
        ),
        0
    );
    let response = response_for(handle, &id, Duration::from_secs(60));
    assert_eq!(response["ok"], true, "{response}");
}

fn decide(handle: *mut KukuriHandle, accept: bool) {
    call(
        handle,
        &id("decide"),
        "decide_account_transfer",
        json!({"accept": accept}),
    );
}

fn accounts(handle: *mut KukuriHandle) -> Value {
    call(handle, &id("accounts"), "list_accounts", json!({}))
}

fn public_key(handle: *mut KukuriHandle) -> Value {
    call(handle, &id("key"), "get_sync_status", json!({}))["local_author_pubkey"].clone()
}

fn reopen(dir: &std::path::Path) -> *mut KukuriHandle {
    let handle = open_real(dir);
    wait_for_startup(handle);
    let deadline = Instant::now() + Duration::from_secs(30);
    while call(
        handle,
        &id("ready"),
        "get_desktop_startup_status",
        json!({}),
    )["status"]
        != "ready"
    {
        assert!(Instant::now() < deadline, "the profile did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    handle
}

/// 移行元（desktop の runtime）。招待のリンクを出し、確認コードの承認を返す。
struct Source {
    tokio: tokio::runtime::Runtime,
    runtime: DesktopRuntime,
}

impl Source {
    fn start(dir: &std::path::Path) -> Self {
        let tokio = tokio::runtime::Runtime::new().unwrap();
        let runtime = tokio
            .block_on(DesktopRuntime::new(dir.join("source.db")))
            .unwrap();
        Self { tokio, runtime }
    }

    fn link(&self) -> String {
        self.tokio
            .block_on(self.runtime.create_account_transfer_invite())
            .unwrap()
            .link
    }

    fn state(&self) -> Value {
        serde_json::to_value(
            self.tokio
                .block_on(self.runtime.account_transfer_status())
                .unwrap(),
        )
        .unwrap()
    }

    fn wait_state(&self, state: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let current = self.state();
            if current["state"] == state {
                return current;
            }
            assert!(Instant::now() < deadline, "source {state}: {current}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn decide(&self, accept: bool) {
        self.tokio
            .block_on(
                self.runtime
                    .decide_account_transfer(DecideAccountTransferRequest { accept }),
            )
            .unwrap();
    }
}

/// 貼り付け → 両方に出る確認コードが一致 → 両方で承認 → 完了。受けたアカウントの ID を返す。
fn receive(source: &Source, handle: *mut KukuriHandle) -> Value {
    paste(handle, &source.link());
    let target = wait_state(handle, "confirming");
    assert_eq!(target["role"], "target");
    assert_eq!(source.wait_state("confirming")["code"], target["code"]);
    source.decide(true);
    decide(handle, true);
    let completed = wait_state(handle, "completed");
    source.wait_state("completed");
    assert_eq!(completed["history"], Value::Null);
    completed["account_id"].clone()
}

#[test]
fn an_existing_account_is_received_by_paste_and_kept_across_restarts() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let source = Source::start(dir.path());
    let existing = source.runtime.local_author_pubkey();
    let handle = open_real(&dir.path().join("game"));
    wait_for_startup(handle);
    consent(handle);
    let game = json!({"application_id": GAME});
    assert_eq!(
        call(handle, "none", "get_game_player", game.clone()),
        Value::Null
    );

    // 開始の前の移行は、受けたアカウントを足すだけで active を変えない。ゲームが切り替えてから始める。
    let created = public_key(handle);
    let received = receive(&source, handle);
    assert_eq!(public_key(handle), created);
    call(
        handle,
        "switch",
        "switch_account",
        json!({"account_id": received}),
    );
    assert_eq!(public_key(handle), existing);
    let player = call(handle, "start", "start_game_player", game.clone());
    assert_eq!(player["public_key"], existing);
    finish(handle);

    let handle = reopen(&dir.path().join("game"));
    assert_eq!(call(handle, "resume", "start_game_player", game), player);
    assert_eq!(public_key(handle), existing);
    assert_eq!(accounts(handle)["active_account_id"], received);
    finish(handle);
}

#[test]
fn refused_or_cancelled_transfers_keep_the_started_player() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let source = Source::start(dir.path());
    let handle = open_real(&dir.path().join("game"));
    wait_for_startup(handle);
    consent(handle);
    let game = json!({"application_id": GAME});
    let player = call(handle, "start", "start_game_player", game.clone());
    let before = accounts(handle);

    // 「一致しない」で拒否する。
    paste(handle, &source.link());
    wait_state(handle, "confirming");
    decide(handle, false);
    assert_eq!(wait_state(handle, "failed")["reason"], "rejected");
    assert_eq!(accounts(handle), before);

    // 確認コードを出している間に取り消す（移行先の移行は消え、何も保存しない）。
    paste(handle, &source.link());
    wait_state(handle, "confirming");
    call(handle, "cancel", "cancel_account_transfer", json!({}));
    wait_state(handle, "idle");
    assert_eq!(accounts(handle), before);

    // 開始済みで別のアカウントを受けると、profile には加わるが切り替えない（統合が要る）。
    let received = receive(&source, handle);
    assert_eq!(
        accounts(handle)["accounts"].as_array().unwrap().len(),
        before["accounts"].as_array().unwrap().len() + 1
    );
    let switch = id("switch");
    assert_eq!(
        send(
            handle,
            &request(&switch, "switch_account", json!({"account_id": received}))
        ),
        0
    );
    let refused = response_for(handle, &switch, Duration::from_secs(60));
    assert_eq!(error_code(&refused), "conflict", "{refused}");
    assert_eq!(
        refused["error"]["details"]["reason"],
        "integration_required"
    );
    assert_eq!(
        accounts(handle)["active_account_id"],
        before["active_account_id"]
    );
    assert_eq!(public_key(handle), player["public_key"]);
    assert_eq!(call(handle, "resume", "start_game_player", game), player);
    finish(handle);
}

//! #1980 AC-1: ゲームのアセットの発行・保持・取得（ADR 0070 §5.3、F-AST-1・F-AST-3）。発行者・保持端末・新しく取得する
//! 端末の 3 役を、別々の profile の `kukuri-sdk-sidecar.exe` の process で動かす（keyring を使わず loopback だけ）。
//! 別の鍵・改変は core の、古い版で戻さないことは store の試験が確かめる。
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{
    SidecarServer, call, finish, poll_once, request, response_for, seen_json, send,
    sidecar_process::{Sidecar, ready},
};

const APPLICATION: &str = "com.kukuri.test.assets";

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn id() -> String {
    format!("a{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// 中身が毎回違う PNG の fixture。
fn png(dir: &Path, name: &str, size: usize) -> (std::path::PathBuf, Vec<u8>) {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let seed = NEXT.fetch_add(1, Ordering::Relaxed);
    bytes.extend((0..size).map(|index| (index * 31 + seed * 7) as u8));
    let path = dir.join(name);
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

fn publish(handle: *mut KukuriHandle, topic: &str, asset: &str, path: &Path) -> Value {
    call(
        handle,
        &id(),
        "publish_game_asset",
        json!({"topic": topic, "application_id": APPLICATION, "asset_id": asset,
            "format": {"kind": "png"}, "path": path}),
    )
}

fn fetch(handle: *mut KukuriHandle, topic: &str, publisher: &str, asset: &str) -> Value {
    let request_id = id();
    let payload = json!({"topic": topic, "publisher_pubkey": publisher,
        "application_id": APPLICATION, "asset_id": asset});
    // 取得は大きさに合わせて長くかかりうるので、要求の期限を延ばす。
    let mut envelope: Value =
        serde_json::from_slice(&request(&request_id, "fetch_game_asset", payload)).unwrap();
    envelope["timeout_ms"] = json!(120_000);
    assert_eq!(send(handle, envelope.to_string().as_bytes()), 0);
    response_for(handle, &request_id, Duration::from_secs(150))
}

/// これまでに受け取った event に `pick` が値を返すものが届くまで poll する。
fn wait_event<T>(
    handle: *mut KukuriHandle,
    what: &str,
    mut pick: impl FnMut(&Value) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        poll_once(handle, 64).unwrap();
        if let Some(found) = seen_json(handle)
            .iter()
            .filter_map(|raw| serde_json::from_slice::<Value>(raw).ok())
            .find_map(|json| pick(&json))
        {
            return found;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_published_asset_reaches_a_new_player_from_a_holder_after_the_publisher_stops() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let fixtures = tempfile::tempdir().unwrap();
    let mut publisher_process = Some(Sidecar::start(dirs[0].path()));
    let (holder_process, newcomer_process) = (
        Sidecar::start(dirs[1].path()),
        Sidecar::start(dirs[2].path()),
    );
    let handles: Vec<_> = dirs
        .iter()
        .map(|dir| SidecarServer::connect(dir.path()))
        .collect();
    let (publisher, holder, newcomer) = (handles[0], handles[1], handles[2]);
    let topic = format!("kukuri:topic:test-game-assets-{}", std::process::id());
    let mut tickets = Vec::new();
    for &handle in &handles {
        ready(handle);
        call(
            handle,
            &id(),
            "set_topic_gossip_enabled",
            json!({"topic": topic, "enabled": true}),
        );
        tickets.push(call(handle, &id(), "get_local_peer_ticket", json!({})));
    }
    for (index, &handle) in handles.iter().enumerate() {
        for (other, ticket) in tickets.iter().enumerate() {
            if other != index {
                call(
                    handle,
                    &id(),
                    "import_peer_ticket",
                    json!({"ticket": ticket}),
                );
            }
        }
    }
    let publisher_key = call(publisher, &id(), "get_sync_status", json!({}))["local_author_pubkey"]
        .as_str()
        .unwrap()
        .to_owned();

    // 発行: 3 つのアセットを版 1 で。ゲームの Player 役（FFI）は発行できないことは c_abi の試験が確かめる。
    let (skin, skin_bytes) = png(fixtures.path(), "skin.png", 4096);
    let (board, board_bytes) = png(fixtures.path(), "board.png", 2048);
    let (banner, _) = png(fixtures.path(), "banner.png", 1024);
    assert_eq!(publish(publisher, &topic, "skin", &skin)["revision"], 1);
    assert_eq!(publish(publisher, &topic, "board", &board)["revision"], 1);
    assert_eq!(publish(publisher, &topic, "banner", &banner)["revision"], 1);

    // 保持端末が skin と board を取得・適用する（F-AST-1）。file は発行した bytes と同じ。
    for (asset, bytes) in [("skin", &skin_bytes), ("board", &board_bytes)] {
        let fetched = fetch(holder, &topic, &publisher_key, asset);
        assert_eq!(fetched["ok"], true, "{fetched}");
        assert_eq!(fetched["data"]["applied_revision"], 1);
        assert_eq!(
            &std::fs::read(fetched["data"]["path"].as_str().unwrap()).unwrap(),
            bytes
        );
    }
    let listed = call(
        holder,
        &id(),
        "list_game_assets",
        json!({"topic": topic,
        "publisher_pubkey": publisher_key, "application_id": APPLICATION,
        "asset_ids": ["skin", "board", "banner"]}),
    );
    let applied: Vec<_> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|release| release["applied_revision"].clone())
        .collect();
    assert_eq!(applied, vec![json!(1), json!(1), Value::Null]);

    // 新しい版を発行すると、保持端末は自動で取得・適用し、path 付きで知らせる。
    let (skin2, skin2_bytes) = png(fixtures.path(), "skin2.png", 8192);
    assert_eq!(publish(publisher, &topic, "skin", &skin2)["revision"], 2);
    let updated = wait_event(holder, "the holder did not update the skin", |event| {
        (event["type"] == "game_asset_updated"
            && event["asset_id"] == "skin"
            && event["revision"] == 2)
            .then(|| event["path"].as_str().map(str::to_owned))
            .flatten()
    });
    assert_eq!(std::fs::read(&updated).unwrap(), skin2_bytes);

    // 発行者を止めても、新しい端末は保持端末から取得できる。
    assert_eq!(
        fetch(newcomer, &topic, &publisher_key, "skin")["data"]["revision"],
        2
    );
    assert!(
        publisher_process
            .take()
            .unwrap()
            .stop(dirs[0].path())
            .success()
    );
    let from_holder = fetch(newcomer, &topic, &publisher_key, "board");
    assert_eq!(from_holder["ok"], true, "{from_holder}");
    assert_eq!(
        std::fs::read(from_holder["data"]["path"].as_str().unwrap()).unwrap(),
        board_bytes
    );

    // 誰も持っていないアセットは、取得できないことを返す（成功扱いにしない。F-AST-3）。
    let missing = fetch(newcomer, &topic, &publisher_key, "banner");
    assert_eq!(missing["ok"], false, "{missing}");
    assert!(
        ["unavailable", "not_found"]
            .contains(&missing["error"]["details"]["reason"].as_str().unwrap_or("")),
        "{missing}"
    );

    for handle in handles {
        finish(handle);
    }
    assert!(holder_process.stop(dirs[1].path()).success());
    assert!(newcomer_process.stop(dirs[2].path()).success());
}

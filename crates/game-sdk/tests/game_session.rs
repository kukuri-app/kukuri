//! #1979 AC-1: SDK の action session の packet（ADR 0070 §3、F-ABI-10・F-ABI-16・F-SES-2・F-SES-4）。2 つの実際の
//! profile で、同じ手順を FFI と Sidecar（Windows）の受け手へ流す。
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use kukuri_game_sdk::{
    BINARY_DATAGRAM, BINARY_STREAM, KukuriHandle, MAX_UNRELEASED_BYTES, Status,
    kukuri_sdk_batch_release, kukuri_sdk_close,
};
use serde_json::{Value, json};
use support::{
    Polled, call, consent, finish, introduce, isolated_environment, open_real, poll_hold,
    poll_once, request, response_for, send, send_binary, subscribe, wait_for,
};

const OK: i32 = Status::Ok as i32;
const INVALID: i32 = Status::InvalidArgument as i32;
const BACKPRESSURE: i32 = Status::Backpressure as i32;
const CLOSED: i32 = Status::Closed as i32;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn id() -> String {
    format!("r{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn numbered(index: usize, len: usize) -> Vec<u8> {
    let mut bytes = (index as u32).to_be_bytes().to_vec();
    bytes.resize(len, 0x5a);
    bytes
}

/// packet の event と続く bytes の組（session ID、配送、bytes）。
fn packets(items: &[Polled]) -> Vec<(String, String, Vec<u8>)> {
    items
        .windows(2)
        .filter(|pair| pair[0].kind == 3 && pair[0].json["type"] == "game_session_packet")
        .map(|pair| {
            assert_eq!(pair[1].kind, 4, "the bytes follow the packet event");
            (
                pair[0].json["session_id"].as_str().unwrap().to_owned(),
                pair[0].json["delivery"].as_str().unwrap().to_owned(),
                pair[1].raw.clone(),
            )
        })
        .collect()
}

/// 同意済みの 2 つの profile をつなぎ、host の room に guest を参加させて session ID を返す。
fn join(host: *mut KukuriHandle, guest: *mut KukuriHandle, topic: &str) -> String {
    for handle in [host, guest] {
        consent(handle);
        subscribe(handle, topic);
    }
    introduce(host, guest);
    let room = call(
        host,
        &id(),
        "create_game_room",
        json!({
            "topic": topic,
            "channel_ref": {"kind": "public"},
            "title": "sdk",
            "description": "",
            "participants": [],
            "action": {"application_id": "com.kukuri.test.sdk", "max_players": 4},
        }),
    );
    // room が guest の手元に届くまでは断られる。
    let deadline = Instant::now() + Duration::from_secs(120);
    let session = loop {
        let request_id = id();
        let payload = json!({"topic": topic, "scope": {"kind": "public"}, "room_id": room});
        assert_eq!(
            send(guest, &request(&request_id, "join_game_session", payload)),
            OK
        );
        let response = response_for(guest, &request_id, Duration::from_secs(60));
        if response["ok"] == true {
            break response["data"]["session_id"].as_str().unwrap().to_owned();
        }
        assert!(Instant::now() < deadline, "join: {}", response["error"]);
        std::thread::sleep(Duration::from_millis(500));
    };
    wait_for(host, "the host opens the session", |json, _| {
        (json["type"] == "game_session_opened" && json["session_id"] == session.as_str())
            .then_some(())
    });
    session
}

/// 受け手が batch を解放しないまま 64 MiB に達すると packet を受け取らなくなり（応答は届く）、解放すると stream の
/// 残りが順に届く。その間の poll の時間を返す。
fn unreleased_limit(
    host: *mut KukuriHandle,
    guest: *mut KukuriHandle,
    session: &str,
) -> Vec<Duration> {
    let stop = Arc::new(AtomicBool::new(false));
    let sender = std::thread::spawn({
        let (host, session, stop) = (host as usize, session.to_owned(), stop.clone());
        move || {
            let mut sent = 0;
            while !stop.load(Ordering::Relaxed) {
                match send_binary(
                    host as *mut _,
                    BINARY_STREAM,
                    &session,
                    &numbered(sent, 64 * 1024),
                ) {
                    OK => sent += 1,
                    BACKPRESSURE => std::thread::sleep(Duration::from_millis(1)),
                    other => panic!("send_binary: {other}"),
                }
            }
        }
    });
    let mut held = Vec::new();
    let mut held_bytes = 0;
    let mut stream = Vec::new();
    let mut times = Vec::new();
    let collect = |items: &[Polled], stream: &mut Vec<usize>| {
        for (_, delivery, bytes) in packets(items) {
            if delivery == "stream" {
                stream.push(u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize);
            }
        }
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    while held_bytes < MAX_UNRELEASED_BYTES {
        assert!(Instant::now() < deadline, "held {held_bytes} bytes");
        let started = Instant::now();
        let polled = poll_hold(guest, 64, 1 << 20);
        times.push(started.elapsed());
        match polled {
            Some((batch, items)) => {
                held_bytes += items.iter().map(|item| item.raw.len()).sum::<usize>();
                collect(&items, &mut stream);
                held.push(batch);
            }
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    // 上限に達した後は、新しい datagram は捨て、stream は止めたまま。応答は届く。
    assert_eq!(send_binary(host, BINARY_DATAGRAM, session, b"during"), OK);
    let request_id = id();
    assert_eq!(
        send(guest, &request(&request_id, "get_sync_status", json!({}))),
        OK
    );
    let mut answered = false;
    let quiet = Instant::now() + Duration::from_millis(800);
    while Instant::now() < quiet || !answered {
        assert!(
            Instant::now() < deadline,
            "no response while over the limit"
        );
        if let Some((batch, items)) = poll_hold(guest, 64, 1 << 20) {
            assert!(
                items.iter().all(|item| item.kind != 4),
                "no packet over the limit"
            );
            answered |= items
                .iter()
                .any(|item| item.kind == 1 && item.json["request_id"] == request_id.as_str());
            held.push(batch);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for batch in held {
        assert_eq!(unsafe { kukuri_sdk_batch_release(guest, batch) }, OK);
    }
    let resumed = stream.len() + 8;
    while stream.len() < resumed {
        assert!(Instant::now() < deadline, "the stream did not resume");
        let items = poll_once(guest, 64).unwrap();
        assert!(
            packets(&items)
                .iter()
                .all(|(_, _, bytes)| bytes != b"during"),
            "datagrams over the limit are dropped"
        );
        collect(&items, &mut stream);
    }
    stop.store(true, Ordering::Relaxed);
    sender.join().unwrap();
    assert_eq!(
        stream,
        (0..stream.len()).collect::<Vec<_>>(),
        "the stream loses nothing"
    );
    times
}

fn scenario(host: *mut KukuriHandle, guest: *mut KukuriHandle, topic: &str) {
    let session = join(host, guest, topic);

    // 両方の向きで datagram と stream が届く。
    assert_eq!(send_binary(guest, BINARY_STREAM, &session, b"catch"), OK);
    assert_eq!(send_binary(guest, BINARY_DATAGRAM, &session, b"move"), OK);
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while received.len() < 2 {
        assert!(Instant::now() < deadline, "the host received {received:?}");
        received.extend(packets(&poll_once(host, 64).unwrap()));
    }
    received.sort();
    assert_eq!(
        received,
        vec![
            (session.clone(), "datagram".into(), b"move".to_vec()),
            (session.clone(), "stream".into(), b"catch".to_vec()),
        ]
    );
    assert_eq!(send_binary(host, BINARY_DATAGRAM, &session, b"state"), OK);
    wait_for(guest, "the guest receives the state", |_, raw| {
        (raw == b"state").then_some(())
    });

    // F-SES-4 と引数: 上限を超える大きさ・不明な kind は送らない。知らない session は閉じたものと同じ。
    assert_eq!(
        send_binary(guest, BINARY_DATAGRAM, &session, &[0; 1101]),
        INVALID
    );
    assert_eq!(
        send_binary(guest, BINARY_STREAM, &session, &vec![0; 64 * 1024 + 1]),
        INVALID
    );
    assert_eq!(send_binary(guest, 9, &session, b"x"), INVALID);
    assert_eq!(
        send_binary(guest, BINARY_STREAM, "gs-unknown", b"x"),
        CLOSED
    );

    // F-ABI-16 と、大量の転送の間の poll の時間（ADR 0070 §8.4・§8.5 の 1 frame の予算は 1 ms）。
    let mut times = unreleased_limit(host, guest, &session);
    times.sort();
    let p99 = times[times.len() * 99 / 100];
    eprintln!(
        "poll during the bulk transfer: p99 {p99:?} over {} polls",
        times.len()
    );
    assert!(p99 <= Duration::from_millis(1), "poll p99 {p99:?}");

    // F-SES-2: 退出した session は、どちらの端末からも送れない。
    call(
        guest,
        &id(),
        "leave_game_session",
        json!({"session_id": session}),
    );
    let reason = wait_for(guest, "the guest's session closes", |json, _| {
        (json["type"] == "game_session_closed" && json["session_id"] == session.as_str())
            .then(|| json["reason"].clone())
    });
    assert_eq!(reason, Value::from("left"));
    assert_eq!(send_binary(guest, BINARY_STREAM, &session, b"late"), CLOSED);
    let deadline = Instant::now() + Duration::from_secs(30);
    while send_binary(host, BINARY_STREAM, &session, b"late") != CLOSED {
        assert!(Instant::now() < deadline, "the host still sends");
        std::thread::sleep(Duration::from_millis(50));
    }

    // F-ABI-10: close の後の send_binary は CLOSED。
    unsafe { kukuri_sdk_close(guest) };
    assert_eq!(send_binary(guest, BINARY_STREAM, &session, b"late"), CLOSED);
}

#[test]
fn packets_flow_through_the_ffi_handle_within_the_limits() {
    isolated_environment();
    let (host_dir, guest_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = open_real(host_dir.path());
    let guest = open_real(guest_dir.path());
    scenario(host, guest, "kukuri:topic:test-sdk-packets-ffi");
    finish(guest);
    finish(host);
}

#[cfg(windows)]
#[test]
fn packets_flow_through_the_sidecar_handle_within_the_limits() {
    use kukuri_game_sdk::{Mode, SdkClient, SdkConfig, kukuri_sdk_free};
    isolated_environment();
    let (host_dir, guest_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = open_real(host_dir.path());
    let server = support::SidecarServer::start(
        SdkClient::open(SdkConfig {
            profile: support::PROFILE.into(),
            profile_dir: guest_dir.path().to_path_buf(),
            mode: Mode::Ffi,
        })
        .unwrap(),
    );
    scenario(host, server.handle, "kukuri:topic:test-sdk-packets-sidecar");
    let guest = server.handle;
    drop(server);
    unsafe { kukuri_sdk_free(guest) };
    finish(host);
}

//! #1982 AC-1: SDK の Live の映像・音声（ADR 0070 §8.3、F-LIV-1・F-LIV-2・F-LIV-5）。ゲームと desktop の 2 つの profile の
//! 間で、配信する側が RGBA の frame とゲームの音を送り、視聴する側が復号した I420 の frame と音声の再生時刻を受ける。
//! 向きを入れ替えて 2 回行う。frame の上半分に frame の番号を 8 本の白黒の帯で描き、受けた frame から読んで遅延を測る。
//! #1982 AC-2: 配信者 1・視聴者 5 で、直接の枠（4）を超えた 5 人目が視聴者の転送で受け、親が抜けても受け続ける。
#![cfg(feature = "vp8")]
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    collections::HashMap,
    f32::consts::TAU,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use kukuri_game_sdk::{BINARY_LIVE_AUDIO, BINARY_LIVE_VIDEO, KukuriHandle, Status};
use serde_json::json;
use support::{
    call, consent, finish, introduce, isolated_environment, open_real, poll_once, send_binary,
    subscribe,
};

const TOPIC: &str = "kukuri:topic:test-game-sdk-live";
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const OK: i32 = Status::Ok as i32;
const CLOSED: i32 = Status::Closed as i32;

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn id() -> String {
    format!("l{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// 下の行から並んだ RGBA。上半分は番号の 8 bit を左から白（1）・黒（0）の帯で、下半分は青。
fn frame(index: u8) -> Vec<u8> {
    let mut bytes = (WIDTH as u16).to_le_bytes().to_vec();
    bytes.extend_from_slice(&(HEIGHT as u16).to_le_bytes());
    for row in (0..HEIGHT).rev() {
        for column in 0..WIDTH {
            let color = if row >= HEIGHT / 2 {
                [20, 20, 230, 255]
            } else if index >> (7 - column * 8 / WIDTH) & 1 == 1 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            };
            bytes.extend_from_slice(&color);
        }
    }
    bytes
}

/// 上から並んだ I420 の上半分の帯から番号を読む。
fn read_index(i420: &[u8]) -> u8 {
    (0..8).fold(0, |index, bit| {
        let x = bit * WIDTH / 8 + WIDTH / 16;
        let luma = i420[(HEIGHT / 4 * WIDTH + x) as usize];
        index << 1 | u8::from(luma > 128)
    })
}

/// 20 ms 分の 48 kHz・stereo の正弦波。
fn audio(start: usize) -> Vec<u8> {
    let mut bytes = 48_000u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&2u16.to_le_bytes());
    for sample in start..start + 960 {
        let value = (TAU * 440.0 * sample as f32 / 48_000.0).sin() * 0.2;
        for _ in 0..2 {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn session(handle: *mut KukuriHandle, title: &str) -> String {
    call(
        handle,
        &id(),
        "create_live_session",
        json!({"topic": TOPIC, "title": title, "description": ""}),
    )
    .as_str()
    .unwrap()
    .to_owned()
}

/// 視聴を始め、音量を 0 にする（試験で speaker から音を出さない。復号・出力・再生時刻は動く）。
fn watch(viewer: *mut KukuriHandle, session_id: &str) {
    media(viewer, "watch_live_media", session_id);
    call(
        viewer,
        &id(),
        "set_live_volume",
        json!({"session_id": session_id, "volume": 0.0}),
    );
}

fn media(handle: *mut KukuriHandle, command: &str, session_id: &str) {
    call(
        handle,
        &id(),
        command,
        json!({"topic": TOPIC, "session_id": session_id}),
    );
}

/// 受けた frame（event と bytes）と視聴の状態。
#[derive(Default)]
struct Received {
    states: Vec<String>,
    /// 番号ごとの最初の受信（frame の timestamp、音声の再生時刻、受けた時刻）。
    frames: HashMap<u8, (u64, u64, Instant)>,
    last_i420: Vec<u8>,
    /// 最後に受けた frame の、配信者からの段数（1 は直接、2 以上は転送）。
    hops: u64,
}

impl Received {
    fn poll(&mut self, handle: *mut KukuriHandle) {
        let items = poll_once(handle, 64).unwrap();
        for pair in items.windows(2) {
            let event = &pair[0].json;
            if pair[0].kind == 3 && event["type"] == "live_video_frame" {
                assert_eq!(pair[1].kind, 4, "the bytes follow the frame event");
                assert_eq!(
                    (event["width"].as_u64(), event["height"].as_u64()),
                    (Some(u64::from(WIDTH)), Some(u64::from(HEIGHT)))
                );
                let index = read_index(&pair[1].raw);
                let timestamp = event["timestamp_us"].as_u64().unwrap();
                let clock = event["audio_clock_us"].as_u64().unwrap();
                self.frames
                    .entry(index)
                    .or_insert((timestamp, clock, Instant::now()));
                self.last_i420 = pair[1].raw.clone();
                self.hops = event["hops"].as_u64().unwrap();
            }
        }
        for item in &items {
            if item.kind == 3 && item.json["type"] == "live_watch_status" {
                self.states
                    .push(item.json["state"].as_str().unwrap().to_owned());
            }
        }
    }
}

/// 視聴する側は参加して見るだけで、投稿・反応・自分の配信をしない。live session が届くまで参加をやり直す。
fn join(viewer: *mut KukuriHandle, live: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let request = id();
        support::send(
            viewer,
            &support::request(
                &request,
                "join_live_session",
                json!({"topic": TOPIC, "session_id": live}),
            ),
        );
        if support::response_for(viewer, &request, Duration::from_secs(30))["ok"] == true {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the live session did not reach the viewer"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `broadcaster` が `live` へ番号 `index` の frame と 20 ms の音を送り、視聴者の受信を poll する（30 fps の 1 回分）。
fn tick(
    broadcaster: *mut KukuriHandle,
    live: &str,
    index: &mut u8,
    viewers: &mut [(*mut KukuriHandle, Received)],
) {
    *index = index.wrapping_add(1).max(1);
    assert_eq!(
        send_binary(broadcaster, BINARY_LIVE_VIDEO, live, &frame(*index)),
        OK
    );
    for _ in 0..2 {
        let _ = send_binary(broadcaster, BINARY_LIVE_AUDIO, live, &audio(0));
    }
    for (handle, received) in viewers.iter_mut() {
        received.poll(*handle);
    }
    std::thread::sleep(Duration::from_millis(33));
}

/// 配信を続けながら、`done` が満たされるまで待つ。
fn broadcast_until(
    broadcaster: *mut KukuriHandle,
    live: &str,
    index: &mut u8,
    viewers: &mut [(*mut KukuriHandle, Received)],
    what: &str,
    done: impl Fn(&[(*mut KukuriHandle, Received)]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !done(viewers) {
        assert!(Instant::now() < deadline, "{what}");
        tick(broadcaster, live, index, viewers);
    }
}

/// `broadcaster` が配信し `viewer` が視聴する。
fn broadcast_and_watch(broadcaster: *mut KukuriHandle, viewer: *mut KukuriHandle, title: &str) {
    let live = session(broadcaster, title);
    // 明示的に配信を始める前は、映像も音も受け付けない（取り込んだものを送らない）。
    assert_eq!(
        send_binary(broadcaster, BINARY_LIVE_VIDEO, &live, &frame(0)),
        CLOSED
    );
    assert_eq!(
        send_binary(broadcaster, BINARY_LIVE_AUDIO, &live, &audio(0)),
        CLOSED
    );
    let status = call(
        broadcaster,
        &id(),
        "set_live_broadcast",
        json!({"topic": TOPIC, "session_id": live, "source": "camera", "microphone": false}),
    );
    assert_eq!(status["limits"]["width"], 1280);
    // 配信者は最初の keyframe を置くまで視聴を断る（`no_media`）ので、配信中の live を見に来る順にする。
    assert_eq!(
        send_binary(broadcaster, BINARY_LIVE_VIDEO, &live, &frame(0)),
        OK
    );
    std::thread::sleep(Duration::from_millis(500));
    join(viewer, &live);
    watch(viewer, &live);

    let mut received = Received::default();
    let mut sent = HashMap::new();
    let started = Instant::now();
    let mut audio_sent = 0;
    for index in 0..=255u8 {
        let at = started + Duration::from_millis(u64::from(index) * 33);
        while Instant::now() < at {
            received.poll(viewer);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            send_binary(broadcaster, BINARY_LIVE_VIDEO, &live, &frame(index)),
            OK
        );
        sent.insert(index, Instant::now());
        while audio_sent * 20 < started.elapsed().as_millis() as usize {
            assert_eq!(
                send_binary(
                    broadcaster,
                    BINARY_LIVE_AUDIO,
                    &live,
                    &audio(audio_sent * 960)
                ),
                OK
            );
            audio_sent += 1;
        }
    }
    let settle = Instant::now() + Duration::from_secs(2);
    while Instant::now() < settle {
        received.poll(viewer);
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        received.states.contains(&"watching".to_owned()),
        "{:?}",
        received.states
    );
    // 番号 0 の frame は送る前に断られた分と区別できないので数えない。後半の frame が届いていること。
    let late: Vec<u8> = received
        .frames
        .keys()
        .copied()
        .filter(|index| *index > 128)
        .collect();
    assert!(late.len() > 30, "only {} later frames arrived", late.len());
    // 下半分は青（I420 の V が小さい）。
    let v = received.last_i420
        [(WIDTH * HEIGHT + WIDTH * HEIGHT / 4 + (HEIGHT - 20) / 2 * WIDTH / 2) as usize];
    assert!(v < 125, "the lower half is blue ({v})");
    // 音声が再生され、描く frame の時刻 − 音声の再生時刻が −200〜+125 ms に入る（p95。2026-10-31 のユーザー判断）。
    let played: Vec<_> = received
        .frames
        .values()
        .filter(|(_, clock, _)| *clock > 0)
        .collect();
    assert!(!played.is_empty(), "no frame came with an audio clock");
    let in_sync = played
        .iter()
        .filter(|(timestamp, clock, _)| {
            (-200_000..=125_000).contains(&(*timestamp as i64 - *clock as i64))
        })
        .count();
    assert!(
        in_sync * 100 >= played.len() * 95,
        "audio and video drifted: {in_sync} of {} frames in sync",
        played.len()
    );
    // 取り込み（送った時刻）から受けた時刻までの遅延（直接の視聴、p95 ≤ 500 ms。ADR 0070 §8.4）。
    let mut delays: Vec<u128> = received
        .frames
        .iter()
        .filter(|(index, _)| **index > 0)
        .map(|(index, (_, _, at))| at.duration_since(sent[index]).as_millis())
        .collect();
    delays.sort_unstable();
    let p95 = delays[delays.len() * 95 / 100];
    eprintln!(
        "KGS_LIVE {title}: frames={} p95_delay_ms={p95} audio_frames={}",
        delays.len(),
        played.len()
    );
    assert!(p95 <= 500, "p95 delay {p95} ms");

    // 視聴を止めたら frame は届かない。配信を止めたら映像も音も受け付けない。
    media(viewer, "stop_watching_live_media", &live);
    received.poll(viewer);
    let before = received.frames.len();
    for index in 0..30u8 {
        let _ = send_binary(broadcaster, BINARY_LIVE_VIDEO, &live, &frame(index));
        std::thread::sleep(Duration::from_millis(33));
        received.poll(viewer);
    }
    assert_eq!(
        received.frames.len(),
        before,
        "no frame after stopping the watch"
    );
    media(broadcaster, "stop_live_broadcast", &live);
    assert_eq!(
        send_binary(broadcaster, BINARY_LIVE_VIDEO, &live, &frame(1)),
        CLOSED
    );
    assert_eq!(
        send_binary(broadcaster, BINARY_LIVE_AUDIO, &live, &audio(0)),
        CLOSED
    );
}

/// 配信者 1・視聴者 5（それぞれ別の profile）。直接の枠は 4 人なので、5 人目は視聴者の転送で受ける（段数 2 以上）。親が
/// 視聴をやめても受け続け（参加し直す）、配信が止まると視聴の状態が変わる。視聴者は投稿・反応・自分の配信をしない。
#[test]
fn the_fifth_viewer_is_relayed_and_keeps_watching_when_its_parent_leaves() {
    isolated_environment();
    let dirs: Vec<_> = (0..6).map(|_| tempfile::tempdir().unwrap()).collect();
    let handles: Vec<_> = dirs.iter().map(|dir| open_real(dir.path())).collect();
    for &handle in &handles {
        consent(handle);
        subscribe(handle, TOPIC);
    }
    for (at, &left) in handles.iter().enumerate() {
        for &right in &handles[at + 1..] {
            introduce(left, right);
        }
    }
    let broadcaster = handles[0];
    let live = session(broadcaster, "test: relay");
    call(
        broadcaster,
        &id(),
        "set_live_broadcast",
        json!({"topic": TOPIC, "session_id": live, "source": "camera", "microphone": false}),
    );
    let mut index = 0u8;
    let mut viewers: Vec<(*mut KukuriHandle, Received)> = Vec::new();
    for _ in 0..3 {
        tick(broadcaster, &live, &mut index, &mut viewers);
    }
    // 直接の枠の 4 人が順に見始め、配信者から直接受ける。
    for &viewer in &handles[1..5] {
        join(viewer, &live);
        watch(viewer, &live);
        viewers.push((viewer, Received::default()));
        broadcast_until(
            broadcaster,
            &live,
            &mut index,
            &mut viewers,
            "a direct viewer did not receive",
            |viewers| viewers.last().unwrap().1.hops == 1,
        );
    }
    // 5 人目は満員の配信者に断られ、直接の視聴者の転送で受ける。
    let fifth = handles[5];
    join(fifth, &live);
    watch(fifth, &live);
    viewers.push((fifth, Received::default()));
    broadcast_until(
        broadcaster,
        &live,
        &mut index,
        &mut viewers,
        "the fifth viewer was not relayed",
        |viewers| viewers[4].1.hops >= 2,
    );
    assert!(viewers[..4].iter().all(|(_, received)| received.hops == 1));
    // 直接の視聴者が順に視聴をやめても、5 人目は受け続け（親を失ったら参加し直す）、空いた直接の枠で受ける。
    for &viewer in &handles[1..5] {
        media(viewer, "stop_watching_live_media", &live);
    }
    viewers.clear();
    viewers.push((fifth, Received::default()));
    broadcast_until(
        broadcaster,
        &live,
        &mut index,
        &mut viewers,
        "the fifth viewer did not recover",
        |viewers| viewers[0].1.hops == 1 && viewers[0].1.frames.len() > 30,
    );
    // 配信が止まると、視聴の状態が watching から変わる（画面は終わったことを示せる）。
    media(broadcaster, "stop_live_broadcast", &live);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !viewers[0].1.states.iter().any(|state| state != "watching") {
        assert!(
            Instant::now() < deadline,
            "the end of the broadcast was not shown"
        );
        viewers[0].1.poll(fifth);
        std::thread::sleep(Duration::from_millis(50));
    }
    for handle in handles {
        finish(handle);
    }
}

#[test]
fn game_and_desktop_broadcast_and_watch_each_other() {
    isolated_environment();
    let (game_dir, desktop_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let game = open_real(game_dir.path());
    let desktop = open_real(desktop_dir.path());
    for handle in [game, desktop] {
        consent(handle);
        subscribe(handle, TOPIC);
    }
    introduce(game, desktop);
    broadcast_and_watch(game, desktop, "test: game to desktop");
    broadcast_and_watch(desktop, game, "test: desktop to game");
    for handle in [game, desktop] {
        finish(handle);
    }
}

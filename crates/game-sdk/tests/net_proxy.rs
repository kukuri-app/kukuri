//! #1984 AC-1: 性能の計測に使う UDP proxy（ADR 0070 §8.4）。`KUKURI_TEST_UDP_PROXY` を指定した 2 つの profile は、
//! proxy を通してだけ届き、片道の遅延と blackout がかかり、proxy は送り手と宛先の組ごとに bytes を数える。
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use kukuri_game_sdk::{BINARY_STREAM, KukuriHandle, Status};
use kukuri_transport::UDP_PROXY_ENV;
use serde_json::json;
use support::{call, finish, isolated_environment, join, open_real, send_binary, wait_for};

fn receive(handle: *mut KukuriHandle, bytes: &'static [u8]) {
    wait_for(handle, "the packet", |_, raw| (raw == bytes).then_some(()));
}

/// 試験が途中で失敗しても proxy を残さない。
struct Proxy(std::process::Child);

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn endpoint_id(handle: *mut KukuriHandle) -> String {
    let ticket = call(handle, "ticket", "get_local_peer_ticket", json!({}));
    ticket
        .as_str()
        .unwrap()
        .split('@')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn two_profiles_reach_each_other_only_through_the_measurement_proxy() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listen = format!("127.0.0.1:{port}");
    let stats = dir.path().join("stats.txt");
    let mut proxy = Proxy(
        Command::new(env!("CARGO_BIN_EXE_kukuri-sdk-net-proxy"))
            .args(["--listen", &listen, "--loss", "0", "--stats"])
            .arg(&stats)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(proxy.0.stdout.take().unwrap()).lines();
    let frequency: f64 = output.next().unwrap().unwrap()["frequency ".len()..]
        .parse()
        .unwrap();
    // ticket の port は使われない（相手は EndpointId で proxy へ送る）が、ticket を作るのに要る。
    unsafe {
        std::env::set_var(UDP_PROXY_ENV, &listen);
        std::env::set_var("KUKURI_ADVERTISE_PORT", port.to_string());
    }
    let (host_dir, guest_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (host, guest) = (open_real(host_dir.path()), open_real(guest_dir.path()));
    let session = join(host, guest, "kukuri:topic:test-sdk-net-proxy");
    let ok = Status::Ok as i32;

    // 片道は 22.5 ms 以上かかる。
    let sent = Instant::now();
    assert_eq!(send_binary(guest, BINARY_STREAM, &session, b"one way"), ok);
    receive(host, b"one way");
    assert!(sent.elapsed() >= Duration::from_micros(22_500));

    // blackout の間に送った stream は、明けてから届く。
    writeln!(proxy.0.stdin.as_mut().unwrap(), "blackout 1000").unwrap();
    let line = output.next().unwrap().unwrap();
    let ticks: Vec<f64> = line["blackout ".len()..]
        .split(' ')
        .map(|value| value.parse().unwrap())
        .collect();
    assert!(
        ((ticks[1] - ticks[0]) / frequency - 1.0).abs() < 0.001,
        "{line}"
    );
    let sent = Instant::now();
    assert_eq!(send_binary(guest, BINARY_STREAM, &session, b"after"), ok);
    receive(host, b"after");
    assert!(sent.elapsed() >= Duration::from_millis(900));

    // 送り手と宛先の組ごとに、両方向の bytes と blackout で落とした packet が数えられる。
    let (host_id, guest_id) = (endpoint_id(host), endpoint_id(guest));
    std::thread::sleep(Duration::from_millis(1500));
    let text = std::fs::read_to_string(&stats).unwrap();
    let last = |source: &str, destination: &str| {
        text.lines()
            .rev()
            .map(|line| line.split(' ').collect::<Vec<_>>())
            .find(|fields| fields[1] == source && fields[2] == destination)
            .map(|fields| {
                fields[3..]
                    .iter()
                    .map(|value| value.parse().unwrap())
                    .collect::<Vec<u64>>()
            })
            .unwrap_or_else(|| panic!("no traffic from {source} to {destination}"))
    };
    let (to_host, to_guest) = (last(&guest_id, &host_id), last(&host_id, &guest_id));
    assert!(to_host[0] > 0 && to_guest[0] > 0);
    assert!(
        to_host[3] > 0,
        "the blackout dropped the stream: {to_host:?}"
    );
    assert_eq!(to_host[2] + to_guest[2], 0, "--loss 0 loses nothing");

    finish(guest);
    finish(host);
}

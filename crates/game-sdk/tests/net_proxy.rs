//! #1984 AC-1: 性能の計測に使う UDP proxy（ADR 0070 §8.4）。`KUKURI_TEST_UDP_PROXY` を指定した 2 つの profile は、
//! proxy を通してだけ届き、片道の遅延・損失・blackout がかかり、proxy は送り手と宛先の組ごとに bytes を数える。§8.4 の
//! 条件（片道 22.5〜27.5 ms・損失 1%）でも、5 MiB を 3 秒以内に送れる（輻輳制御が BBR3 で、順序の入れ替わりと遅延の
//! ゆらぎを損失とみなさない。Cubic では 16 秒ほど、後の 3 packet で損失とみなす既定では 2〜7 秒にばらつき、往復時間の
//! 9/8 で損失とみなす既定では負荷があると 14 秒かかる回がある）。
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
            .args(["--listen", &listen, "--stats"])
            .arg(&stats)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(proxy.0.stdout.take().unwrap()).lines();
    // 最初の行（`frequency`）が出たら受け付けている。
    output.next().unwrap().unwrap();
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
    assert!(output.next().unwrap().unwrap().starts_with("blackout "));
    let sent = Instant::now();
    assert_eq!(send_binary(guest, BINARY_STREAM, &session, b"after"), ok);
    receive(host, b"after");
    // 行を読むまでの間の分だけ、1 秒より短くなりうる。
    assert!(sent.elapsed() >= Duration::from_millis(800));

    // 5 MiB を stream で送る（受け手を poll しながら、送り待ちが溢れたら送り直す）。
    let chunk = vec![0x5a; 60 * 1024];
    let total = 5 << 20;
    let (mut sent, mut received) = (0, 0);
    let started = Instant::now();
    while received < total {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{received} of {total}"
        );
        let len = chunk.len().min(total - sent);
        if len > 0 && send_binary(guest, BINARY_STREAM, &session, &chunk[..len]) == ok {
            sent += len;
        }
        received += support::poll_once(host, 64)
            .unwrap()
            .iter()
            .filter(|item| item.kind == 4)
            .map(|item| item.raw.len())
            .sum::<usize>();
        std::thread::sleep(Duration::from_millis(1));
    }
    let elapsed = started.elapsed();
    // 閾値は指定の PC で判定する（§8.4）。CI の共有の runner は負荷で 3 秒を超えることがあるので、届くことだけを確かめる。
    if std::env::var_os("CI").is_none() {
        assert!(elapsed < Duration::from_secs(3), "5 MiB took {elapsed:?}");
    }

    // 送り手と宛先の組ごとに、両方向の bytes と、損失・blackout で落とした packet が数えられる。
    let (host_id, guest_id) = (endpoint_id(host), endpoint_id(guest));
    std::thread::sleep(Duration::from_millis(1500));
    let text = std::fs::read_to_string(&stats).unwrap();
    let last = |source: &str, destination: &str| {
        text.lines()
            .rev()
            .map(|line| line.split(' ').collect::<Vec<_>>())
            // 書いている途中の最後の行は、欄が足りないので使わない。
            .find(|fields| fields.len() == 7 && fields[1] == source && fields[2] == destination)
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
    assert!(to_host[2] > 0, "the proxy loses packets: {to_host:?}");

    finish(guest);
    finish(host);
}

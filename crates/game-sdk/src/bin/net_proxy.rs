//! 計測用の UDP proxy（#1984 AC-1、ADR 0070 §8.4）。`KUKURI_TEST_UDP_PROXY` でこの proxy を指定した kukuri の
//! process の packet を受け、片道の遅延（一様分布）と方向ごとの損失を加えて宛先へ渡す。
//!
//! `kukuri-sdk-net-proxy --listen <IP:port> [--delay-ms <最小> <最大>] [--loss <割合>] --stats <file>`
//!
//! 既定は ADR 0070 §8.4 の条件（22.5〜27.5 ms、1%）。標準入力の `blackout <ms>` で、その間の全 packet を落とし、
//! 標準出力に `blackout <開始> <終了>` を書く。stats の file には 1 秒ごとに、送り手と宛先の組ごとの累計
//! （`<時刻> <送り手> <宛先> <bytes> <packet 数> <損失> <blackout で落とした数>`。bytes は QUIC の datagram の長さ）を
//! 足していく。時刻は QueryPerformanceCounter の値（C# の `Stopwatch.GetTimestamp()` と同じ）で、Windows 以外では
//! 起動からの ns。

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    io::{BufRead, Write},
    net::{SocketAddr, UdpSocket},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use kukuri_transport::{UDP_PROXY_ENV, UDP_PROXY_HEADER_LEN as HEADER};

type Id = [u8; 32];

#[derive(Default)]
struct Counter {
    bytes: u64,
    packets: u64,
    lost: u64,
    blacked: u64,
}

struct Shared {
    socket: UdpSocket,
    routes: Mutex<HashMap<Id, SocketAddr>>,
    counters: Mutex<HashMap<(Id, Id), Counter>>,
    blackout_until: Mutex<Option<Instant>>,
}

/// 渡す時刻の早い順に取り出す packet。
struct Queued {
    due: Instant,
    sequence: u64,
    destination: Id,
    packet: Vec<u8>,
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.due, other.sequence).cmp(&(self.due, self.sequence))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Queued {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str, offset: usize| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + offset))
    };
    let (Some(listen), Some(stats)) = (value("--listen", 1), value("--stats", 1)) else {
        eprintln!(
            "usage: kukuri-sdk-net-proxy --listen <IP:port> --stats <file> (see {UDP_PROXY_ENV})"
        );
        std::process::exit(2);
    };
    let number = |name: &str, offset: usize, default: f64| {
        value(name, offset).map_or(default, |text| text.parse().expect(name))
    };
    let delay = (
        number("--delay-ms", 1, 22.5) / 1000.0,
        number("--delay-ms", 2, 27.5) / 1000.0,
    );
    let loss = number("--loss", 1, 0.01);
    let shared = Arc::new(Shared {
        socket: UdpSocket::bind(listen).expect("bind the proxy"),
        routes: Mutex::default(),
        counters: Mutex::default(),
        blackout_until: Mutex::default(),
    });
    let (queue, queued) = mpsc::channel::<Queued>();
    std::thread::spawn({
        let shared = Arc::clone(&shared);
        move || receive(&shared, &queue, delay, loss)
    });
    std::thread::spawn({
        let shared = Arc::clone(&shared);
        move || deliver(&shared, &queued)
    });
    std::thread::spawn({
        let (shared, stats) = (Arc::clone(&shared), stats.clone());
        move || record(&shared, &stats)
    });
    println!("frequency {}", frequency());
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if let Some(ms) = line
            .strip_prefix("blackout ")
            .and_then(|ms| ms.trim().parse().ok())
        {
            let start = ticks();
            *shared.blackout_until.lock().expect("blackout") =
                Some(Instant::now() + Duration::from_millis(ms));
            println!(
                "blackout {start} {}",
                start + ms as i64 * frequency() / 1000
            );
            let _ = std::io::stdout().flush();
        }
    }
    // 標準入力が無くても、止められるまで中継を続ける。
    loop {
        std::thread::park();
    }
}

/// 受けた packet を数え、落とすか、遅延をつけて渡す待ちへ入れる。
fn receive(shared: &Shared, queue: &mpsc::Sender<Queued>, delay: (f64, f64), loss: f64) {
    let mut random = Random(ticks() as u64 | 1);
    let mut buffer = vec![0; u16::MAX as usize];
    let mut sequence = 0;
    loop {
        // Windows は、閉じた port へ送った後の受信を失敗として返す。捨てて続ける。
        let Ok((len, from)) = shared.socket.recv_from(&mut buffer) else {
            continue;
        };
        if len < HEADER {
            continue;
        }
        let (source, destination) = (id(&buffer[..32]), id(&buffer[32..HEADER]));
        shared.routes.lock().expect("routes").insert(source, from);
        if destination == [0; 32] {
            continue;
        }
        let now = Instant::now();
        let mut counters = shared.counters.lock().expect("counters");
        let counter = counters.entry((source, destination)).or_default();
        counter.bytes += (len - HEADER) as u64;
        counter.packets += 1;
        if shared
            .blackout_until
            .lock()
            .expect("blackout")
            .is_some_and(|until| now < until)
        {
            counter.blacked += 1;
            continue;
        }
        if random.next() < loss {
            counter.lost += 1;
            continue;
        }
        drop(counters);
        sequence += 1;
        let _ = queue.send(Queued {
            due: now + Duration::from_secs_f64(delay.0 + random.next() * (delay.1 - delay.0)),
            sequence,
            destination,
            packet: buffer[..len].to_vec(),
        });
    }
}

/// 時刻の来た packet を宛先へ渡す。std の sleep は Windows でも高精度の timer を使う。
fn deliver(shared: &Shared, queued: &mpsc::Receiver<Queued>) {
    let mut heap = BinaryHeap::new();
    loop {
        if heap.is_empty() {
            match queued.recv() {
                Ok(packet) => heap.push(packet),
                Err(_) => return,
            }
        }
        heap.extend(queued.try_iter());
        let now = Instant::now();
        while heap.peek().is_some_and(|packet| packet.due <= now) {
            let Some(packet) = heap.pop() else { break };
            let route = shared
                .routes
                .lock()
                .expect("routes")
                .get(&packet.destination)
                .copied();
            if let Some(route) = route {
                let _ = shared.socket.send_to(&packet.packet, route);
            }
        }
        if !heap.is_empty() {
            std::thread::sleep(Duration::from_micros(200));
        }
    }
}

/// 1 秒ごとに、組ごとの累計を stats の file へ足す。
fn record(shared: &Shared, path: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open the stats file");
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = ticks();
        let lines: String = shared
            .counters
            .lock()
            .expect("counters")
            .iter()
            .map(|((source, destination), counter)| {
                format!(
                    "{now} {} {} {} {} {} {}\n",
                    hex(source),
                    hex(destination),
                    counter.bytes,
                    counter.packets,
                    counter.lost,
                    counter.blacked
                )
            })
            .collect();
        let _ = file.write_all(lines.as_bytes());
    }
}

fn id(bytes: &[u8]) -> Id {
    bytes.try_into().unwrap_or([0; 32])
}

fn hex(id: &Id) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 遅延と損失に使う乱数（xorshift64*、0 以上 1 未満）。
struct Random(u64);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(windows)]
fn ticks() -> i64 {
    let mut value = 0;
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut value) };
    value
}

#[cfg(windows)]
fn frequency() -> i64 {
    let mut value = 0;
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut value) };
    value
}

#[cfg(not(windows))]
fn ticks() -> i64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as i64
}

#[cfg(not(windows))]
fn frequency() -> i64 {
    1_000_000_000
}

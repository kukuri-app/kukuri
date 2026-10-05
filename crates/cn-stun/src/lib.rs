//! relay と同じ host で、relay とは別の process として動かす STUN の server（ADR 0057 §6、RFC 8489）。
//! 認証の無い Binding Request にだけ、送信元の IP・port を XOR-MAPPED-ADDRESS で返す。
//! TURN・認証・他の method は持たない。送信元の IP・port は応答にだけ使い、ログへ出さない（#1483）。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;

/// 待ち受ける port。クライアントの送信先（`kukuri_webrtc_transport::STUN_PORT`）と同じ値。
pub const PORT: u16 = 3478;
/// 受け付ける要求の上限。RFC 8489 §6.1 の IPv4 の目安（576 byte）から IP・UDP の header を除いた値。
const MAX_REQUEST_BYTES: usize = 548;
/// 応答する件数を数える窓。窓ごとに、送信元 IP ごと・全体の上限を超えた要求には応答しない。
const WINDOW: Duration = Duration::from_secs(1);
const PER_IP_PER_WINDOW: u32 = 20;
const TOTAL_PER_WINDOW: u32 = 2_000;
/// 件数の要約をログへ出す間隔。
const REPORT_INTERVAL: Duration = Duration::from_secs(600);

const HEADER_BYTES: usize = 20;
const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
const BINDING_REQUEST: [u8; 2] = [0x00, 0x01];
const BINDING_SUCCESS: [u8; 2] = [0x01, 0x01];
const XOR_MAPPED_ADDRESS: [u8; 2] = [0x00, 0x20];

/// `socket` で受けた Binding Request に応答し続ける。
pub async fn serve(socket: &UdpSocket) {
    let mut responder = Responder::default();
    let mut report = tokio::time::interval(REPORT_INTERVAL);
    // 上限より 1 byte 大きい buffer で、上限を超える datagram を見分ける。
    let mut buf = [0u8; MAX_REQUEST_BYTES + 1];
    loop {
        tokio::select! {
            received = socket.recv_from(&mut buf) => {
                // 受信のエラー（Windows では届かなかった送信の ICMP）では待受けを止めない。
                if let Ok((len, from)) = received
                    && let Some(reply) = responder.respond(&buf[..len], from, Instant::now())
                {
                    let _ = socket.send_to(&reply, from).await;
                }
            }
            _ = report.tick() => responder.report(),
        }
    }
}

/// 応答の可否を決める状態。窓の中の件数と、要約までの件数だけを持つ。
#[derive(Default)]
struct Responder {
    window_start: Option<Instant>,
    per_ip: HashMap<IpAddr, u32>,
    total: u32,
    counts: Counts,
}

/// 要約に出す件数。送信元の IP・port は持たない。
#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    answered: u64,
    limited: u64,
    ignored: u64,
}

impl Responder {
    /// 応答する要求なら応答の bytes を返す。Binding Request でない・上限を超えた要求には `None`（応答しない）。
    fn respond(&mut self, request: &[u8], from: SocketAddr, now: Instant) -> Option<Vec<u8>> {
        let Some(reply) = binding_response(request, from) else {
            self.counts.ignored += 1;
            return None;
        };
        if !self.admit(from.ip(), now) {
            self.counts.limited += 1;
            return None;
        }
        self.counts.answered += 1;
        Some(reply)
    }

    /// 窓の中で、送信元 IP ごと・全体の上限に収まるかを数える。
    /// 表は窓ごとに空にし、表に載る IP の数は全体の上限を超えない。
    fn admit(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self
            .window_start
            .is_none_or(|start| now.duration_since(start) >= WINDOW)
        {
            self.window_start = Some(now);
            self.per_ip.clear();
            self.total = 0;
        }
        if self.total >= TOTAL_PER_WINDOW {
            return false;
        }
        let count = self.per_ip.entry(ip).or_default();
        if *count >= PER_IP_PER_WINDOW {
            return false;
        }
        *count += 1;
        self.total += 1;
        true
    }

    fn report(&mut self) {
        let counts = std::mem::take(&mut self.counts);
        if counts != Counts::default() {
            tracing::info!(
                answered = counts.answered,
                limited = counts.limited,
                ignored = counts.ignored,
                "stun requests since the last report"
            );
        }
    }
}

/// Binding Request への成功応答（XOR-MAPPED-ADDRESS だけ。RFC 8489 §14.2）を作る。
/// Binding Request でなければ `None`。要求の属性は読まない（応答を変えない）。
fn binding_response(request: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
    if request.len() < HEADER_BYTES
        || request.len() > MAX_REQUEST_BYTES
        || request[..2] != BINDING_REQUEST
        || request[4..8] != MAGIC_COOKIE
        || usize::from(u16::from_be_bytes([request[2], request[3]])) != request.len() - HEADER_BYTES
    {
        return None;
    }
    let (family, address) = match from.ip() {
        IpAddr::V4(ip) => (0x01, ip.octets().to_vec()),
        IpAddr::V6(ip) => (0x02, ip.octets().to_vec()),
    };
    // magic cookie と transaction id（要求の 4..20 byte）で XOR する。IPv4 は先頭の magic cookie だけを使う。
    let mask = &request[4..HEADER_BYTES];
    let value_len = 4 + address.len() as u16;
    let mut reply = Vec::with_capacity(HEADER_BYTES + 4 + usize::from(value_len));
    reply.extend_from_slice(&BINDING_SUCCESS);
    reply.extend_from_slice(&(4 + value_len).to_be_bytes());
    reply.extend_from_slice(mask);
    reply.extend_from_slice(&XOR_MAPPED_ADDRESS);
    reply.extend_from_slice(&value_len.to_be_bytes());
    reply.extend_from_slice(&[0x00, family]);
    reply.extend_from_slice(&(from.port() ^ 0x2112).to_be_bytes());
    reply.extend(address.iter().zip(mask).map(|(byte, mask)| byte ^ mask));
    Some(reply)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    /// RFC 5769 §2.2・§2.3 の試験ベクタの transaction id。
    const TRANSACTION_ID: [u8; 12] = [
        0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
    ];

    /// 属性の部分が `attributes` byte の Binding Request。
    fn request(attributes: usize) -> Vec<u8> {
        let length = u16::try_from(attributes).unwrap().to_be_bytes();
        let mut request = [
            &BINDING_REQUEST[..],
            &length,
            &MAGIC_COOKIE,
            &TRANSACTION_ID,
        ]
        .concat();
        request.resize(HEADER_BYTES + attributes, 0);
        request
    }

    /// RFC 5769 の試験ベクタの応答の header（message の長さだけが違う）。
    fn success_header(length: u8) -> Vec<u8> {
        [
            &[0x01, 0x01, 0x00, length][..],
            &MAGIC_COOKIE,
            &TRANSACTION_ID,
        ]
        .concat()
    }

    /// AC-1a: XOR-MAPPED-ADDRESS だけの成功応答を返し、その符号化が RFC 5769 §2.2・§2.3 の試験ベクタと一致する。
    #[test]
    fn binding_requests_get_the_rfc_5769_mapped_address() {
        let v4 = [
            success_header(12),
            vec![
                0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43,
            ],
        ]
        .concat();
        let v6 = [
            success_header(24),
            vec![
                0x00, 0x20, 0x00, 0x14, 0x00, 0x02, 0xa1, 0x47, 0x01, 0x13, 0xa9, 0xfa, 0xa5, 0xd3,
                0xf1, 0x79, 0xbc, 0x25, 0xf4, 0xb5, 0xbe, 0xd2, 0xb9, 0xd9,
            ],
        ]
        .concat();
        let from_v4: SocketAddr = "192.0.2.1:32853".parse().unwrap();
        let from_v6: SocketAddr = "[2001:db8:1234:5678:11:2233:4455:6677]:32853"
            .parse()
            .unwrap();
        assert_eq!(binding_response(&request(0), from_v4), Some(v4.clone()));
        assert_eq!(binding_response(&request(0), from_v6), Some(v6));
        // 属性（FINGERPRINT 等）は読まずに応答する。548 byte の要求まで受け付ける。
        let largest = request(MAX_REQUEST_BYTES - HEADER_BYTES);
        assert_eq!(largest.len(), 548);
        assert_eq!(binding_response(&largest, from_v4), Some(v4));
    }

    /// AC-1b: Binding Request でない・magic cookie 違い・長さの不一致・548 byte 超の datagram には応答しない。
    #[test]
    fn other_datagrams_get_no_response() {
        let from: SocketAddr = "192.0.2.1:32853".parse().unwrap();
        let mut response = request(0);
        response[..2].copy_from_slice(&BINDING_SUCCESS);
        let mut cookie = request(0);
        cookie[4] ^= 0xff;
        let mut length = request(0);
        length[3] = 4;
        let short = &request(0)[..HEADER_BYTES - 1];
        let oversized = request(MAX_REQUEST_BYTES - HEADER_BYTES + 1);
        for datagram in [
            &response[..],
            &cookie,
            &length,
            short,
            &oversized,
            b"not stun",
        ] {
            assert_eq!(binding_response(datagram, from), None);
        }
    }

    /// AC-1c: 1 秒の窓ごとに、送信元 IP ごとに 20 件、全体で 2,000 件まで応答する。
    #[test]
    fn requests_over_the_limits_get_no_response_until_the_next_window() {
        let mut responder = Responder::default();
        let request = request(0);
        let source = |ip: u32, port: u16| SocketAddr::from((Ipv4Addr::from(ip), port));
        let start = Instant::now();
        for port in 0..20 {
            assert!(
                responder
                    .respond(&request, source(1, port), start)
                    .is_some()
            );
        }
        // 同じ IP は port が違っても同じ上限で数える。他の IP には応答する。
        assert!(responder.respond(&request, source(1, 20), start).is_none());
        assert!(responder.respond(&request, source(2, 1), start).is_some());
        assert!(
            responder
                .respond(&request, source(1, 1), start + WINDOW)
                .is_some()
        );

        let later = start + WINDOW * 2;
        for ip in 0..TOTAL_PER_WINDOW {
            assert!(responder.respond(&request, source(ip, 1), later).is_some());
        }
        assert!(
            responder
                .respond(&request, source(TOTAL_PER_WINDOW, 1), later)
                .is_none()
        );
        assert_eq!(responder.per_ip.len(), 2_000);
        assert!(
            responder
                .respond(b"not stun", source(1, 1), later)
                .is_none()
        );
        let expected = Counts {
            answered: 2_022,
            limited: 2,
            ignored: 1,
        };
        assert_eq!(responder.counts, expected);
        responder.report();
        assert_eq!(responder.counts, Counts::default());
    }

    /// AC-1d: UDP で受けた Binding Request ごとに、送信元へ応答を 1 つ返す。形式不正の datagram には応答せず、その後も応答を続ける。
    #[tokio::test]
    async fn the_socket_answers_each_binding_request_once() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let serving = tokio::spawn(async move { serve(&server).await });
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let first = request(0);
        let mut second = request(0);
        second[8] ^= 0xff;
        for datagram in [&b"not stun"[..], &first, b"not stun", &second] {
            client.send_to(datagram, server_addr).await.unwrap();
        }
        // 届く順は要求の順で、間に余分な datagram が無い。
        for request in [first, second] {
            let mut buf = [0u8; 64];
            let (len, from) =
                tokio::time::timeout(Duration::from_secs(10), client.recv_from(&mut buf))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(from, server_addr);
            let expected = binding_response(&request, client.local_addr().unwrap());
            assert_eq!(Some(buf[..len].to_vec()), expected);
        }
        serving.abort();
    }
}

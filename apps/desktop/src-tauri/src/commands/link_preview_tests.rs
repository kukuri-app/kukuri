use super::*;
use std::collections::VecDeque;

struct FakeTransport {
    resolved: Mutex<HashMap<String, Vec<SocketAddr>>>,
    responses: Mutex<VecDeque<TransportResponse>>,
    hits: Mutex<Vec<String>>,
}

impl FakeTransport {
    fn new(responses: Vec<TransportResponse>) -> Self {
        Self {
            resolved: Mutex::new(HashMap::new()),
            responses: Mutex::new(responses.into()),
            hits: Mutex::new(Vec::new()),
        }
    }

    async fn add_host(&self, host: &str, ip: [u8; 4]) {
        self.resolved.lock().await.insert(
            host.to_string(),
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), 443)],
        );
    }

    fn html(status: u16, body: &str, remote: [u8; 4]) -> TransportResponse {
        TransportResponse {
            status,
            content_type: Some("text/html; charset=utf-8".into()),
            location: None,
            body: body.as_bytes().to_vec(),
            remote_addr: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(remote)), 443)),
        }
    }
}

impl PreviewTransport for FakeTransport {
    async fn resolve(&self, host: &str, _port: u16) -> Result<Vec<SocketAddr>, TransportFailure> {
        Ok(self
            .resolved
            .lock()
            .await
            .get(host)
            .cloned()
            .unwrap_or_default())
    }

    async fn get(
        &self,
        url: &Url,
        _resolved: &[SocketAddr],
        _accept: &'static str,
        max_bytes: usize,
    ) -> Result<TransportResponse, TransportFailure> {
        self.hits.lock().await.push(url.as_str().to_string());
        let response = self
            .responses
            .lock()
            .await
            .pop_front()
            .ok_or(TransportFailure::Network)?;
        if response.body.len() > max_bytes {
            return Err(TransportFailure::TooLarge);
        }
        Ok(response)
    }
}

#[test]
fn normalizes_only_credential_free_default_port_http_urls() {
    assert_eq!(
        normalize_url("https://Example.COM:443/path?q=1#fragment")
            .unwrap()
            .as_str(),
        "https://example.com/path?q=1"
    );
    for value in [
        "",
        "file:///tmp/a",
        "https://user:secret@example.com/",
        "https://example.com:444/",
        "https://localhost/",
        "https://service.internal/",
        "https://singlelabel/",
        "https://example.com\\@other.test/",
    ] {
        assert!(normalize_url(value).is_err(), "{value}");
    }
}

#[test]
fn rejects_private_and_reserved_addresses() {
    for value in [
        "0.0.0.0",
        "10.0.0.1",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.1.1",
        "172.16.0.1",
        "192.168.0.1",
        "192.0.2.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "255.255.255.255",
        "::1",
        "fc00::1",
        "fe80::1",
        "2001::1",
        "2001:10::1",
        "2001:20::1",
        "2001:db8::1",
        "2002:a00:1::1",
        "3fff::1",
    ] {
        assert!(!is_public_ip(value.parse().unwrap()), "{value}");
    }
    assert!(is_public_ip("93.184.216.34".parse().unwrap()));
    assert!(is_public_ip(
        "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap()
    ));
    assert!(is_public_ip("2001:4860:4860::8888".parse().unwrap()));
}

#[test]
fn parses_ogp_without_evaluating_markup() {
    let metadata = parse_metadata(
        br#"<!doctype html><html><head>
        <meta property="og:title" content="  Example &amp; title  ">
        <meta property="og:description" content="Description">
        <meta property="og:site_name" content="Example Site">
        <meta property="og:image" content="/preview.png">
        <title>Fallback</title><script>throw new Error('never')</script>
        </head></html>"#,
    );
    assert_eq!(metadata.title.as_deref(), Some("  Example & title  "));
    assert_eq!(metadata.description.as_deref(), Some("Description"));
    assert_eq!(metadata.site_name.as_deref(), Some("Example Site"));
    assert_eq!(metadata.image.as_deref(), Some("/preview.png"));
    assert_eq!(metadata.html_title, "Fallback");
}

#[test]
fn raster_image_requires_matching_declared_mime_and_magic_bytes() {
    let png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    assert_eq!(raster_mime(&png, Some("image/png")), Some("image/png"));
    assert_eq!(raster_mime(&png, Some("image/jpeg")), None);
    assert_eq!(raster_mime(&png, None), None);
}

// AC-2d: record には本文に書かれた URL のまま、取得した文字と画像の bytes を渡す。
#[test]
fn record_input_keeps_the_post_url_and_decodes_the_image() {
    let png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    let preview = LinkPreview {
        url: "https://example.test/a".into(),
        source_label: "Example".into(),
        title: "Title".into(),
        description: Some("Description".into()),
        image_data_url: Some(format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(png)
        )),
    };
    let input = record_input("https://example.test/a#top".into(), &preview);
    assert_eq!(input.url, "https://example.test/a#top");
    assert_eq!(
        (input.title.as_str(), input.site_name.as_str()),
        ("Title", "Example")
    );
    assert_eq!(input.description.as_deref(), Some("Description"));
    assert_eq!(input.image.as_deref(), Some(png.as_slice()));
    let text_only = LinkPreview {
        image_data_url: None,
        ..preview
    };
    assert_eq!(
        record_input("https://example.test/a".into(), &text_only).image,
        None
    );
}

#[tokio::test]
async fn fetches_html_and_optional_safe_image() {
    let image = TransportResponse {
        status: 200,
        content_type: Some("image/png".into()),
        location: None,
        body: [
            &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a][..],
            b"fixture",
        ]
        .concat(),
        remote_addr: Some("93.184.216.34:443".parse().unwrap()),
    };
    let transport = FakeTransport::new(vec![
        FakeTransport::html(
            200,
            r#"<meta property="og:title" content="Preview title">
               <meta property="og:image" content="https://cdn.example.com/p.png">"#,
            [93, 184, 216, 34],
        ),
        image,
    ]);
    transport.add_host("example.com", [93, 184, 216, 34]).await;
    transport
        .add_host("cdn.example.com", [93, 184, 216, 34])
        .await;

    let result = fetch_preview(
        &transport,
        Url::parse("https://example.com/post#fragment").unwrap(),
    )
    .await;
    let LinkPreviewOutcome::Available { preview } = result else {
        panic!("expected preview");
    };
    assert_eq!(preview.url, "https://example.com/post#fragment");
    assert_eq!(preview.source_label, "example.com");
    assert_eq!(preview.title, "Preview title");
    assert!(
        preview
            .image_data_url
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert_eq!(transport.hits.lock().await.len(), 2);
}

#[tokio::test]
async fn blocks_private_dns_before_http_sink() {
    let transport = FakeTransport::new(Vec::new());
    transport.add_host("example.com", [127, 0, 0, 1]).await;
    let result = fetch_preview(&transport, Url::parse("https://example.com/post").unwrap()).await;
    assert_eq!(
        result,
        LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::BlockedTarget)
    );
    assert!(transport.hits.lock().await.is_empty());
}

#[tokio::test]
async fn blocks_ipv6_special_purpose_dns_before_http_sink() {
    let transport = FakeTransport::new(Vec::new());
    transport.resolved.lock().await.insert(
        "example.com".into(),
        vec![SocketAddr::new("2001:10::1".parse().unwrap(), 443)],
    );
    let result = fetch_preview(&transport, Url::parse("https://example.com/post").unwrap()).await;
    assert_eq!(
        result,
        LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::BlockedTarget)
    );
    assert!(transport.hits.lock().await.is_empty());
}

#[tokio::test]
async fn revalidates_redirect_target_before_second_http_hit() {
    let mut redirect = FakeTransport::html(302, "", [93, 184, 216, 34]);
    redirect.location = Some("https://private.example/metadata".into());
    let transport = FakeTransport::new(vec![redirect]);
    transport.add_host("example.com", [93, 184, 216, 34]).await;
    transport.add_host("private.example", [10, 0, 0, 1]).await;

    let result = fetch_preview(&transport, Url::parse("https://example.com/post").unwrap()).await;
    assert_eq!(
        result,
        LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::BlockedTarget)
    );
    assert_eq!(transport.hits.lock().await.len(), 1);
}

#[tokio::test]
async fn distinct_in_flight_urls_are_bounded_before_spawning_network_tasks() {
    let state = LinkPreviewState::default();
    let mut in_flight = state.inner.in_flight.lock().await;
    for index in 0..MAX_IN_FLIGHT_ENTRIES {
        let (sender, _receiver) = watch::channel(None);
        in_flight.insert(format!("https://example.com/{index}"), sender);
    }
    drop(in_flight);

    assert_eq!(
        state.request("https://example.com/overflow").await,
        LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::Busy)
    );
    assert_eq!(
        state.inner.in_flight.lock().await.len(),
        MAX_IN_FLIGHT_ENTRIES
    );
}

#[test]
fn cache_is_bounded_and_expires_negative_entries() {
    let now = Instant::now();
    let mut cache = PreviewCache::default();
    let unavailable = LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::Network);
    cache.insert("https://example.com/".into(), unavailable.clone(), now);
    assert_eq!(cache.get("https://example.com/", now), Some(unavailable));
    assert!(
        cache
            .get(
                "https://example.com/",
                now + FAILURE_TTL + Duration::from_secs(1)
            )
            .is_none()
    );

    for index in 0..MAX_CACHE_ENTRIES + 10 {
        cache.insert(
            format!("https://example.com/{index}"),
            LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::Network),
            now,
        );
    }
    assert_eq!(cache.entries.len(), MAX_CACHE_ENTRIES);
    assert!(cache.bytes <= MAX_CACHE_BYTES);
}

const NIP05_PUBKEY: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

fn nip05_document(status: u16, body: String) -> TransportResponse {
    TransportResponse {
        content_type: Some("application/json".into()),
        ..FakeTransport::html(status, &body, [93, 184, 216, 34])
    }
}

// #1670 AC-2.1・2.3: その名前の値が公開鍵と一致するときだけ確認でき、転送・非 2xx・JSON でない・上限を超える応答は確認できない。
#[tokio::test]
async fn nip05_matches_only_the_named_public_key() {
    let names =
        |value: &str| format!(r#"{{"names":{{"alice":"{value}","bob":"{NIP05_PUBKEY}"}}}}"#);
    let mut redirect = nip05_document(302, String::new());
    redirect.location = Some("https://other.example/.well-known/nostr.json?name=alice".into());
    for (response, expected) in [
        (nip05_document(200, names(NIP05_PUBKEY)), true),
        (nip05_document(200, names(&"0".repeat(64))), false),
        (nip05_document(200, r#"{"names":{}}"#.into()), false),
        (nip05_document(404, names(NIP05_PUBKEY)), false),
        (nip05_document(200, "<html>".into()), false),
        (redirect, false),
        (
            nip05_document(
                200,
                format!("{} {}", names(NIP05_PUBKEY), " ".repeat(512 * 1024)),
            ),
            false,
        ),
    ] {
        let transport = FakeTransport::new(vec![response]);
        transport.add_host("example.com", [93, 184, 216, 34]).await;
        assert_eq!(
            verify_nip05(&transport, NIP05_PUBKEY, "alice@example.com").await,
            expected
        );
        assert_eq!(
            *transport.hits.lock().await,
            ["https://example.com/.well-known/nostr.json?name=alice"]
        );
    }
}

// #1670 AC-2.1・2.3: 取得できない(時間切れ・通信の失敗)ときは確認できない。
#[tokio::test]
async fn nip05_is_unverified_when_the_request_fails() {
    let transport = FakeTransport::new(Vec::new());
    transport.add_host("example.com", [93, 184, 216, 34]).await;
    assert!(!verify_nip05(&transport, NIP05_PUBKEY, "alice@example.com").await);
    assert_eq!(transport.hits.lock().await.len(), 1);
}

// #1670 AC-2.3: 形に合わない識別子と、非公開のアドレス・名前へは送らない。
#[tokio::test]
async fn nip05_does_not_request_invalid_or_private_targets() {
    let transport = FakeTransport::new(Vec::new());
    transport.add_host("intranet.example", [10, 0, 0, 1]).await;
    for identifier in [
        "Alice@example.com",
        "alice@192.0.2.1",
        "alice@printer.local",
        "alice@intranet.example",
    ] {
        assert!(!verify_nip05(&transport, NIP05_PUBKEY, identifier).await);
    }
    assert!(transport.hits.lock().await.is_empty());
}

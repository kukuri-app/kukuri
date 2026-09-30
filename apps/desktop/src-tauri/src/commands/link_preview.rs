use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use html5ever::{
    tendril::StrTendril,
    tokenizer::{
        BufferQueue, CharacterTokens, EndTag, StartTag, TagToken, Token, TokenSink,
        TokenSinkResult, Tokenizer,
    },
};
use reqwest::{StatusCode, header};
use serde::Serialize;
use tauri::{State, Url};
use tokio::sync::{Mutex, Semaphore, watch};

use crate::state::CommandError;

const MAX_URL_BYTES: usize = 4_096;
const MAX_REDIRECTS: usize = 3;
const MAX_HTML_BYTES: usize = 512 * 1024;
const MAX_IMAGE_BYTES: usize = 1024 * 1024;
const MAX_TITLE_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 500;
const MAX_SITE_NAME_CHARS: usize = 100;
const MAX_CONCURRENT_FETCHES: usize = 4;
const MAX_IN_FLIGHT_ENTRIES: usize = 32;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const SUCCESS_TTL: Duration = Duration::from_secs(10 * 60);
const FAILURE_TTL: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const USER_AGENT: &str = concat!("kukuri-link-preview/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LinkPreview {
    pub url: String,
    pub source_label: String,
    pub title: String,
    pub description: Option<String>,
    pub image_data_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkPreviewUnavailableReason {
    InvalidUrl,
    BlockedTarget,
    RedirectRejected,
    TooManyRedirects,
    Busy,
    Timeout,
    Network,
    HttpStatus,
    UnsupportedContent,
    ResponseTooLarge,
    MissingMetadata,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LinkPreviewOutcome {
    Available {
        preview: LinkPreview,
    },
    Unavailable {
        reason: LinkPreviewUnavailableReason,
    },
}

impl LinkPreviewOutcome {
    fn unavailable(reason: LinkPreviewUnavailableReason) -> Self {
        Self::Unavailable { reason }
    }

    fn estimated_bytes(&self) -> usize {
        match self {
            Self::Available { preview } => {
                preview.url.len()
                    + preview.source_label.len()
                    + preview.title.len()
                    + preview.description.as_ref().map_or(0, String::len)
                    + preview.image_data_url.as_ref().map_or(0, String::len)
            }
            Self::Unavailable { .. } => 32,
        }
    }

    fn ttl(&self) -> Duration {
        match self {
            Self::Available { .. } => SUCCESS_TTL,
            Self::Unavailable { .. } => FAILURE_TTL,
        }
    }
}

#[derive(Clone)]
struct CacheEntry {
    outcome: LinkPreviewOutcome,
    expires_at: Instant,
    bytes: usize,
}

#[derive(Default)]
struct PreviewCache {
    entries: HashMap<String, CacheEntry>,
    order: VecDeque<String>,
    bytes: usize,
}

impl PreviewCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<LinkPreviewOutcome> {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| entry.expires_at <= now)
        {
            self.remove(key);
            return None;
        }
        let outcome = self.entries.get(key)?.outcome.clone();
        self.touch(key);
        Some(outcome)
    }

    fn insert(&mut self, key: String, outcome: LinkPreviewOutcome, now: Instant) {
        self.remove(&key);
        let bytes = outcome.estimated_bytes();
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        self.bytes += bytes;
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            CacheEntry {
                expires_at: now + outcome.ttl(),
                outcome,
                bytes,
            },
        );
        while self.entries.len() > MAX_CACHE_ENTRIES || self.bytes > MAX_CACHE_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
        self.order.retain(|candidate| candidate != key);
    }

    fn touch(&mut self, key: &str) {
        self.order.retain(|candidate| candidate != key);
        self.order.push_back(key.to_string());
    }
}

struct LinkPreviewInner {
    cache: Mutex<PreviewCache>,
    in_flight: Mutex<HashMap<String, watch::Sender<Option<LinkPreviewOutcome>>>>,
    semaphore: Semaphore,
}

#[derive(Clone)]
pub struct LinkPreviewState {
    inner: Arc<LinkPreviewInner>,
}

impl Default for LinkPreviewState {
    fn default() -> Self {
        Self {
            inner: Arc::new(LinkPreviewInner {
                cache: Mutex::new(PreviewCache::default()),
                in_flight: Mutex::new(HashMap::new()),
                semaphore: Semaphore::new(MAX_CONCURRENT_FETCHES),
            }),
        }
    }
}

impl LinkPreviewState {
    async fn request(&self, value: &str) -> LinkPreviewOutcome {
        let target = match normalize_url(value) {
            Ok(target) => target,
            Err(reason) => return LinkPreviewOutcome::unavailable(reason),
        };
        let key = target.as_str().to_string();
        if let Some(cached) = self.inner.cache.lock().await.get(&key, Instant::now()) {
            return cached;
        }

        let mut receiver = {
            let mut in_flight = self.inner.in_flight.lock().await;
            if let Some(sender) = in_flight.get(&key) {
                sender.subscribe()
            } else if in_flight.len() >= MAX_IN_FLIGHT_ENTRIES {
                return LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::Busy);
            } else {
                let (sender, receiver) = watch::channel(None);
                in_flight.insert(key.clone(), sender);
                self.spawn_fetch(key.clone(), target);
                receiver
            }
        };

        loop {
            if let Some(outcome) = receiver.borrow().clone() {
                return outcome;
            }
            if receiver.changed().await.is_err() {
                return LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::Network);
            }
        }
    }

    fn spawn_fetch(&self, key: String, target: Url) {
        let inner = self.inner.clone();
        tauri::async_runtime::spawn(async move {
            let outcome = match inner.semaphore.acquire().await {
                Ok(_permit) => fetch_preview(&NetworkTransport, target).await,
                Err(_) => LinkPreviewOutcome::unavailable(LinkPreviewUnavailableReason::Network),
            };
            inner
                .cache
                .lock()
                .await
                .insert(key.clone(), outcome.clone(), Instant::now());
            if let Some(sender) = inner.in_flight.lock().await.remove(&key) {
                let _ = sender.send(Some(outcome));
            }
        });
    }
}

#[tauri::command]
pub async fn fetch_link_preview(
    state: State<'_, LinkPreviewState>,
    url: String,
) -> Result<LinkPreviewOutcome, CommandError> {
    Ok(state.request(&url).await)
}

#[derive(Clone, Debug)]
struct TransportResponse {
    status: u16,
    content_type: Option<String>,
    location: Option<String>,
    body: Vec<u8>,
    remote_addr: Option<SocketAddr>,
}

#[derive(Clone, Copy, Debug)]
enum TransportFailure {
    Timeout,
    Network,
    TooLarge,
}

trait PreviewTransport: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, TransportFailure>;

    async fn get(
        &self,
        url: &Url,
        resolved: &[SocketAddr],
        accept: &'static str,
        max_bytes: usize,
    ) -> Result<TransportResponse, TransportFailure>;
}

struct NetworkTransport;

impl PreviewTransport for NetworkTransport {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, TransportFailure> {
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| TransportFailure::Timeout)?
            .map(|addresses| addresses.collect())
            .map_err(|_| TransportFailure::Network)
    }

    async fn get(
        &self,
        url: &Url,
        resolved: &[SocketAddr],
        accept: &'static str,
        max_bytes: usize,
    ) -> Result<TransportResponse, TransportFailure> {
        let host = url.host_str().ok_or(TransportFailure::Network)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .referer(false)
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .resolve_to_addrs(host, resolved)
            .build()
            .map_err(|_| TransportFailure::Network)?;
        let mut response = client
            .get(url.as_str())
            .header(header::ACCEPT, accept)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(TransportFailure::TooLarge);
        }
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let remote_addr = response.remote_addr();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(map_reqwest_error)? {
            if body.len().saturating_add(chunk.len()) > max_bytes {
                return Err(TransportFailure::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(TransportResponse {
            status,
            content_type,
            location,
            body,
            remote_addr,
        })
    }
}

fn map_reqwest_error(error: reqwest::Error) -> TransportFailure {
    if error.is_timeout() {
        TransportFailure::Timeout
    } else {
        TransportFailure::Network
    }
}

fn normalize_url(value: &str) -> Result<Url, LinkPreviewUnavailableReason> {
    let trimmed = value.trim();
    if trimmed != value
        || trimmed.is_empty()
        || trimmed.len() > MAX_URL_BYTES
        || trimmed.contains('\\')
        || trimmed
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || !(trimmed.to_ascii_lowercase().starts_with("https://")
            || trimmed.to_ascii_lowercase().starts_with("http://"))
    {
        return Err(LinkPreviewUnavailableReason::InvalidUrl);
    }
    let mut url = Url::parse(trimmed).map_err(|_| LinkPreviewUnavailableReason::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(LinkPreviewUnavailableReason::InvalidUrl);
    }
    let expected_port = if url.scheme() == "https" { 443 } else { 80 };
    if url.port().is_some_and(|port| port != expected_port) {
        return Err(LinkPreviewUnavailableReason::BlockedTarget);
    }
    let host = url
        .host_str()
        .ok_or(LinkPreviewUnavailableReason::InvalidUrl)?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() || is_blocked_hostname(&host) {
        return Err(LinkPreviewUnavailableReason::BlockedTarget);
    }
    if host.parse::<IpAddr>().is_err() && !host.contains('.') {
        return Err(LinkPreviewUnavailableReason::BlockedTarget);
    }
    url.set_host(Some(&host))
        .map_err(|_| LinkPreviewUnavailableReason::InvalidUrl)?;
    if url.port() == Some(expected_port) {
        let _ = url.set_port(None);
    }
    url.set_fragment(None);
    Ok(url)
}

fn is_blocked_hostname(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".home.arpa")
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    if a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
    {
        return false;
    }
    true
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = ip.segments();
    if segments[0] & 0xe000 != 0x2000 {
        return false;
    }
    // IANA IPv6 special-purpose registry: 2001::/23 contains protocol assignments,
    // benchmarking and ORCHID ranges. None are valid link-preview destinations.
    if segments[0] == 0x2001 && segments[1] <= 0x01ff {
        return false;
    }
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return false;
    }
    // 6to4 is deprecated and embeds an IPv4 destination. Reject the whole range
    // instead of allowing an encoded private/non-global IPv4 target.
    if segments[0] == 0x2002 {
        return false;
    }
    if segments[0] == 0x3fff && segments[1] & 0xf000 == 0 {
        return false;
    }
    true
}

async fn resolve_public<T: PreviewTransport>(
    transport: &T,
    url: &Url,
) -> Result<Vec<SocketAddr>, LinkPreviewUnavailableReason> {
    let host = url
        .host_str()
        .ok_or(LinkPreviewUnavailableReason::InvalidUrl)?;
    let port = url
        .port_or_known_default()
        .ok_or(LinkPreviewUnavailableReason::BlockedTarget)?;
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        transport
            .resolve(host, port)
            .await
            .map_err(map_transport_failure)?
    };
    if addresses.is_empty() || addresses.iter().any(|addr| !is_public_ip(addr.ip())) {
        return Err(LinkPreviewUnavailableReason::BlockedTarget);
    }
    let mut seen = HashSet::new();
    Ok(addresses
        .into_iter()
        .filter(|address| seen.insert(*address))
        .collect())
}

fn map_transport_failure(failure: TransportFailure) -> LinkPreviewUnavailableReason {
    match failure {
        TransportFailure::Timeout => LinkPreviewUnavailableReason::Timeout,
        TransportFailure::Network => LinkPreviewUnavailableReason::Network,
        TransportFailure::TooLarge => LinkPreviewUnavailableReason::ResponseTooLarge,
    }
}

async fn fetch_bounded<T: PreviewTransport>(
    transport: &T,
    initial: Url,
    accept: &'static str,
    max_bytes: usize,
) -> Result<(Url, TransportResponse), LinkPreviewUnavailableReason> {
    let mut current = initial;
    for redirect_count in 0..=MAX_REDIRECTS {
        let resolved = resolve_public(transport, &current).await?;
        let response = transport
            .get(&current, &resolved, accept, max_bytes)
            .await
            .map_err(map_transport_failure)?;
        let remote = response
            .remote_addr
            .ok_or(LinkPreviewUnavailableReason::BlockedTarget)?;
        if !is_public_ip(remote.ip()) || !resolved.iter().any(|address| address.ip() == remote.ip())
        {
            return Err(LinkPreviewUnavailableReason::BlockedTarget);
        }
        let status = StatusCode::from_u16(response.status)
            .map_err(|_| LinkPreviewUnavailableReason::HttpStatus)?;
        if status.is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err(LinkPreviewUnavailableReason::TooManyRedirects);
            }
            let location = response
                .location
                .as_deref()
                .ok_or(LinkPreviewUnavailableReason::RedirectRejected)?;
            let joined = current
                .join(location)
                .map_err(|_| LinkPreviewUnavailableReason::RedirectRejected)?;
            let next = normalize_url(joined.as_str())
                .map_err(|_| LinkPreviewUnavailableReason::RedirectRejected)?;
            if current.scheme() == "https" && next.scheme() != "https" {
                return Err(LinkPreviewUnavailableReason::RedirectRejected);
            }
            current = next;
            continue;
        }
        if !status.is_success() {
            return Err(LinkPreviewUnavailableReason::HttpStatus);
        }
        return Ok((current, response));
    }
    Err(LinkPreviewUnavailableReason::TooManyRedirects)
}

#[derive(Default)]
struct MetadataState {
    title: Option<String>,
    description: Option<String>,
    site_name: Option<String>,
    image: Option<String>,
    html_title: String,
}

struct MetadataSink {
    state: RefCell<MetadataState>,
    in_title: Cell<bool>,
}

impl TokenSink for MetadataSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        match token {
            TagToken(tag) if tag.kind == StartTag && &*tag.name == "title" => {
                self.in_title.set(true);
            }
            TagToken(tag) if tag.kind == EndTag && &*tag.name == "title" => {
                self.in_title.set(false);
            }
            TagToken(tag) if tag.kind == StartTag && &*tag.name == "meta" => {
                let mut key = None;
                let mut content = None;
                for attribute in tag.attrs {
                    match attribute.name.local.as_ref() {
                        "property" | "name" => key = Some(attribute.value.to_string()),
                        "content" => content = Some(attribute.value.to_string()),
                        _ => {}
                    }
                }
                if let (Some(key), Some(content)) = (key, content) {
                    let key = key.trim().to_ascii_lowercase();
                    let mut state = self.state.borrow_mut();
                    match key.as_str() {
                        "og:title" if state.title.is_none() => state.title = Some(content),
                        "og:description" if state.description.is_none() => {
                            state.description = Some(content)
                        }
                        "description" if state.description.is_none() => {
                            state.description = Some(content)
                        }
                        "og:site_name" if state.site_name.is_none() => {
                            state.site_name = Some(content)
                        }
                        "og:image" | "og:image:url" | "og:image:secure_url"
                            if state.image.is_none() =>
                        {
                            state.image = Some(content)
                        }
                        _ => {}
                    }
                }
            }
            CharacterTokens(value) if self.in_title.get() => {
                let mut state = self.state.borrow_mut();
                if state.html_title.chars().count() < MAX_TITLE_CHARS * 2 {
                    state.html_title.push_str(&value);
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn parse_metadata(bytes: &[u8]) -> MetadataState {
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(&String::from_utf8_lossy(bytes)));
    let tokenizer = Tokenizer::new(
        MetadataSink {
            state: RefCell::new(MetadataState::default()),
            in_title: Cell::new(false),
        },
        Default::default(),
    );
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.state.into_inner()
}

fn normalized_text(value: &str, max_chars: usize) -> Option<String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(max_chars).collect())
}

fn media_type(value: Option<&str>) -> Option<&str> {
    value?.split(';').next().map(str::trim)
}

fn raster_mime(bytes: &[u8], declared: Option<&str>) -> Option<&'static str> {
    let sniffed = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        "image/png"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    if !declared.is_some_and(|value| value.eq_ignore_ascii_case(sniffed)) {
        return None;
    }
    Some(sniffed)
}

async fn fetch_preview<T: PreviewTransport>(transport: &T, initial: Url) -> LinkPreviewOutcome {
    match fetch_preview_result(transport, initial).await {
        Ok(preview) => LinkPreviewOutcome::Available { preview },
        Err(reason) => LinkPreviewOutcome::unavailable(reason),
    }
}

async fn fetch_preview_result<T: PreviewTransport>(
    transport: &T,
    initial: Url,
) -> Result<LinkPreview, LinkPreviewUnavailableReason> {
    let original_url = initial.as_str().to_string();
    let (final_url, html) = fetch_bounded(
        transport,
        initial,
        "text/html,application/xhtml+xml;q=0.9",
        MAX_HTML_BYTES,
    )
    .await?;
    let declared_html = media_type(html.content_type.as_deref());
    if !declared_html.is_some_and(|value| {
        value.eq_ignore_ascii_case("text/html")
            || value.eq_ignore_ascii_case("application/xhtml+xml")
    }) {
        return Err(LinkPreviewUnavailableReason::UnsupportedContent);
    }
    let metadata = parse_metadata(&html.body);
    let title = metadata
        .title
        .as_deref()
        .and_then(|value| normalized_text(value, MAX_TITLE_CHARS))
        .or_else(|| normalized_text(&metadata.html_title, MAX_TITLE_CHARS))
        .ok_or(LinkPreviewUnavailableReason::MissingMetadata)?;
    let host = final_url
        .host_str()
        .ok_or(LinkPreviewUnavailableReason::MissingMetadata)?;
    let source_label = metadata
        .site_name
        .as_deref()
        .and_then(|value| normalized_text(value, MAX_SITE_NAME_CHARS))
        .unwrap_or_else(|| host.trim_start_matches("www.").to_string());
    let description = metadata
        .description
        .as_deref()
        .and_then(|value| normalized_text(value, MAX_DESCRIPTION_CHARS));
    let image_data_url = if let Some(image) = metadata.image.as_deref() {
        fetch_image(transport, &final_url, image).await.ok()
    } else {
        None
    };
    Ok(LinkPreview {
        url: original_url,
        source_label,
        title,
        description,
        image_data_url,
    })
}

async fn fetch_image<T: PreviewTransport>(
    transport: &T,
    page_url: &Url,
    image_value: &str,
) -> Result<String, LinkPreviewUnavailableReason> {
    let joined = page_url
        .join(image_value.trim())
        .map_err(|_| LinkPreviewUnavailableReason::InvalidUrl)?;
    let image_url = normalize_url(joined.as_str())?;
    if page_url.scheme() == "https" && image_url.scheme() != "https" {
        return Err(LinkPreviewUnavailableReason::RedirectRejected);
    }
    let (_, response) = fetch_bounded(
        transport,
        image_url,
        "image/avif,image/webp,image/png,image/jpeg,image/gif;q=0.9",
        MAX_IMAGE_BYTES,
    )
    .await?;
    let declared = media_type(response.content_type.as_deref());
    let mime = raster_mime(&response.body, declared)
        .ok_or(LinkPreviewUnavailableReason::UnsupportedContent)?;
    Ok(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(response.body)
    ))
}

#[cfg(test)]
#[path = "link_preview_tests.rs"]
mod tests;

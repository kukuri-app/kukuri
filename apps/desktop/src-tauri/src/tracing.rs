use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tracing::{Event, Level, Subscriber, field::Visit};
use tracing_subscriber::{
    EnvFilter, Layer, layer::Context, layer::SubscriberExt, util::SubscriberInitExt,
};

pub(crate) const DEFAULT_TRACING_DIRECTIVES: &str =
    "warn,kukuri_desktop_tauri_lib=info,kukuri_app_api=info,kukuri_connectivity=info";
pub(crate) const DEFAULT_SUPPRESS_DIRECTIVES: &[&str] = &[
    "mainline::rpc::socket=error",
    "noq_proto::connection=error",
    "iroh::socket::remote_map::remote_state=error",
    "iroh_docs::engine::live=error",
    "iroh_gossip::net=error",
];

/// #978: 開発者向けログビューアが読む in-memory ring buffer の上限。
/// 件数・合計byte・1行byteの3段で固定し、稼働時間に比例して増えない(AC-3)。
/// 超過時は古い行から落とし、新しい行は捨てない。
pub(crate) const LOG_BUFFER_MAX_ENTRIES: usize = 2_000;
pub(crate) const LOG_BUFFER_MAX_BYTES: usize = 1024 * 1024;
pub(crate) const LOG_LINE_MAX_BYTES: usize = 4 * 1024;

pub(crate) fn resolve_tracing_directives(rust_log: Option<&str>) -> String {
    let directives = rust_log
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_TRACING_DIRECTIVES)
        .to_owned();

    let mut resolved = directives.clone();
    for suppress_directive in DEFAULT_SUPPRESS_DIRECTIVES {
        let target = suppress_directive
            .split('=')
            .next()
            .expect("suppress directives must have a target");
        if directives_contains_target(&directives, target) {
            continue;
        }
        resolved.push(',');
        resolved.push_str(suppress_directive);
    }

    resolved
}

fn directives_contains_target(directives: &str, target: &str) -> bool {
    directives
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .any(|directive| directive == target || directive.starts_with(&format!("{target}=")))
}

/// 1件のログ行。`seq` はプロセス内で単調増加し、buffer から落ちた行の範囲を
/// 呼出元が `oldest_seq` との差から判別できる。
///
/// #1206: 直前の行と level・target・message が同じ event は行を増やさず、末尾の行を
/// 新しい `seq` で置き換えて `repeat_count` を進める。`first_seq` / `first_timestamp_ms` は
/// 最初の発生、`seq` / `timestamp_ms` は最新の発生を指す。同一警告の連発で他の行が
/// buffer から押し出されず、件数と期間も失われない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DesktopLogEntry {
    pub(crate) seq: u64,
    pub(crate) timestamp_ms: u64,
    pub(crate) level: &'static str,
    pub(crate) target: String,
    pub(crate) message: String,
    pub(crate) repeat_count: u64,
    pub(crate) first_seq: u64,
    pub(crate) first_timestamp_ms: u64,
}

impl DesktopLogEntry {
    fn byte_len(&self) -> usize {
        self.target.len() + self.message.len()
    }
}

/// `read_desktop_logs` の応答。`oldest_seq` は buffer に残っている最古の行(の `first_seq`)、
/// `next_seq` は次に採番される値(= これまでに記録した総件数 + 1)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DesktopLogSnapshot {
    pub(crate) entries: Vec<DesktopLogEntry>,
    pub(crate) oldest_seq: Option<u64>,
    pub(crate) next_seq: u64,
    pub(crate) max_entries: usize,
    pub(crate) max_bytes: usize,
}

struct BufferInner {
    entries: VecDeque<DesktopLogEntry>,
    bytes: usize,
    next_seq: u64,
}

pub(crate) struct DesktopLogBuffer {
    inner: Mutex<BufferInner>,
    max_entries: usize,
    max_bytes: usize,
    max_line_bytes: usize,
}

impl DesktopLogBuffer {
    pub(crate) fn with_limits(max_entries: usize, max_bytes: usize, max_line_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(BufferInner {
                entries: VecDeque::new(),
                bytes: 0,
                next_seq: 1,
            }),
            max_entries,
            max_bytes,
            max_line_bytes,
        }
    }

    pub(crate) fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// 行を追加し、上限を超えた分を古い側から落とす。lockが毒化していても
    /// ログ出力を止めない(標準出力側の挙動には影響しない)。
    pub(crate) fn push(&self, level: &Level, target: &str, message: String) {
        let message = truncate_line(message, self.max_line_bytes);
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let seq = inner.next_seq;
        inner.next_seq += 1;
        let timestamp_ms = unix_millis();
        if let Some(last) = inner.entries.back_mut()
            && last.level == level.as_str()
            && last.target == target
            && last.message == message
        {
            last.seq = seq;
            last.timestamp_ms = timestamp_ms;
            last.repeat_count += 1;
            return;
        }
        let entry = DesktopLogEntry {
            seq,
            timestamp_ms,
            level: level.as_str(),
            target: target.to_owned(),
            message,
            repeat_count: 1,
            first_seq: seq,
            first_timestamp_ms: timestamp_ms,
        };
        inner.bytes += entry.byte_len();
        inner.entries.push_back(entry);
        while inner.entries.len() > self.max_entries || inner.bytes > self.max_bytes {
            let Some(dropped) = inner.entries.pop_front() else {
                break;
            };
            inner.bytes -= dropped.byte_len();
        }
    }

    /// `after_seq` より新しい行を、新しい側から最大 `limit` 件返す。
    pub(crate) fn snapshot(&self, after_seq: Option<u64>, limit: usize) -> DesktopLogSnapshot {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let limit = limit.min(self.max_entries);
        let matching: Vec<&DesktopLogEntry> = inner
            .entries
            .iter()
            .filter(|entry| after_seq.is_none_or(|after| entry.seq > after))
            .collect();
        let skip = matching.len().saturating_sub(limit);
        DesktopLogSnapshot {
            entries: matching.into_iter().skip(skip).cloned().collect(),
            oldest_seq: inner.entries.front().map(|entry| entry.first_seq),
            next_seq: inner.next_seq,
            max_entries: self.max_entries,
            max_bytes: self.max_bytes,
        }
    }

    #[cfg(test)]
    fn stats(&self) -> (usize, usize) {
        let inner = self.inner.lock().expect("log buffer lock");
        (inner.entries.len(), inner.bytes)
    }
}

fn truncate_line(message: String, max_line_bytes: usize) -> String {
    if message.len() <= max_line_bytes {
        return message;
    }
    let mut cut = max_line_bytes;
    while !message.is_char_boundary(cut) {
        cut -= 1;
    }
    let removed = message.len() - cut;
    let mut truncated = message[..cut].to_owned();
    truncated.push_str(&format!("…(truncated {removed} bytes)"));
    truncated
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// fmt layer と同じ並び(message の後に `key=value`)で1行へまとめる。
#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: String,
}

impl LineVisitor {
    fn finish(self) -> String {
        if self.message.is_empty() {
            self.fields
        } else if self.fields.is_empty() {
            self.message
        } else {
            format!("{} {}", self.message, self.fields)
        }
    }

    fn push_field(&mut self, name: &str, value: impl std::fmt::Display) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        self.fields.push_str(&format!("{name}={value}"));
    }
}

impl Visit for LineVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.push_field(field.name(), format_args!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            // fmt layer と同じく文字列 field は引用符付きで出す。
            self.push_field(field.name(), format_args!("{value:?}"));
        }
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.push_field(field.name(), value);
    }
}

/// 標準出力用 subscriber の上に重ね、同じ filter を通った event だけを buffer へ写す。
pub(crate) struct DesktopLogBufferLayer {
    buffer: Arc<DesktopLogBuffer>,
}

impl DesktopLogBufferLayer {
    pub(crate) fn new(buffer: Arc<DesktopLogBuffer>) -> Self {
        Self { buffer }
    }
}

impl<S: Subscriber> Layer<S> for DesktopLogBufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = LineVisitor::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        self.buffer
            .push(metadata.level(), metadata.target(), visitor.finish());
    }
}

static DESKTOP_LOG_BUFFER: OnceLock<Arc<DesktopLogBuffer>> = OnceLock::new();

/// process 全体で1つの buffer。`init_tracing` が subscriber を組む前でも
/// Tauri state として `manage` できるよう、初回参照時に作る。
pub(crate) fn desktop_log_buffer() -> Arc<DesktopLogBuffer> {
    DESKTOP_LOG_BUFFER
        .get_or_init(|| {
            Arc::new(DesktopLogBuffer::with_limits(
                LOG_BUFFER_MAX_ENTRIES,
                LOG_BUFFER_MAX_BYTES,
                LOG_LINE_MAX_BYTES,
            ))
        })
        .clone()
}

pub(crate) fn init_tracing() {
    let env_filter = EnvFilter::new(resolve_tracing_directives(
        std::env::var("RUST_LOG").ok().as_deref(),
    ));

    // 標準出力側(fmt + EnvFilter)は従来のまま。buffer layer はその外側に重ねるため、
    // 標準出力へ出ない event を保持することはなく、`RUST_LOG` の解釈も変わらない。
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .finish()
        .with(DesktopLogBufferLayer::new(desktop_log_buffer()))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SUPPRESS_DIRECTIVES, DEFAULT_TRACING_DIRECTIVES, DesktopLogBuffer,
        DesktopLogBufferLayer, LOG_BUFFER_MAX_BYTES, LOG_BUFFER_MAX_ENTRIES, LOG_LINE_MAX_BYTES,
        resolve_tracing_directives,
    };
    use std::sync::Arc;
    use tracing::Level;
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt};

    #[test]
    fn default_tracing_directives_add_noise_suppression() {
        let directives = resolve_tracing_directives(None);
        assert!(directives.contains(DEFAULT_TRACING_DIRECTIVES));
        for suppress_directive in DEFAULT_SUPPRESS_DIRECTIVES {
            assert!(directives.contains(suppress_directive));
        }
    }

    #[test]
    fn default_filter_keeps_connectivity_transitions_without_verbose_runtime_logs() {
        let buffer = Arc::new(DesktopLogBuffer::with_limits(10, usize::MAX, usize::MAX));
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(resolve_tracing_directives(None)))
            .with_writer(std::io::sink)
            .finish()
            .with(DesktopLogBufferLayer::new(buffer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "kukuri_connectivity", generation = 1, "stack recovered");
            tracing::info!(target: "kukuri_desktop_runtime", "verbose runtime detail");
        });
        let snapshot = buffer.snapshot(None, 10);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].target, "kukuri_connectivity");
        assert!(snapshot.entries[0].message.contains("stack recovered"));
    }

    #[test]
    fn explicit_rust_log_keeps_target_specific_override() {
        let directives = resolve_tracing_directives(Some(
            "info,iroh_docs::engine::live=warn,kukuri_desktop_tauri_lib=debug",
        ));
        assert!(
            directives.contains("iroh_docs::engine::live=warn"),
            "expected explicit target override to be preserved"
        );
        assert!(!directives.contains("iroh_docs::engine::live=error"));
        assert!(directives.contains("noq_proto::connection=error"));
        assert!(directives.contains("mainline::rpc::socket=error"));
    }

    #[test]
    fn entry_limit_drops_oldest_lines_and_keeps_sequence() {
        let buffer = DesktopLogBuffer::with_limits(3, usize::MAX, usize::MAX);
        for index in 1..=5 {
            buffer.push(&Level::INFO, "kukuri", format!("line {index}"));
        }
        let snapshot = buffer.snapshot(None, 100);
        assert_eq!(
            snapshot
                .entries
                .iter()
                .map(|entry| entry.seq)
                .collect::<Vec<_>>(),
            [3, 4, 5]
        );
        assert_eq!(snapshot.oldest_seq, Some(3));
        assert_eq!(snapshot.next_seq, 6);
        assert_eq!(buffer.stats().0, 3);
    }

    #[test]
    fn repeated_identical_lines_collapse_into_one_entry_with_count() {
        // #1206 TR-4: 反復 drop → 件数集約 → 復旧。先行する行を buffer から押し出さない。
        let buffer = DesktopLogBuffer::with_limits(3, usize::MAX, usize::MAX);
        buffer.push(&Level::INFO, "kukuri", "before".to_owned());
        for _ in 0..1_000 {
            buffer.push(
                &Level::WARN,
                "iroh",
                "Dropping received relay packet".to_owned(),
            );
        }
        buffer.push(&Level::INFO, "kukuri", "recovered".to_owned());

        let snapshot = buffer.snapshot(None, 100);
        assert_eq!(
            snapshot
                .entries
                .iter()
                .map(|entry| (entry.message.as_str(), entry.repeat_count))
                .collect::<Vec<_>>(),
            [
                ("before", 1),
                ("Dropping received relay packet", 1_000),
                ("recovered", 1)
            ]
        );
        let repeated = &snapshot.entries[1];
        assert_eq!((repeated.first_seq, repeated.seq), (2, 1_001));
        assert!(repeated.first_timestamp_ms <= repeated.timestamp_ms);
        assert_eq!(snapshot.oldest_seq, Some(1));
        assert_eq!(snapshot.next_seq, 1_003);
    }

    #[test]
    fn repeated_line_is_reissued_to_incremental_readers() {
        let buffer = DesktopLogBuffer::with_limits(10, usize::MAX, usize::MAX);
        buffer.push(&Level::WARN, "k", "same".to_owned());
        let last_seen = buffer.snapshot(None, 10).entries[0].seq;
        buffer.push(&Level::WARN, "k", "same".to_owned());
        // level か target が違えば別の行として扱う。
        buffer.push(&Level::INFO, "k", "same".to_owned());
        buffer.push(&Level::INFO, "other", "same".to_owned());

        let newer = buffer.snapshot(Some(last_seen), 10);
        assert_eq!(newer.entries.len(), 3);
        assert_eq!(
            (
                newer.entries[0].first_seq,
                newer.entries[0].seq,
                newer.entries[0].repeat_count
            ),
            (1, 2, 2)
        );
        assert_eq!(newer.oldest_seq, Some(1));
        assert_eq!(buffer.stats().0, 3);
    }

    #[test]
    fn byte_limit_drops_several_old_lines_before_entry_limit() {
        // 各行 target(1) + message(10) = 11 byte。上限 30 byte なら 2 行しか残らない。
        let buffer = DesktopLogBuffer::with_limits(100, 30, usize::MAX);
        for index in 0..4 {
            buffer.push(&Level::INFO, "k", format!("0123456{index:03}"));
        }
        let (len, bytes) = buffer.stats();
        assert_eq!(len, 2);
        assert!(bytes <= 30, "bytes={bytes}");
        let snapshot = buffer.snapshot(None, 100);
        assert_eq!(snapshot.oldest_seq, Some(3));
        assert_eq!(snapshot.entries[0].message, "0123456002");
    }

    #[test]
    fn long_line_is_truncated_on_a_char_boundary_before_it_is_stored() {
        let buffer = DesktopLogBuffer::with_limits(10, usize::MAX, 8);
        buffer.push(&Level::WARN, "k", "ああああああ".to_owned()); // 18 bytes
        let entry = &buffer.snapshot(None, 10).entries[0];
        assert!(entry.message.starts_with("ああ"));
        assert!(
            entry.message.contains("(truncated 12 bytes)"),
            "{}",
            entry.message
        );
        assert!(buffer.stats().1 < 18 + 1 + 32);
        buffer.push(&Level::WARN, "k", "short".to_owned());
        assert_eq!(buffer.snapshot(None, 10).entries[1].message, "short");
    }

    #[test]
    fn snapshot_after_seq_returns_only_newer_lines_and_honours_limit() {
        let buffer = DesktopLogBuffer::with_limits(10, usize::MAX, usize::MAX);
        for index in 1..=6 {
            buffer.push(&Level::INFO, "k", format!("{index}"));
        }
        let newer = buffer.snapshot(Some(4), 10);
        assert_eq!(
            newer
                .entries
                .iter()
                .map(|entry| entry.seq)
                .collect::<Vec<_>>(),
            [5, 6]
        );
        let limited = buffer.snapshot(None, 2);
        assert_eq!(
            limited
                .entries
                .iter()
                .map(|entry| entry.seq)
                .collect::<Vec<_>>(),
            [5, 6]
        );
        assert_eq!(limited.oldest_seq, Some(1));
        assert!(buffer.snapshot(Some(6), 10).entries.is_empty());
    }

    #[test]
    fn production_limits_are_fixed() {
        assert_eq!(LOG_BUFFER_MAX_ENTRIES, 2_000);
        assert_eq!(LOG_BUFFER_MAX_BYTES, 1024 * 1024);
        assert_eq!(LOG_LINE_MAX_BYTES, 4 * 1024);
        let buffer = DesktopLogBuffer::with_limits(
            LOG_BUFFER_MAX_ENTRIES,
            LOG_BUFFER_MAX_BYTES,
            LOG_LINE_MAX_BYTES,
        );
        for index in 0..(LOG_BUFFER_MAX_ENTRIES * 2) {
            buffer.push(
                &Level::INFO,
                "kukuri",
                format!("line {index} {}", "x".repeat(5000)),
            );
        }
        let (len, bytes) = buffer.stats();
        assert!(len <= LOG_BUFFER_MAX_ENTRIES);
        assert!(bytes <= LOG_BUFFER_MAX_BYTES);
    }

    #[test]
    fn layer_receives_only_events_that_pass_the_stdout_filter() {
        let buffer = Arc::new(DesktopLogBuffer::with_limits(10, usize::MAX, usize::MAX));
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(resolve_tracing_directives(Some(
                "warn,buffer_test=info",
            ))))
            .with_writer(std::io::sink)
            .finish()
            .with(DesktopLogBufferLayer::new(buffer.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "buffer_test", peer = "abc", "connected");
            tracing::debug!(target: "buffer_test", "filtered by level");
            tracing::info!(target: "other_crate", "filtered by target");
            tracing::warn!(target: "other_crate", "kept as warn");
        });
        let snapshot = buffer.snapshot(None, 10);
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.entries[0].level, "INFO");
        assert_eq!(snapshot.entries[0].target, "buffer_test");
        assert_eq!(snapshot.entries[0].message, "connected peer=\"abc\"");
        assert_eq!(snapshot.entries[1].level, "WARN");
        assert_eq!(snapshot.entries[1].message, "kept as warn");
    }
}

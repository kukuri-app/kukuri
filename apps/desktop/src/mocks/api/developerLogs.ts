import { type DesktopApi, type DesktopLogEntry, type DesktopLogSnapshot } from '@/lib/api';

type DeveloperLogsMock = Pick<DesktopApi, 'readDesktopLogs'>;

// #978: browser / Storybook / Playwright 向けの固定ログ。時刻は視覚回帰のため固定する。
// 実 backend の文言に近い長さ・識別子を含め、長い 1 行の折り返しも確認できるようにする。
const BASE_TIMESTAMP_MS = Date.UTC(2026, 8, 11, 6, 0, 0);
const MOCK_MAX_ENTRIES = 2000;
const MOCK_MAX_BYTES = 1024 * 1024;

const MOCK_LINES: Array<[level: string, target: string, message: string]> = [
  ['INFO', 'kukuri_desktop_tauri_lib', 'desktop profile lease acquired profile="desktop"'],
  ['INFO', 'kukuri_desktop_tauri_lib', 'app-level legal consent satisfied; starting runtime'],
  ['INFO', 'kukuri_app_api::service', 'timeline projection rebuilt topic="kukuri:topic:general" posts=42'],
  ['INFO', 'kukuri_desktop_tauri_lib', 'received kukuri desktop single-instance activation'],
  ['WARN', 'kukuri_app_api::sync', 'docs sync retry scheduled replica="kukuri-docs:topic:general" attempt=2 delay_ms=1500'],
  ['INFO', 'kukuri_app_api::service', 'community node session established base_url="https://node.example.test" phase="active"'],
  ['ERROR', 'kukuri_desktop_tauri_lib', 'failed to show background OS notification error=notification service unavailable'],
  ['INFO', 'kukuri_app_api::service', 'private channel replica joined channel="kukuri:channel:9f1c" audience=12'],
  ['WARN', 'kukuri_app_api::sync', 'peer connection degraded to relay fallback peer=3f9ab2c1e4d5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1 path="relay_fallback"'],
  ['INFO', 'kukuri_app_api::service', 'notification projection updated unread=3'],
  ['INFO', 'kukuri_desktop_tauri_lib', 'update check finished status="up_to_date" current_version="0.2.1"'],
  ['INFO', 'kukuri_app_api::service', 'blob preview generated asset="blob:sha256:7c2e…" bytes=184320'],
];

export function developerLogFixtureEntries(): DesktopLogEntry[] {
  return MOCK_LINES.map(([level, target, message], index) => ({
    seq: index + 1,
    timestamp_ms: BASE_TIMESTAMP_MS + index * 4_250,
    level,
    target,
    message,
    repeat_count: 1,
    first_seq: index + 1,
    first_timestamp_ms: BASE_TIMESTAMP_MS + index * 4_250,
  }));
}

/// Storybook / test 向けの同期 snapshot。`readDesktopLogs` と同じ内容を返す。
export function developerLogFixtureSnapshot(
  afterSeq?: number | null,
  limit?: number | null
): DesktopLogSnapshot {
  const entries = developerLogFixtureEntries();
  const matching = entries.filter((entry) => afterSeq == null || entry.seq > afterSeq);
  const bounded = limit && limit > 0 ? matching.slice(-limit) : matching;
  return {
    entries: bounded,
    oldest_seq: entries[0]?.seq ?? null,
    next_seq: entries.length + 1,
    max_entries: MOCK_MAX_ENTRIES,
    max_bytes: MOCK_MAX_BYTES,
  };
}

export function createDeveloperLogsMock(): DeveloperLogsMock {
  return {
    async readDesktopLogs(afterSeq, limit) {
      return developerLogFixtureSnapshot(afterSeq, limit);
    },
  };
}

import type { DesktopLogEntry, DesktopLogSnapshot } from '@/lib/api';

export const DESKTOP_LOGS_EXPORT_FILE_NAME = 'kukuri-logs.txt';

/// ビューアが保持する表示状態。backend の ring buffer と同じ範囲(oldestSeq 以降)だけを持つ。
export type DesktopLogsView = {
  entries: DesktopLogEntry[];
  oldestSeq: number | null;
  nextSeq: number;
  maxEntries: number;
  maxBytes: number;
  /// backend の buffer が一度でも上限に達し、それより古い行が残っていない。
  droppedOlder: boolean;
  /// 前回の表示と今回の応答の間に上限を超え、間の行が失われた(差分ではなく全件で置き換えた)。
  gapSinceLastRefresh: boolean;
};

export function lastDesktopLogSeq(view: DesktopLogsView | null): number | null {
  const last = view?.entries.at(-1);
  return last ? last.seq : null;
}

/// `afterSeq = lastDesktopLogSeq(previous)` で取得した snapshot を表示状態へ取り込む。
/// 連続していれば末尾へ追記し、buffer から落ちた行は表示からも外す。連続していなければ
/// 応答で置き換え、失われた範囲があることを記録する。
export function mergeDesktopLogSnapshot(
  previous: DesktopLogsView | null,
  snapshot: DesktopLogSnapshot
): DesktopLogsView {
  const lastSeq = lastDesktopLogSeq(previous);
  const oldestSeq = snapshot.oldest_seq;
  const contiguous =
    previous !== null && lastSeq !== null && oldestSeq !== null && oldestSeq <= lastSeq + 1;
  const newer = contiguous ? snapshot.entries.filter((entry) => entry.seq > lastSeq) : [];
  // 反復で再発行された行は、表示済みの同じ行(first_seq 以降)を置き換える。
  const supersededFrom = newer[0]?.first_seq ?? Number.POSITIVE_INFINITY;
  const entries = contiguous
    ? [
        ...previous.entries.filter((entry) => entry.seq >= oldestSeq && entry.seq < supersededFrom),
        ...newer,
      ]
    : snapshot.entries;
  const gapSinceLastRefresh =
    previous !== null && lastSeq !== null && oldestSeq !== null && oldestSeq > lastSeq + 1;
  const droppedOlder =
    (oldestSeq !== null && oldestSeq > 1) || (oldestSeq === null && snapshot.next_seq > 1);
  return {
    entries,
    oldestSeq,
    nextSeq: snapshot.next_seq,
    maxEntries: snapshot.max_entries,
    maxBytes: snapshot.max_bytes,
    droppedOlder,
    gapSinceLastRefresh,
  };
}

export function formatDesktopLogTimestamp(timestampMs: number): string {
  const date = new Date(timestampMs);
  return Number.isNaN(date.getTime()) ? String(timestampMs) : date.toISOString();
}

/// 反復した行だけに付ける接尾辞。件数と最初の発生時刻を残し、期間を読めるようにする。
export function formatDesktopLogRepeat(entry: DesktopLogEntry): string | null {
  return entry.repeat_count > 1
    ? `[repeated ${entry.repeat_count} times since ${formatDesktopLogTimestamp(entry.first_timestamp_ms)}]`
    : null;
}

export function formatDesktopLogLine(entry: DesktopLogEntry): string {
  const line = `${formatDesktopLogTimestamp(entry.timestamp_ms)} ${entry.level.padEnd(5)} ${entry.target}: ${entry.message}`;
  const repeat = formatDesktopLogRepeat(entry);
  return repeat ? `${line} ${repeat}` : line;
}

export function formatDesktopLogBytes(bytes: number): string {
  if (bytes % (1024 * 1024) === 0) {
    return `${bytes / (1024 * 1024)} MiB`;
  }
  if (bytes % 1024 === 0) {
    return `${bytes / 1024} KiB`;
  }
  return `${bytes} B`;
}

/// コピー／書き出しの本文。診断レポートと同じく、含めない情報を先頭で明示する。
export function buildDesktopLogsExport(view: DesktopLogsView, exportedAt: Date = new Date()): string {
  const lines = [
    '# kukuri desktop logs',
    '',
    `exported_at: ${exportedAt.toISOString()}`,
    `entries: ${view.entries.length}`,
    `buffer_limit: ${view.maxEntries} entries / ${formatDesktopLogBytes(view.maxBytes)}`,
    `older_lines_dropped: ${view.droppedOlder ? 'yes' : 'no'}`,
    '',
    'redaction:',
    '- secret keys, auth tokens, DM bodies, and passphrases are never logged.',
    '- peer, topic, channel, node URL, and local path identifiers may appear; remove lines before sharing.',
    '',
  ];
  for (const entry of view.entries) {
    lines.push(formatDesktopLogLine(entry));
  }
  return lines.join('\n');
}

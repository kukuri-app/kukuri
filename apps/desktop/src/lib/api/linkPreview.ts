import { invokeDesktop } from './invoke/desktop';

export type LinkPreview = {
  url: string;
  source_label: string;
  title: string;
  description: string | null;
  image_data_url: string | null;
};

export type LinkPreviewUnavailableReason =
  | 'invalid_url'
  | 'blocked_target'
  | 'redirect_rejected'
  | 'too_many_redirects'
  | 'busy'
  | 'timeout'
  | 'network'
  | 'http_status'
  | 'unsupported_content'
  | 'response_too_large'
  | 'missing_metadata';

export type LinkPreviewOutcome =
  | { status: 'available'; preview: LinkPreview }
  | { status: 'unavailable'; reason: LinkPreviewUnavailableReason };

// objectId は primary content の投稿。native は自分の公開投稿なら record を書き、Web はその record を読む(ADR 0051 §7)。
export type LinkPreviewFetcher = (url: string, objectId: string) => Promise<LinkPreviewOutcome>;

export function fetchLinkPreview(url: string, objectId: string): Promise<LinkPreviewOutcome> {
  // 表示は自分の取得の結果。投稿者の record も読んで保持し、他の参加者へ中継する(#1220 AC-2f)。
  void readLinkPreviewRecord(url, objectId);
  return invokeDesktop<LinkPreviewOutcome>('fetch_link_preview', { url, objectId });
}

// record の読取りの結果は、native の取得と同じ上限と期限で手元に持つ(ADR 0051 §4)。
const RECORD_CACHE_ENTRIES = 128;
const RECORD_CACHE_BYTES = 16 * 1024 * 1024;
const RECORD_SUCCESS_TTL_MS = 10 * 60 * 1000;
const RECORD_FAILURE_TTL_MS = 60 * 1000;
const RECORD_UNAVAILABLE: LinkPreviewOutcome = { status: 'unavailable', reason: 'missing_metadata' };
type RecordEntry = { expiresAt: number; bytes: number; outcome: Promise<LinkPreviewOutcome> };
const recordCache = new Map<string, RecordEntry>();
let recordCacheBytes = 0;

function previewBytes(preview: LinkPreview): number {
  return [preview.url, preview.source_label, preview.title, preview.description, preview.image_data_url]
    .reduce((total, value) => total + (value?.length ?? 0), 0);
}

function forgetRecord(key: string) {
  recordCacheBytes -= recordCache.get(key)?.bytes ?? 0;
  recordCache.delete(key);
}

export function readLinkPreviewRecord(url: string, objectId: string): Promise<LinkPreviewOutcome> {
  const key = `${objectId}\n${url}`;
  const cached = recordCache.get(key);
  if (cached && cached.expiresAt > Date.now()) return cached.outcome;
  forgetRecord(key);
  if (recordCache.size >= RECORD_CACHE_ENTRIES) {
    forgetRecord(recordCache.keys().next().value!);
  }
  const entry: RecordEntry = {
    expiresAt: Number.POSITIVE_INFINITY,
    bytes: 0,
    outcome: Promise.resolve(RECORD_UNAVAILABLE),
  };
  entry.outcome = invokeDesktop<LinkPreview | null>('read_link_preview_record', { objectId, url })
    .then(
      (preview): LinkPreviewOutcome => {
        if (recordCache.get(key) === entry) {
          entry.expiresAt = Date.now() + (preview ? RECORD_SUCCESS_TTL_MS : RECORD_FAILURE_TTL_MS);
          entry.bytes = preview ? previewBytes(preview) : 0;
          recordCacheBytes += entry.bytes;
          for (const [oldest, other] of recordCache) {
            if (recordCacheBytes <= RECORD_CACHE_BYTES) break;
            if (other.bytes > 0) forgetRecord(oldest);
          }
        }
        return preview ? { status: 'available', preview } : RECORD_UNAVAILABLE;
      },
      () => {
        // 断られた(上限)・失敗した読取りは持たず、次の表示で読み直す(native の取得の busy と同じ)。
        if (recordCache.get(key) === entry) recordCache.delete(key);
        return RECORD_UNAVAILABLE;
      }
    );
  recordCache.set(key, entry);
  return entry.outcome;
}

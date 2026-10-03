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
  return invokeDesktop<LinkPreviewOutcome>('fetch_link_preview', { url, objectId });
}

// Web の record の読取りの結果は、native の取得と同じ上限と期限で手元に持つ(ADR 0051 §4)。
const RECORD_CACHE_ENTRIES = 128;
const RECORD_SUCCESS_TTL_MS = 10 * 60 * 1000;
const RECORD_FAILURE_TTL_MS = 60 * 1000;
const RECORD_UNAVAILABLE: LinkPreviewOutcome = { status: 'unavailable', reason: 'missing_metadata' };
const recordCache = new Map<string, { expiresAt: number; outcome: Promise<LinkPreviewOutcome> }>();

export function readLinkPreviewRecord(url: string, objectId: string): Promise<LinkPreviewOutcome> {
  const key = `${objectId}\n${url}`;
  const cached = recordCache.get(key);
  if (cached && cached.expiresAt > Date.now()) return cached.outcome;
  recordCache.delete(key);
  if (recordCache.size >= RECORD_CACHE_ENTRIES) {
    recordCache.delete(recordCache.keys().next().value!);
  }
  const entry = { expiresAt: Number.POSITIVE_INFINITY, outcome: Promise.resolve(RECORD_UNAVAILABLE) };
  entry.outcome = invokeDesktop<LinkPreview | null>('read_link_preview_record', { objectId, url })
    .then(
      (preview): LinkPreviewOutcome =>
        preview ? { status: 'available', preview } : RECORD_UNAVAILABLE,
      () => RECORD_UNAVAILABLE
    )
    .then((outcome) => {
      entry.expiresAt =
        Date.now() +
        (outcome.status === 'available' ? RECORD_SUCCESS_TTL_MS : RECORD_FAILURE_TTL_MS);
      return outcome;
    });
  recordCache.set(key, entry);
  return entry.outcome;
}

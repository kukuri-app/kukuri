import { DISPLAY_RETRY_ATTEMPTS, DISPLAY_RETRY_DELAYS_MS, DISPLAY_RETRY_LIMIT } from '@/lib/displayRetryPolicy';

/// #1207: メディア取得の試行台帳。hash 単位で自動取得の回数と間隔を管理する。
///
/// 1 試行 = `getBlobMediaPayload` 1 回(Rust 側では peer 走査 1 系列)。台帳は shell の data 層に
/// 1 つだけ置き、component の mount や state の参照の変化とは無関係に回数を数える。
/// リセットするのは、利用者の明示再試行、attachment の status が `Available` へ変わったとき、
/// 成人向け gate の切替、api の差し替え(=別の backend)だけとする。

/// 自動取得の最大試行数(初回 + 再試行 3 回)。
export const MEDIA_FETCH_MAX_AUTO_ATTEMPTS = DISPLAY_RETRY_ATTEMPTS;
/// n 回目の失敗から次の試行までの待ち時間。
export const MEDIA_FETCH_RETRY_DELAYS_MS: readonly number[] = DISPLAY_RETRY_DELAYS_MS;
/// 実際に参照する待ち時間。表示の結合 test が実時間を待たずに上限到達を再現するためだけに差し替える。
export const mediaFetchRetryPolicy: { retryDelaysMs: readonly number[] } = {
  retryDelaysMs: MEDIA_FETCH_RETRY_DELAYS_MS,
};
/// 利用者の明示再試行は 1 回だけ試し、結果をすぐ返す。
export const MEDIA_FETCH_MANUAL_ATTEMPTS = 1;
/// 台帳の上限。満杯なら取得中でない古い項目を一つ捨て、全件取得中なら新規を延期する。
export const MEDIA_FETCH_LEDGER_LIMIT = DISPLAY_RETRY_LIMIT;
export const MEDIA_FETCH_FAILURE_KEY_MAX_BYTES = 256;
export const MEDIA_MEMORY_BUDGET_BYTES = 128 * 1024 * 1024;
/// #1419: 画面外へ出た取得済みの画像を、戻したときに取り直さないよう残す上限(件数とメモリ)。
export const MEDIA_HIDDEN_RETAIN_LIMIT = 48;
export const MEDIA_HIDDEN_RETAIN_BYTES = MEDIA_MEMORY_BUDGET_BYTES / 2;
export type MediaResource = { url: string; memoryBytes: number; release: () => void };
const encoder = new TextEncoder();

function validKey(hash: string): boolean {
  return (
    hash.length <= MEDIA_FETCH_FAILURE_KEY_MAX_BYTES &&
    encoder.encode(hash).length <= MEDIA_FETCH_FAILURE_KEY_MAX_BYTES
  );
}

type LedgerEntry = {
  attempts: number;
  maxAttempts: number;
  inFlight: boolean;
  nextRetryAt: number | null;
  exhausted: boolean;
  lastStatus: string | null;
  reservedBytes: number;
  cancel?: () => void;
  resource?: MediaResource;
};

export type MediaFetchDecision =
  | { kind: 'fetch'; attempt: number }
  | { kind: 'wait'; retryAt: number }
  | { kind: 'skip' };

export type MediaFetchFailureOutcome =
  | { kind: 'retry'; retryAt: number }
  | { kind: 'exhausted' };

export class MediaFetchLedger {
  private readonly entries = new Map<string, LedgerEntry>();
  private retainedBytes = 0;
  private pendingBytes = 0;

  /// この hash を今取得してよいかを決める。`fetch` を返したときは試行を 1 回消費し、取得中にする。
  decide(hash: string, status: string | null, now: number, reserveBytes = 0): MediaFetchDecision {
    if (!validKey(hash)) {
      return { kind: 'skip' };
    }
    let entry = this.entries.get(hash);
    if (entry && !entry.inFlight && !entry.resource && status === 'Available' && entry.lastStatus !== 'Available') {
      // backend がローカルに揃ったと報告した。失敗の記録を引き継がずに取り直す。
      this.entries.delete(hash);
      entry = undefined;
    }
    if (!entry) {
      if (!this.makeRoom()) {
        return { kind: 'skip' };
      }
      entry = {
        attempts: 0,
        maxAttempts: MEDIA_FETCH_MAX_AUTO_ATTEMPTS,
        inFlight: false,
        nextRetryAt: null,
        exhausted: false,
        lastStatus: status,
        reservedBytes: 0,
      };
      this.entries.set(hash, entry);
    }
    entry.lastStatus = status;
    if (entry.inFlight || entry.exhausted || entry.resource) {
      return { kind: 'skip' };
    }
    if (entry.nextRetryAt !== null && entry.nextRetryAt > now) {
      return { kind: 'wait', retryAt: entry.nextRetryAt };
    }
    if (reserveBytes < 0 || this.retainedBytes + this.pendingBytes + reserveBytes > MEDIA_MEMORY_BUDGET_BYTES) {
      return { kind: 'skip' };
    }
    entry.attempts += 1;
    entry.inFlight = true;
    entry.reservedBytes = reserveBytes;
    this.pendingBytes += reserveBytes;
    entry.nextRetryAt = null;
    // 挿入順を「最後に試行した順」に保つ(上限超過時に古いものから捨てるため)。
    this.entries.delete(hash);
    this.entries.set(hash, entry);
    return { kind: 'fetch', attempt: entry.attempts };
  }

  succeed(hash: string, resource?: MediaResource): void {
    const entry = this.entries.get(hash);
    if (!entry) return;
    this.pendingBytes -= entry.reservedBytes;
    entry.reservedBytes = 0;
    entry.cancel = undefined;
    if (!resource) {
      this.entries.delete(hash);
      return;
    }
    entry.inFlight = false;
    entry.resource = resource;
    this.retainedBytes += resource.memoryBytes;
  }

  fail(hash: string, now: number): MediaFetchFailureOutcome {
    const entry = this.entries.get(hash);
    if (!entry) {
      return { kind: 'exhausted' };
    }
    this.pendingBytes -= entry.reservedBytes;
    entry.reservedBytes = 0;
    entry.cancel = undefined;
    entry.inFlight = false;
    if (entry.attempts >= entry.maxAttempts) {
      entry.exhausted = true;
      entry.nextRetryAt = null;
      return { kind: 'exhausted' };
    }
    const delays = mediaFetchRetryPolicy.retryDelaysMs;
    const delay = delays[Math.min(entry.attempts - 1, delays.length - 1)] ?? 0;
    entry.nextRetryAt = now + delay;
    return { kind: 'retry', retryAt: entry.nextRetryAt };
  }

  /// 利用者の明示再試行。取得中なら何もしない(重複実行を防ぐ)。
  requestManualRetry(hash: string): boolean {
    if (!validKey(hash)) {
      return false;
    }
    const entry = this.entries.get(hash);
    if (entry?.inFlight || entry?.resource) {
      return false;
    }
    if (!entry && !this.makeRoom()) {
      return false;
    }
    this.entries.set(hash, {
      attempts: 0,
      maxAttempts: MEDIA_FETCH_MANUAL_ATTEMPTS,
      inFlight: false,
      nextRetryAt: null,
      exhausted: false,
      lastStatus: entry?.lastStatus ?? null,
      reservedBytes: 0,
    });
    return true;
  }

  /// gate の切替などで、この hash の記録を捨てる。取得中の結果は呼出元が epoch で無効にする。
  forget(hash: string): void {
    const entry = this.entries.get(hash);
    if (!entry) return;
    entry.cancel?.();
    entry.resource?.release();
    this.pendingBytes -= entry.reservedBytes;
    this.retainedBytes -= entry.resource?.memoryBytes ?? 0;
    this.entries.delete(hash);
  }

  clear(): void {
    for (const hash of this.entries.keys()) this.forget(hash);
  }

  trackCancel(hash: string, cancel: () => void): void {
    const entry = this.entries.get(hash);
    if (entry?.inFlight) entry.cancel = cancel;
    else cancel();
  }

  /// 表示の対象から外れた hash のうち、解放するもの(#1419)。取得中は止める。取得済みは、最後に表示の対象だった順に
  /// `MEDIA_HIDDEN_RETAIN_LIMIT` 件・`MEDIA_HIDDEN_RETAIN_BYTES` まで残し、残りを古い順に解放する。
  hiddenReleases(visible: ReadonlySet<string>): string[] {
    for (const hash of visible) {
      const entry = this.entries.get(hash);
      if (entry?.resource) {
        this.entries.delete(hash);
        this.entries.set(hash, entry);
      }
    }
    const released: string[] = [];
    let kept = 0;
    let keptBytes = 0;
    for (const [hash, entry] of [...this.entries].reverse()) {
      if (visible.has(hash) || !(entry.inFlight || entry.resource)) continue;
      const bytes = entry.resource?.memoryBytes ?? 0;
      if (entry.resource && kept < MEDIA_HIDDEN_RETAIN_LIMIT && keptBytes + bytes <= MEDIA_HIDDEN_RETAIN_BYTES) {
        kept += 1;
        keptBytes += bytes;
        continue;
      }
      released.push(hash);
    }
    return released;
  }

  demandHashes(): string[] {
    return [...this.entries]
      .filter(([, entry]) => entry.inFlight || entry.resource)
      .map(([hash]) => hash);
  }

  get memoryBytes(): number {
    return this.retainedBytes + this.pendingBytes;
  }

  isInFlight(hash: string): boolean {
    return this.entries.get(hash)?.inFlight ?? false;
  }

  isExhausted(hash: string): boolean {
    return this.entries.get(hash)?.exhausted ?? false;
  }

  get size(): number {
    return this.entries.size;
  }

  private makeRoom(): boolean {
    if (this.entries.size < MEDIA_FETCH_LEDGER_LIMIT) {
      return true;
    }
    for (const [hash, entry] of this.entries) {
      if (!entry.inFlight && !entry.resource) {
        this.entries.delete(hash);
        return true;
      }
    }
    return false;
  }
}

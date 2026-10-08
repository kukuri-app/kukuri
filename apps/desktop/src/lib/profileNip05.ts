import { invokeDesktop } from './api/invoke/desktop';
import { isTauriRuntime } from './releaseReadiness';

export type ProfileNip05 = {
  identifier: string;
  name: string;
  domain: string;
};

const NAME = /^[a-z0-9._-]{1,64}$/;
const DOMAIN_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

/// NIP-05 の識別子 `name@domain` の前後の空白を除いて英大文字を小文字にし、名前とドメインに分ける。
/// 形は crates/core の `normalize_profile_nip05` と同じ(ADR 0064 §1)。合わなければ null。
export function parseProfileNip05(value: string): ProfileNip05 | null {
  const identifier = value.trim().replace(/[A-Z]+/g, (letters) => letters.toLowerCase());
  const at = identifier.indexOf('@');
  if (at < 0) return null;
  const name = identifier.slice(0, at);
  const domain = identifier.slice(at + 1);
  const labels = domain.split('.');
  const valid =
    NAME.test(name) &&
    domain.length <= 253 &&
    labels.length >= 2 &&
    labels.every((label) => DOMAIN_LABEL.test(label)) &&
    /[a-z]/.test(labels[labels.length - 1]);
  return valid ? { identifier, name, domain } : null;
}

// 照会の結果の保持と上限(ADR 0064 §4)。デスクトップと Web で共通。
const CACHE_ENTRIES = 128;
const VERIFIED_TTL_MS = 10 * 60 * 1000;
const UNVERIFIED_TTL_MS = 60 * 1000;
const MAX_ACTIVE = 4;
const MAX_WAITING = 32;
const WEB_TIMEOUT_MS = 6_000;
const MAX_DOCUMENT_BYTES = 512 * 1024;

const cache = new Map<string, { expiresAt: number; verified: Promise<boolean> }>();
const waiting: Array<() => void> = [];
let active = 0;

/// 著者の識別子のドメインが、その名前で著者の公開鍵を示すなら、表示するドメインを返す(ADR 0064 §3・§4)。
/// 同じ(公開鍵, 識別子)は保持中に再送しない。待ちが上限を超えたら照会せず null を返し、結果を持たない(次の表示で照会する)。
export async function verifyProfileNip05(pubkey: string, nip05: string): Promise<string | null> {
  const claim = parseProfileNip05(nip05);
  if (!claim) return null;
  const key = `${pubkey}\n${claim.identifier}`;
  let entry = cache.get(key);
  if (!entry || entry.expiresAt <= Date.now()) {
    cache.delete(key);
    if (active >= MAX_ACTIVE && waiting.length >= MAX_WAITING) return null;
    const created = { expiresAt: Number.POSITIVE_INFINITY, verified: Promise.resolve(false) };
    created.verified = withSlot(() =>
      isTauriRuntime()
        ? invokeDesktop<boolean>('verify_profile_domain', { pubkey, identifier: claim.identifier })
        : fetchVerified(pubkey, claim)
    )
      .catch(() => false)
      .then((verified) => {
        if (cache.get(key) === created) {
          created.expiresAt = Date.now() + (verified ? VERIFIED_TTL_MS : UNVERIFIED_TTL_MS);
        }
        return verified;
      });
    if (cache.size >= CACHE_ENTRIES) cache.delete(cache.keys().next().value!);
    cache.set(key, created);
    entry = created;
  }
  return (await entry.verified) ? claim.domain : null;
}

/// 同時の照会を 4 件までにする。空きを待つ照会には、終わった照会の枠をそのまま渡す。
async function withSlot<T>(task: () => Promise<T>): Promise<T> {
  if (active < MAX_ACTIVE) active += 1;
  else await new Promise<void>((resolve) => waiting.push(resolve));
  try {
    return await task();
  } finally {
    const next = waiting.shift();
    if (next) next();
    else active -= 1;
  }
}

/// Web(とブラウザ mock)は、ブラウザから取得する。cookie と Referer を送らず、転送はエラーにする。
async function fetchVerified(pubkey: string, claim: ProfileNip05): Promise<boolean> {
  const response = await fetch(
    `https://${claim.domain}/.well-known/nostr.json?name=${claim.name}`,
    {
      credentials: 'omit',
      redirect: 'error',
      referrerPolicy: 'no-referrer',
      signal: AbortSignal.timeout(WEB_TIMEOUT_MS),
    }
  );
  if (!response.ok || !response.body) return false;
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let text = '';
  let bytes = 0;
  for (let chunk = await reader.read(); !chunk.done; chunk = await reader.read()) {
    bytes += chunk.value.byteLength;
    if (bytes > MAX_DOCUMENT_BYTES) {
      await reader.cancel();
      return false;
    }
    text += decoder.decode(chunk.value, { stream: true });
  }
  const json = JSON.parse(text + decoder.decode()) as { names?: Record<string, unknown> } | null;
  return json?.names?.[claim.name] === pubkey;
}

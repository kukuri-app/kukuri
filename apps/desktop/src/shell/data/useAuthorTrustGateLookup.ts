import { useEffect, useMemo, useRef } from 'react';

import type {
  AuthorTrustGate,
  CommunityNodeConfig,
  CommunityNodeNodeStatus,
  DesktopApi,
  PostView,
} from '@/lib/api';
import { useDesktopShellStoreApi } from '@/shell/store';
import { retainRecordEntries } from '@/shell/stateUpdates';

/// 同じ描画更新でまとめて照会するための待ち時間。
export const AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS = 300;
/// 1 回の照会で送る著者数の上限（CN 側の一括評価の上限と同じ）。
export const AUTHOR_TRUST_GATE_LOOKUP_BATCH_SIZE = 100;
/// 期限を持たない判断（未評価・照会失敗・照会中）を作り直すまでの待ち時間。
export const AUTHOR_TRUST_GATE_LOOKUP_FALLBACK_TTL_MS = 600_000;
/// 期限切れの判断を照会し直す間隔。
export const AUTHOR_TRUST_GATE_LOOKUP_SWEEP_MS = 30_000;
/// 期限を過ぎた判断を、作り直しの応答を待つあいだ使い続けてよい猶予。
///
/// 掃除の間隔と合わせて、期限からの上限は最大 2 回ぶん（約 60 秒）になる。応答が返らない
/// CN があっても、それ以上は古い判断で折りたたまない（#1061 TR-4）。
export const AUTHOR_TRUST_GATE_LOOKUP_STALE_GRACE_MS = 30_000;

/// #1061: 投稿カードから、表示判断の対象になる著者を集める。
///
/// 引用・repost では元投稿の著者も対象にする（ADR 0022 と同じ扱い）。
export function postTrustGateAuthors(post: PostView): string[] {
  const authors = [post.author_pubkey, post.repost_of?.source_author_pubkey];
  return authors
    .map((pubkey) => pubkey?.trim())
    .filter((pubkey): pubkey is string => Boolean(pubkey));
}

/// #1061: 採用順位に選んだ node と、その認証・必須同意の状態。
///
/// 同意取消・再認証・CN 削除で変わるため、これが変わったら判断をすべて作り直す
/// （runtime 側の cache 世代の更新と対になる）。
export function trustGateAdoptionSignature(
  config: CommunityNodeConfig,
  statuses: readonly CommunityNodeNodeStatus[]
): string {
  const statusByUrl = new Map(statuses.map((status) => [status.base_url, status]));
  return (config.trust_node_priority ?? [])
    .map((baseUrl) => {
      const status = statusByUrl.get(baseUrl);
      const authenticated = status?.auth_state.authenticated ? '1' : '0';
      const consented = status?.consent_state?.all_required_accepted ? '1' : '0';
      return `${baseUrl}:${authenticated}${consented}`;
    })
    .join('|');
}

/// 判断の作り直しと破棄の時刻（epoch ms）。
type TrustGateExpiry = {
  /// この時刻を過ぎたら照会し直す（評価の期限）。
  refreshAt: number;
  /// この時刻を過ぎたら判断を捨てる（作り直せないまま使い続けない）。
  dropAt: number;
};

/// 判断を使ってよい期限（epoch ms）。期限が無い・読めない場合は fallback を使う。
function gateExpiry(gate: AuthorTrustGate, fallback: number): number {
  if (!gate.expires_at) return fallback;
  const parsed = Date.parse(gate.expires_at);
  return Number.isNaN(parsed) ? fallback : parsed;
}

export type UseAuthorTrustGateLookupArgs = {
  api: Pick<DesktopApi, 'evaluateAuthorTrustGates'>;
  /// 表示中の投稿（タイムライン・スレッド・プロフィール・ブックマーク）。
  posts: readonly PostView[];
  /// live / game 一覧の主催者など、投稿以外の著者。
  hostPubkeys?: readonly string[];
  /// 採用順位。空なら照会しない（この機能による非表示を行わない）。
  config: CommunityNodeConfig;
  /// 採用順位に選んだ node の認証・同意の状態。
  statuses?: readonly CommunityNodeNodeStatus[];
  /// 上の状態を読み終えたか。読む前は照会しない（未確定の状態で判断を作らない）。
  statusesLoaded?: boolean;
};

/// #1061: 表示中の著者を採用 CN へ一括照会し、折りたたみ判断を shell state へ置く。
///
/// - 採用順位が空なら照会しない。送るのは著者 pubkey だけで、投稿本文や閲覧履歴は送らない。
/// - 判断は評価の期限（ADR 0026 §8.4、既定 600 秒）まで使い、過ぎたら照会し直して差し替える。
///   応答が届くまでは猶予のあいだだけ前の判断のままにして、折りたたんだ投稿が一瞬開かない
///   ようにする。猶予を過ぎても作り直せなければ判断を捨てる（折りたたまない）。
/// - 採用順位・認証・必須同意が変わったら、すべての判断を捨てて照会し直す。
/// - 失敗は runtime 側が「未評価」として返すため、折りたたみは起きない（fail-open）。
export function useAuthorTrustGateLookup({
  api,
  posts,
  hostPubkeys = [],
  config,
  statuses = [],
  statusesLoaded = true,
}: UseAuthorTrustGateLookupArgs) {
  const storeApi = useDesktopShellStoreApi();
  const priority = config.trust_node_priority ?? [];
  const active = priority.length > 0 && statusesLoaded;
  const adoptionSignature = trustGateAdoptionSignature(config, statuses);
  // 照会済みの著者と、作り直す時刻・使うのをやめる時刻（epoch ms）。
  const expiryRef = useRef<Map<string, TrustGateExpiry>>(new Map());
  const queueRef = useRef<Set<string>>(new Set());
  const requestTokensRef = useRef<Map<string, number>>(new Map());
  const nextRequestTokenRef = useRef(0);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const generationRef = useRef(0);
  const apiRef = useRef(api);
  const authorsRef = useRef<ReadonlySet<string>>(new Set<string>());
  useEffect(() => {
    apiRef.current = api;
  }, [api]);

  const authorsKey = useMemo(() => {
    const unique = new Set<string>();
    for (const post of posts) {
      for (const author of postTrustGateAuthors(post)) unique.add(author);
    }
    for (const host of hostPubkeys) {
      const trimmed = host.trim();
      if (trimmed) unique.add(trimmed);
    }
    return [...unique].sort().join(',');
  }, [hostPubkeys, posts]);

  // 採用順位・認証・必須同意が変わったら、以前の判断と照会済み記録を捨てる。
  useEffect(() => {
    generationRef.current += 1;
    expiryRef.current = new Map();
    queueRef.current = new Set();
    requestTokensRef.current = new Map();
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    storeApi.getState().setField('authorTrustGates', {});
  }, [adoptionSignature, storeApi]);

  useEffect(() => {
    const authors = new Set(authorsKey ? authorsKey.split(',') : []);
    authorsRef.current = authors;
    for (const author of expiryRef.current.keys()) {
      if (!authors.has(author)) expiryRef.current.delete(author);
    }
    for (const author of queueRef.current) {
      if (!authors.has(author)) queueRef.current.delete(author);
    }
    for (const author of requestTokensRef.current.keys()) {
      if (!authors.has(author)) requestTokensRef.current.delete(author);
    }
    storeApi.getState().setField('authorTrustGates', (current) => retainRecordEntries(current, authors));
    if (!active) return;

    async function lookupBatch(batch: string[], generation: number) {
      const fallback = Date.now() + AUTHOR_TRUST_GATE_LOOKUP_FALLBACK_TTL_MS;
      const requested = new Set(batch);
      const tokens = new Map(batch.map((author) => [author, requestTokensRef.current.get(author)]));
      const isCurrent = (author: string) =>
        authorsRef.current.has(author) && tokens.get(author) === requestTokensRef.current.get(author);
      try {
        const result = await apiRef.current.evaluateAuthorTrustGates({ author_pubkeys: batch });
        if (generation !== generationRef.current) return;
        const additions: Record<string, AuthorTrustGate> = {};
        for (const gate of result.gates) {
          if (!requested.has(gate.author_pubkey) || !isCurrent(gate.author_pubkey)) continue;
          additions[gate.author_pubkey] = gate;
          const refreshAt = gateExpiry(gate, fallback);
          expiryRef.current.set(gate.author_pubkey, {
            refreshAt,
            dropAt: refreshAt + AUTHOR_TRUST_GATE_LOOKUP_STALE_GRACE_MS,
          });
        }
        // 応答に含まれなかった著者の判断は残さない（作り直しに失敗した扱い）。
        const dropped = batch.filter((author) => isCurrent(author) && !(author in additions));
        storeApi.getState().setField('authorTrustGates', (current) => {
          const next = { ...current, ...additions };
          for (const author of dropped) delete next[author];
          return Object.keys(additions).length || dropped.length ? next : current;
        });
      } catch {
        // 照会できない著者は未評価として扱い、折りたたみをやめる（fail-open）。
        if (generation !== generationRef.current) return;
        for (const author of batch) {
          if (!isCurrent(author)) continue;
          expiryRef.current.set(author, {
            refreshAt: fallback,
            dropAt: fallback + AUTHOR_TRUST_GATE_LOOKUP_STALE_GRACE_MS,
          });
        }
        storeApi.getState().setField('authorTrustGates', (current) => {
          const dropped = batch.filter((author) => isCurrent(author) && author in current);
          if (dropped.length === 0) return current;
          const next = { ...current };
          for (const author of dropped) delete next[author];
          return next;
        });
      }
    }

    function flush() {
      timerRef.current = null;
      const batch = [...queueRef.current];
      queueRef.current = new Set();
      const generation = generationRef.current;
      for (let offset = 0; offset < batch.length; offset += AUTHOR_TRUST_GATE_LOOKUP_BATCH_SIZE) {
        void lookupBatch(
          batch.slice(offset, offset + AUTHOR_TRUST_GATE_LOOKUP_BATCH_SIZE),
          generation
        );
      }
    }

    /// 期限切れの判断を照会し直し、判断を持たない著者を照会待ちに入れる。
    function refresh() {
      const now = Date.now();
      // 猶予を過ぎても作り直せなかった判断は捨てる（古い判断で折りたたみ続けない）。
      const dropped: string[] = [];
      for (const [author, expiry] of expiryRef.current) {
        if (expiry.dropAt > now) continue;
        expiryRef.current.delete(author);
        dropped.push(author);
      }
      if (dropped.length > 0) {
        storeApi.getState().setField('authorTrustGates', (current) => {
          const next = { ...current };
          for (const author of dropped) delete next[author];
          return next;
        });
      }
      for (const author of authorsRef.current) {
        const expiry = expiryRef.current.get(author);
        if (expiry && expiry.refreshAt > now) continue;
        // 照会中も期限として扱い、応答が返るまで同じ著者を二重に送らない。
        // 前の判断は猶予のあいだ残す（折りたたんだ投稿を一瞬開かせない）。
        const refreshAt = now + AUTHOR_TRUST_GATE_LOOKUP_FALLBACK_TTL_MS;
        expiryRef.current.set(author, {
          refreshAt,
          dropAt: expiry?.dropAt ?? refreshAt + AUTHOR_TRUST_GATE_LOOKUP_STALE_GRACE_MS,
        });
        requestTokensRef.current.set(author, ++nextRequestTokenRef.current);
        queueRef.current.add(author);
      }
      // 直前の cleanup で timer が消えている場合も、待ち行列が残っていれば張り直す。
      if (queueRef.current.size === 0 || timerRef.current) return;
      timerRef.current = setTimeout(flush, AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
    }

    refresh();
    const sweep = setInterval(refresh, AUTHOR_TRUST_GATE_LOOKUP_SWEEP_MS);

    return () => {
      clearInterval(sweep);
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [active, authorsKey, storeApi]);
}

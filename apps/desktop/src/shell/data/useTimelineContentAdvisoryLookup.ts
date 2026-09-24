import { useEffect, useMemo, useRef } from 'react';

import type {
  CommunityNodeConfig,
  CommunityNodeNodeStatus,
  DesktopApi,
  NotificationView,
  PostView,
} from '@/lib/api';
import {
  advisorySubjectKey,
  postAdvisorySubjects,
  type AdvisorySubjectRef,
  type TimelineContentAdvisoryIndex,
} from '@/shell/contentAdvisories';
import { useDesktopShellStoreApi } from '@/shell/store';
import { retainRecordEntries } from '@/shell/stateUpdates';

/// 同じ描画更新でまとめて照会するための待ち時間。
export const TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS = 300;
/// 1 回の command 呼出しで送る subject の上限(runtime 側の上限 1000 未満)。
export const TIMELINE_ADVISORY_LOOKUP_BATCH_SIZE = 500;
/// 応答が遅い node でスケルトンのまま止まらないよう、未決を解除するまでの上限。
export const TIMELINE_ADVISORY_LOOKUP_SETTLE_TIMEOUT_MS = 10_000;

export type AdoptingContentAdvisoryNodes = {
  /// 照会先になりうる node(採用 ON・認証済み・必須同意承認済み・通信エラーなし)。
  baseUrls: string[];
  /// node 設定または状態が未取得で、照会先の有無をまだ決められない。
  undetermined: boolean;
};

/// #1056: advisory の照会先を決める。実際の送信可否は desktop-runtime が HTTP の手前で再判定する
/// (ここは照会と照会中表示の要否だけを決める)。
export function adoptingContentAdvisoryNodes(
  config: CommunityNodeConfig,
  statuses: readonly CommunityNodeNodeStatus[],
  loaded: { configLoaded: boolean; statusesLoaded: boolean } = {
    configLoaded: true,
    statusesLoaded: true,
  }
): AdoptingContentAdvisoryNodes {
  if (!loaded.configLoaded) {
    return { baseUrls: [], undetermined: true };
  }
  const statusByUrl = new Map(statuses.map((status) => [status.base_url, status]));
  const enabled = config.nodes
    .filter((node) => node.content_advisory_enabled !== false)
    .map((node) => node.base_url);
  const undetermined =
    enabled.length > 0 &&
    (!loaded.statusesLoaded || enabled.some((baseUrl) => !statusByUrl.has(baseUrl)));
  const baseUrls = enabled.filter((baseUrl) => {
    const status = statusByUrl.get(baseUrl);
    return Boolean(
      status?.auth_state.authenticated &&
        status.consent_state?.all_required_accepted &&
        !status.last_error
    );
  });
  return { baseUrls, undetermined };
}

type UseTimelineContentAdvisoryLookupArgs = {
  api: Pick<DesktopApi, 'lookupCommunityNodeContentAdvisories'>;
  /// 表示中の投稿(タイムライン・スレッド・ブックマーク・プロフィール等)。
  posts: readonly PostView[];
  notifications: readonly NotificationView[];
  /// `adoptingContentAdvisoryNodes` の結果。照会先が無く確定していれば、照会も照会中表示もしない。
  adoptingNodes: AdoptingContentAdvisoryNodes;
};

/// #1056: 可視 subject を採用 node へ一括照会し、結果と照会の進み具合を shell state へ置く。
///
/// - 照会先がある、または未確定(起動直後に node 状態が未取得)の間は `active`。`active` の間は
///   照会済みでない subject を照会中とみなすため、投稿が描画された時点で取得が止まる。
///   確定前にメディアを取得すると、ゲートや ephemeral 取得の判定より先に bytes が届くため。
/// - 照会は表示中の subject ごとに 1 回。非表示後は結果を破棄し、再表示時に照会し直す。
/// - 応答・失敗・上限時間のいずれかで照会済みにする。失敗時は advisory 無しとして通常表示へ戻す
///   (ADR 0046 §6.5 の fail-open)。
/// - 送る識別子は post id と blob hash だけ(INVAR-1)。
export function useTimelineContentAdvisoryLookup({
  api,
  posts,
  notifications,
  adoptingNodes,
}: UseTimelineContentAdvisoryLookupArgs) {
  const storeApi = useDesktopShellStoreApi();
  const undetermined = adoptingNodes.undetermined;
  const active = undetermined || adoptingNodes.baseUrls.length > 0;
  const nodesSignature = undetermined
    ? '<undetermined>'
    : [...adoptingNodes.baseUrls].sort().join('|');
  const requestedRef = useRef<Set<string>>(new Set());
  const queueRef = useRef<Map<string, AdvisorySubjectRef>>(new Map());
  const requestTokensRef = useRef<Map<string, number>>(new Map());
  const nextRequestTokenRef = useRef(0);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const generationRef = useRef(0);
  const apiRef = useRef(api);
  const subjectsRef = useRef<Map<string, AdvisorySubjectRef>>(new Map());
  const activeKeysRef = useRef<ReadonlySet<string>>(new Set());
  useEffect(() => {
    apiRef.current = api;
  }, [api]);

  const subjects = useMemo(() => {
    const unique = new Map<string, AdvisorySubjectRef>();
    for (const post of posts) {
      for (const subject of postAdvisorySubjects(post)) {
        unique.set(advisorySubjectKey(subject.kind, subject.id), subject);
      }
    }
    for (const notification of notifications) {
      const objectId = notification.object_id?.trim();
      if (objectId) {
        unique.set(advisorySubjectKey('post_id', objectId), { kind: 'post_id', id: objectId });
      }
    }
    return unique;
  }, [notifications, posts]);
  const subjectKeys = [...subjects.keys()].sort().join('\0');
  useEffect(() => {
    subjectsRef.current = subjects;
  }, [subjects]);

  // 採用 node の組が変わったら、以前の結果・照会済み記録・送信済み記録を捨てる。
  useEffect(() => {
    generationRef.current += 1;
    requestedRef.current = new Set();
    queueRef.current = new Map();
    requestTokensRef.current = new Map();
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    const { setField } = storeApi.getState();
    setField('timelineContentAdvisories', {});
    setField('timelineAdvisoryLookup', { active, settled: {} });
  }, [active, nodesSignature, storeApi]);

  useEffect(() => {
    const subjects = subjectsRef.current;
    const activeKeys = new Set(subjects.keys());
    activeKeysRef.current = activeKeys;
    for (const key of requestedRef.current) {
      if (!activeKeys.has(key)) requestedRef.current.delete(key);
    }
    for (const key of queueRef.current.keys()) {
      if (!activeKeys.has(key)) queueRef.current.delete(key);
    }
    for (const key of requestTokensRef.current.keys()) {
      if (!activeKeys.has(key)) requestTokensRef.current.delete(key);
    }
    storeApi.getState().setField('timelineContentAdvisories', (current) =>
      retainRecordEntries(current, activeKeys));
    storeApi.getState().setField('timelineAdvisoryLookup', (current) => {
      const settled = retainRecordEntries(current.settled, activeKeys);
      return settled === current.settled ? current : { ...current, settled };
    });
    // 照会先が無い(確定)なら何もしない。未確定の間は送らず、照会中のまま待つ。
    if (!active || undetermined) return;
    const fresh: AdvisorySubjectRef[] = [];
    for (const [key, subject] of subjects) {
      if (requestedRef.current.has(key)) continue;
      requestedRef.current.add(key);
      requestTokensRef.current.set(key, ++nextRequestTokenRef.current);
      queueRef.current.set(key, subject);
      fresh.push(subject);
    }
    if (fresh.length === 0 || timerRef.current) return;
    timerRef.current = setTimeout(() => {
      timerRef.current = null;
      const queued = [...queueRef.current.entries()];
      queueRef.current = new Map();
      const generation = generationRef.current;
      for (let offset = 0; offset < queued.length; offset += TIMELINE_ADVISORY_LOOKUP_BATCH_SIZE) {
        void lookupBatch(
          queued.slice(offset, offset + TIMELINE_ADVISORY_LOOKUP_BATCH_SIZE),
          generation
        );
      }
    }, TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);

    async function lookupBatch(batch: [string, AdvisorySubjectRef][], generation: number) {
      const keys = batch.map(([key]) => key);
      const requested = new Set(keys);
      const tokens = new Map(keys.map((key) => [key, requestTokensRef.current.get(key)]));
      const isCurrent = (key: string) =>
        activeKeysRef.current.has(key) && tokens.get(key) === requestTokensRef.current.get(key);
      let settled = false;
      const settle = () => {
        if (settled || generation !== generationRef.current) return;
        settled = true;
        storeApi.getState().setField('timelineAdvisoryLookup', (current) => {
          const fresh = keys.filter((key) => isCurrent(key) && !current.settled[key]);
          if (fresh.length === 0) return current;
          const next = { ...current.settled };
          for (const key of fresh) next[key] = true;
          return { ...current, settled: next };
        });
      };
      const timeout = setTimeout(settle, TIMELINE_ADVISORY_LOOKUP_SETTLE_TIMEOUT_MS);
      try {
        const result = await apiRef.current.lookupCommunityNodeContentAdvisories({
          post_ids: batch
            .filter(([, subject]) => subject.kind === 'post_id')
            .map(([, subject]) => subject.id),
          blob_hashes: batch
            .filter(([, subject]) => subject.kind === 'blob_cid')
            .map(([, subject]) => subject.id),
        });
        if (generation !== generationRef.current) return;
        const additions: TimelineContentAdvisoryIndex = {};
        for (const node of result.nodes) {
          for (const advisory of node.advisories) {
            const key = advisorySubjectKey(advisory.subject_kind, advisory.subject_id);
            if (!isCurrent(key) || !requested.has(key)) continue;
            (additions[key] ??= []).push({ advisory, nodeBaseUrl: node.base_url });
          }
        }
        if (Object.keys(additions).length > 0) {
          storeApi.getState().setField('timelineContentAdvisories', (current) => {
            const next = { ...current };
            for (const [key, entries] of Object.entries(additions)) {
              next[key] = [...(next[key] ?? []), ...entries];
            }
            return next;
          });
        }
      } catch {
        // 照会に失敗した subject は advisory 無しとして扱う(fail-open)。取得ゲートは Rust 側が別に持つ。
      } finally {
        clearTimeout(timeout);
        settle();
      }
    }
    // 採用 node の組だけが変わった場合も、破棄した結果を照会し直す(nodesSignature)。
  }, [active, nodesSignature, storeApi, subjectKeys, undetermined]);

  useEffect(
    () => () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    },
    []
  );
}

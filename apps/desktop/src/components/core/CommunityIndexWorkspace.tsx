import { type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { TFunction } from 'i18next';
import { Search } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type {
  AuthorSocialView,
  BookmarkedCustomReactionView,
  CommunityIndexPostResolveInput,
  CommunityIndexResolvedPostView,
  CommunityNodeIndexQueryRequest,
  CommunityNodeNodeStatus,
  CustomReactionAssetView,
  DesktopApi,
  IndexEntryView,
  PostView,
  Profile,
  ReactionKeyInput,
  RecentReactionView,
  TimelineScope,
} from '@/lib/api';
import { InvokeError } from '@/lib/api/invoke/error';
import type { InternalSmartReference } from '@/lib/internalLinks';
import { copyTextToClipboard } from '@/lib/utils';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Notice } from '@/components/ui/notice';
import { CommunityNodeConsentDialog } from '@/components/settings/CommunityNodeConsentDialog';
import type { CommunityNodeAvailability } from '@/lib/api/communityNodeAvailability';
import { postGateableMediaHashes } from '@/shell/contentAdvisories';
import { useCommunityNodeConsentFlow, type AcceptCommunityNodeConsents } from '@/shell/actions/useCommunityNodeConsentFlow';
import { CommunityIndexAvailabilityNotice } from './CommunityIndexAvailabilityNotice';
import { CommunityIndexEmptyState } from './CommunityIndexEmptyState';
import type { CommunityIndexingTarget } from './CommunityIndexingRequestDialog';
import {
  type CommunityIndexEmptyIndexStatusInput,
  communityIndexEmptyGuidance,
} from './communityIndexEmptyGuidance';
import { communityIndexPostCardView } from './communityIndexPostCardView';
import {
  useCommunityIndexAdvisories,
  usePublishedAdvisoryHashes,
} from './useCommunityIndexAdvisories';
import { PostCard } from './PostCard';

type IndexOperation = 'search' | 'discovery' | 'recommendations';
const EMPTY_KNOWN_AUTHORS: Record<string, AuthorSocialView> = {};
const EMPTY_UNSUPPORTED_VIDEO_MANIFESTS: Record<string, true> = {};
const EMPTY_GATED_MEDIA_HASHES: readonly string[] = [];

type CommunityIndexWorkspaceProps = {
  api: DesktopApi;
  mode: 'topic' | 'explore';
  activeTopic: string;
  activeTimelineScope: TimelineScope;
  /** private channel 範囲の索引登録申請に使う表示名。未指定なら channel id を使う。 */
  activeChannelLabel?: string | null;
  eligibleNodeBaseUrls: readonly string[];
  // #857: 設定済みだがローカル同意が成立していない(未同意・撤回・再同意待ち)node。
  // Node 機能の利用直前に同意モーダルを提示するために使う。
  consentPendingNodeBaseUrls?: readonly string[];
  selectedNodeBaseUrl: string | null;
  configuredNodeBaseUrls?: readonly string[];
  nodeStatuses?: readonly CommunityNodeNodeStatus[];
  availability?: CommunityNodeAvailability;
  onAcceptConsents?: AcceptCommunityNodeConsents;
  onRetryNode?: (recovery?: 'metadata' | 'manifest' | 'status') => Promise<void>;
  onAutomaticNode?: () => void;
  onOpenCommunityNodeSettings: () => void;
  /** #960: 空状態の次の行動。未指定の導線は表示しない。 */
  onOpenConnectivitySettings?: () => void;
  onOpenTimeline?: () => void;
  onRequestIndexing?: (target: CommunityIndexingTarget) => void;
  knownAuthorsByPubkey?: Record<string, AuthorSocialView>;
  mediaObjectUrls?: Record<string, string | null>;
  adultContentEnabled?: boolean;
  /// #1052: 解決済み投稿の添付をタイムラインと同じ規則で描画するための表示条件。
  unsupportedVideoManifests?: Record<string, true>;
  locale?: string | null;
  /// #1052: 表示中の解決済み投稿。呼出元がメディアのプリフェッチと成人向け取得ゲートの
  /// 対象へ加えるために使う。結果の失効と Column の終了では空配列を通知する。
  onResolvedPostsChange?: (posts: PostView[]) => void;
  /// #1055: 表示設定 OFF のため advisory でゲート中の添付 blob hash。呼出元がプリフェッチの
  /// 除外集合に使う。結果の失効と Column の終了では空配列を通知する。
  /// #1107: 投稿への advisory で代替表示にした解決済み投稿の添付も含める(その投稿は
  /// `onResolvedPostsChange` へ公開しないため、ここで伝えないと同じ blob を別の投稿が表示する)。
  onAdvisoryGatedMediaHashesChange?: (hashes: string[]) => void;
  /// #1107: 表示設定 OFF の間ゲートする添付 blob hash(全表示経路で共通)。
  gatedMediaHashes?: readonly string[];
  onOpenAuthor: (pubkey: string) => void;
  onOpenThread?: (threadId: string) => void;
  onOpenThreadInTopic?: (threadId: string, topicId: string) => void;
  onReply?: (post: PostView) => void;
  onRepost?: (post: PostView) => void;
  onQuoteRepost?: (post: PostView) => void;
  localAuthorPubkey?: string;
  localProfile?: Profile | null;
  ownedReactionAssets?: CustomReactionAssetView[];
  bookmarkedReactionAssets?: BookmarkedCustomReactionView[];
  recentReactions?: RecentReactionView[];
  onToggleReaction?: (post: PostView, reactionKey: ReactionKeyInput) => void;
  onBookmarkCustomReaction?: (asset: CustomReactionAssetView) => void;
  onReactionPickerOpen?: () => void;
  showBookmarkAction?: boolean;
  bookmarkedPostIds?: Set<string>;
  onToggleBookmark?: (post: PostView) => void;
  onWithdraw?: (post: PostView) => void;
  onActivateReference?: (reference: InternalSmartReference) => void;
  onCopyPostLink?: (link: string) => void;
};

type IndexRequestContext = {
  key: string;
  nodeBaseUrl: string;
  operation: IndexOperation;
  scopeKind: CommunityNodeIndexQueryRequest['scope_kind'];
  scopeId: CommunityNodeIndexQueryRequest['scope_id'];
  topicId: CommunityNodeIndexQueryRequest['topic_id'];
};

type IndexResultState = {
  context: IndexRequestContext;
  entries: IndexEntryView[];
  /** 実際に送った検索語。空状態の説明は編集中の入力ではなくこれを使う。 */
  query: string;
};

type ResolvedPostState = {
  contextKey: string;
  entriesByKey: Record<string, CommunityIndexResolvedPostView>;
  statusByKey: Record<string, 'loading' | 'resolved' | 'failed'>;
};

type ResolvedAuthorState = {
  contextKey: string;
  entriesByPubkey: Record<
    string,
    { status: 'loading' | 'resolved' | 'failed'; author: AuthorSocialView | null }
  >;
};

function indexEntryKey(entry: IndexEntryView): string {
  return `${entry.scope_kind}:${entry.scope_id}:${entry.object_id}`;
}

function resolveInputForEntry(
  entry: IndexEntryView,
  context: IndexRequestContext
): CommunityIndexPostResolveInput | null {
  const topic =
    entry.scope_kind === 'public_topic' ? entry.scope_id : context.topicId?.trim() || null;
  if (!topic) return null;
  return {
    key: indexEntryKey(entry),
    topic,
    object_id: entry.object_id,
    author_pubkey: entry.author_pubkey,
    channel_ref:
      entry.scope_kind === 'public_topic'
        ? { kind: 'public' }
        : { kind: 'private_channel', channel_id: entry.scope_id },
  };
}

function localAuthorView(profile: Profile, authorPubkey: string): AuthorSocialView {
  return {
    author_pubkey: authorPubkey,
    name: profile.name ?? null,
    display_name: profile.display_name ?? null,
    about: profile.about ?? null,
    picture_asset: profile.picture_asset ?? null,
    updated_at: profile.updated_at,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    friend_of_friend_via_pubkeys: [],
    provenance: null,
    muted: false,
    blocking: false,
    blocked_by: false,
  };
}

function operationMethod(api: DesktopApi, operation: IndexOperation) {
  if (operation === 'search') return api.searchCommunityNodeIndex.bind(api);
  if (operation === 'discovery') return api.discoverCommunityNodeIndex.bind(api);
  return api.recommendCommunityNodeIndex.bind(api);
}

function communityIndexErrorMessage(error: unknown, t: TFunction): string {
  if (!(error instanceof InvokeError)) {
    return error instanceof Error ? error.message : String(error);
  }
  if (error.code === 'AUTH_REQUIRED' || error.status === 401) {
    return t('shell:communityIndex.errors.authRequired');
  }
  if (error.code === 'CONSENT_REQUIRED') {
    return t('shell:communityIndex.errors.consentRequired');
  }
  if (error.code === 'INDEX_QUERY_NOT_CONFIGURED') {
    return t('shell:communityIndex.errors.notConfigured');
  }
  if (error.code === 'INDEX_QUERY_NOT_ACTIVATED') {
    return t('shell:communityIndex.errors.notActivated');
  }
  if (error.status === 429 || error.code === 'RATE_LIMITED') {
    return error.retryAfterSeconds
      ? t('shell:communityIndex.errors.rateLimitedWithRetry', {
          seconds: error.retryAfterSeconds,
        })
      : t('shell:communityIndex.errors.rateLimited');
  }
  if (error.status === 403) return t('shell:communityIndex.availability.admissionDenied');
  return error.message;
}

function indexContext(
  mode: CommunityIndexWorkspaceProps['mode'],
  operation: IndexOperation,
  selectedNodeBaseUrl: string | null,
  activeTopic: string,
  activeTimelineScope: TimelineScope
): IndexRequestContext | null {
  if (!selectedNodeBaseUrl) return null;
  const scopeKind =
    mode !== 'topic'
      ? null
      : activeTimelineScope.kind === 'channel'
        ? 'private_channel'
        : 'public_topic';
  const scopeId =
    mode !== 'topic'
      ? null
      : activeTimelineScope.kind === 'channel'
        ? activeTimelineScope.channel_id
        : activeTopic;
  return {
    key: [
      selectedNodeBaseUrl,
      operation,
      mode,
      activeTimelineScope.kind,
      scopeKind ?? '',
      scopeId ?? '',
    ].join('\u0000'),
    nodeBaseUrl: selectedNodeBaseUrl,
    operation,
    scopeKind,
    scopeId,
    // 非公開チャンネル範囲では所属証明(channel secret)を引くために topic が要る(#711)。
    topicId: scopeKind === 'private_channel' ? activeTopic : null,
  };
}

export function CommunityIndexWorkspace({
  api,
  mode,
  activeTopic,
  activeTimelineScope,
  activeChannelLabel = null,
  eligibleNodeBaseUrls,
  consentPendingNodeBaseUrls = [],
  selectedNodeBaseUrl,
  configuredNodeBaseUrls,
  nodeStatuses,
  availability,
  onAcceptConsents,
  onRetryNode = async () => {},
  onAutomaticNode = () => {},
  onOpenCommunityNodeSettings,
  onOpenConnectivitySettings,
  onOpenTimeline,
  onRequestIndexing,
  knownAuthorsByPubkey = EMPTY_KNOWN_AUTHORS,
  mediaObjectUrls = {},
  adultContentEnabled = false,
  unsupportedVideoManifests = EMPTY_UNSUPPORTED_VIDEO_MANIFESTS,
  locale = null,
  onResolvedPostsChange,
  onAdvisoryGatedMediaHashesChange,
  gatedMediaHashes = EMPTY_GATED_MEDIA_HASHES,
  onOpenAuthor,
  onOpenThread,
  onOpenThreadInTopic,
  onReply,
  onRepost,
  onQuoteRepost,
  localAuthorPubkey,
  localProfile,
  ownedReactionAssets = [],
  bookmarkedReactionAssets = [],
  recentReactions = [],
  onToggleReaction,
  onBookmarkCustomReaction,
  onReactionPickerOpen,
  showBookmarkAction = false,
  bookmarkedPostIds,
  onToggleBookmark,
  onWithdraw,
  onActivateReference,
  onCopyPostLink,
}: CommunityIndexWorkspaceProps) {
  const { t } = useTranslation(['shell', 'common']);
  const [operation, setOperation] = useState<IndexOperation>('search');
  const consentFlow = useCommunityNodeConsentFlow({
    api,
    configuredBaseUrls: configuredNodeBaseUrls ?? [...eligibleNodeBaseUrls, ...consentPendingNodeBaseUrls],
    statuses: nodeStatuses,
    acceptConsents: onAcceptConsents,
  });
  const [query, setQuery] = useState('');
  const [resultState, setResultState] = useState<IndexResultState | null>(null);
  const [status, setStatus] = useState<'idle' | 'loading' | 'success' | 'error'>('idle');
  const [error, setError] = useState<string | null>(null);
  const [queryRecovery, setQueryRecovery] = useState<'consent' | 'metadata' | 'settings' | null>(null);
  const [queryRetryDeadlines, setQueryRetryDeadlines] = useState<Record<string, number>>({});
  const [queryClock, setQueryClock] = useState(() => Date.now());
  const [resolvedPostState, setResolvedPostState] = useState<ResolvedPostState | null>(null);
  const [resolvedAuthorState, setResolvedAuthorState] = useState<ResolvedAuthorState | null>(null);
  // #975: 空状態の説明用に読む索引状況。結果 object ごとに取り直し、永続化しない。
  const [emptyIndexStatus, setEmptyIndexStatus] = useState<{
    result: IndexResultState;
    state: CommunityIndexEmptyIndexStatusInput;
  } | null>(null);
  const emptyStatusSequence = useRef(0);
  const requestSequence = useRef(0);
  const detailSequence = useRef(0);
  const authorSequence = useRef(0);

  const effectiveOperation: IndexOperation = mode === 'topic' ? 'search' : operation;
  const isAllJoined = mode === 'topic' && activeTimelineScope.kind === 'all_joined';
  // 選択値が適格一覧(認証・同意・通信・提供中能力)に含まれない間は、再調整が追いつくまで
  // 古いノードへ要求を送らない(#698)。文脈も無効化するので古い結果・通報対象は失効する。
  const activeNodeBaseUrl =
    selectedNodeBaseUrl !== null && eligibleNodeBaseUrls.includes(selectedNodeBaseUrl)
      ? selectedNodeBaseUrl
      : null;
  const disabled = activeNodeBaseUrl === null || isAllJoined ||
    (availability !== undefined && availability.reason !== 'ready');
  const retryDeadline = activeNodeBaseUrl ? queryRetryDeadlines[activeNodeBaseUrl] ?? 0 : 0;
  const queryRetrySeconds = Math.max(0, Math.ceil((retryDeadline - queryClock) / 1000));
  useEffect(() => {
    if (retryDeadline <= Date.now()) return;
    const timer = window.setInterval(() => setQueryClock(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [retryDeadline]);
  const currentContext = useMemo(
    () => indexContext(mode, effectiveOperation, activeNodeBaseUrl, activeTopic, activeTimelineScope),
    [activeNodeBaseUrl, activeTimelineScope, activeTopic, effectiveOperation, mode]
  );
  const currentContextKey = currentContext?.key ?? null;
  const currentContextKeyRef = useRef(currentContextKey);
  const visibleResult =
    resultState && resultState.context.key === currentContextKey ? resultState : null;
  const resolvedPostsByKey = useMemo(
    () =>
      visibleResult && resolvedPostState?.contextKey === visibleResult.context.key
        ? resolvedPostState.entriesByKey
        : {},
    [resolvedPostState, visibleResult]
  );
  const resolvedPostStatusByKey = useMemo(
    () =>
      visibleResult && resolvedPostState?.contextKey === visibleResult.context.key
        ? resolvedPostState.statusByKey
        : {},
    [resolvedPostState, visibleResult]
  );
  // #1055: 発行元の表示名とゲート中の添付 hash は専用フックが持つ。
  const { issuerNodeName: advisoryIssuerNodeName, gatedMediaHashes: advisoryGatedMediaHashes } =
    useCommunityIndexAdvisories({
      api,
      entries: visibleResult?.entries ?? null,
      nodeBaseUrl: visibleResult?.context.nodeBaseUrl ?? null,
      adultContentEnabled,
    });

  const resolvedAuthorsByPubkey = useMemo(
    () =>
      visibleResult && resolvedAuthorState?.contextKey === visibleResult.context.key
        ? resolvedAuthorState.entriesByPubkey
        : {},
    [resolvedAuthorState, visibleResult]
  );
  const visiblePostCards = useMemo(
    () =>
      visibleResult?.entries.map((entry) => {
        const key = indexEntryKey(entry);
        const localAuthor =
          entry.author_pubkey === localAuthorPubkey && localProfile
            ? localAuthorView(localProfile, entry.author_pubkey)
            : null;
        const authorResolution = resolvedAuthorsByPubkey[entry.author_pubkey];
        const knownAuthor =
          localAuthor ??
          knownAuthorsByPubkey[entry.author_pubkey] ??
          authorResolution?.author ??
          null;
        const resolvedEntry = resolvedPostsByKey[key] ?? null;
        const resolutionStatus = resolvedPostStatusByKey[key] ?? 'loading';
        return {
          key,
          resolvedEntry,
          view: communityIndexPostCardView(entry, {
            nodeBaseUrl: visibleResult.context.nodeBaseUrl,
            operation: visibleResult.context.operation,
            topicId: visibleResult.context.topicId ?? null,
            knownAuthor,
            authorStatus: knownAuthor ? 'resolved' : authorResolution?.status ?? 'loading',
            resolutionStatus,
            resolvedEntry,
            mediaObjectUrls,
            adultContentEnabled,
            gatedMediaHashes,
            unsupportedVideoManifests,
            locale,
            nodeName: advisoryIssuerNodeName,
          }),
        };
      }) ?? [],
    [
      adultContentEnabled,
      advisoryIssuerNodeName,
      gatedMediaHashes,
      knownAuthorsByPubkey,
      localAuthorPubkey,
      localProfile,
      locale,
      mediaObjectUrls,
      resolvedAuthorsByPubkey,
      resolvedPostStatusByKey,
      resolvedPostsByKey,
      unsupportedVideoManifests,
      visibleResult,
    ]
  );

  // #1052: 表示中の解決済み投稿を呼出元へ公開し、タイムラインと同じプリフェッチ・
  // 成人向け取得ゲートの対象に含める。同じ添付集合を繰り返し通知しない。
  // #1055: advisory でゲート中の投稿は公開しない。self-label 由来のゲートは
  // `usePreviewableMediaAttachments` が `isAdultLabeledPost` で除外するが、advisory は
  // `PostView` に現れないため、判定を持つこの層で外す。Rust 側の hash ゲート
  // (`blob_media_payload`)は独立した fail-closed backstop として別に効く。
  const resolvedPosts = useMemo(
    () =>
      visiblePostCards.flatMap(({ resolvedEntry, view }) =>
        resolvedEntry?.post && view.gatedBy !== 'advisory' ? [resolvedEntry.post] : []
      ),
    [visiblePostCards]
  );
  const gatedResolvedMediaHashes = useMemo(() => {
    const hashes = new Set(advisoryGatedMediaHashes);
    for (const { resolvedEntry, view } of visiblePostCards) {
      const post = resolvedEntry?.post;
      if (!post || view.gatedBy !== 'advisory') continue;
      for (const hash of postGateableMediaHashes(post)) hashes.add(hash);
    }
    return [...hashes].sort();
  }, [advisoryGatedMediaHashes, visiblePostCards]);
  usePublishedAdvisoryHashes(gatedResolvedMediaHashes, onAdvisoryGatedMediaHashesChange);
  const resolvedPostsChangeRef = useRef(onResolvedPostsChange);
  useEffect(() => {
    resolvedPostsChangeRef.current = onResolvedPostsChange;
  }, [onResolvedPostsChange]);
  const publishedResolvedSignature = useRef<string | null>(null);
  useEffect(() => {
    const signature = resolvedPosts
      .map(
        (post) =>
          `${post.object_id}:${post.attachments.map((attachment) => attachment.hash).join(',')}`
      )
      .join('|');
    if (publishedResolvedSignature.current === signature) return;
    publishedResolvedSignature.current = signature;
    onResolvedPostsChange?.(resolvedPosts);
  }, [onResolvedPostsChange, resolvedPosts]);
  useEffect(
    () => () => {
      resolvedPostsChangeRef.current?.([]);
    },
    []
  );

  const invalidateResults = useCallback(() => {
    requestSequence.current += 1;
    detailSequence.current += 1;
    authorSequence.current += 1;
    setStatus('idle');
    setResultState(null);
    setResolvedPostState(null);
    setResolvedAuthorState(null);
    setError(null);
    setQueryRecovery(null);
  }, []);

  useEffect(() => {
    currentContextKeyRef.current = currentContextKey;
    invalidateResults();
  }, [currentContextKey, invalidateResults]);

  useEffect(() => {
    const isEmptyResult =
      status === 'success' && visibleResult !== null && visibleResult.entries.length === 0;
    if (!isEmptyResult || typeof api.readCommunityNodeIndexingStatus !== 'function') return;
    const result = visibleResult;
    const sequence = ++emptyStatusSequence.current;
    // 検索したのと同じノードへ、認証済みの read を 1 回だけ送る。公開 topic の範囲指定は対象付きで
    // 読み、横断・非公開チャンネルは自分の申請一覧だけを読む(所属証明の秘密値は送らない。INVAR-3)。
    const publicTopic =
      result.context.scopeKind === 'public_topic' && result.context.scopeId
        ? result.context.scopeId
        : null;
    setEmptyIndexStatus({ result, state: { kind: 'loading' } });
    api
      .readCommunityNodeIndexingStatus({
        base_url: result.context.nodeBaseUrl,
        scope_kind: publicTopic ? 'public_topic' : null,
        topic_id: publicTopic,
        channel_id: null,
        confirm_private_channel_secret_disclosure: false,
      })
      .then((response) => {
        if (sequence !== emptyStatusSequence.current) return;
        setEmptyIndexStatus({ result, state: { kind: 'known', response } });
      })
      .catch(() => {
        if (sequence !== emptyStatusSequence.current) return;
        setEmptyIndexStatus({ result, state: { kind: 'unknown' } });
      });
  }, [api, status, visibleResult]);

  useEffect(() => {
    if (!visibleResult) {
      setResolvedPostState(null);
      return;
    }
    const contextKey = visibleResult.context.key;
    const sequence = ++detailSequence.current;
    const entries = visibleResult.entries.flatMap((entry) => {
      const input = resolveInputForEntry(entry, visibleResult.context);
      return input ? [input] : [];
    });
    const canResolve = typeof api.resolveCommunityIndexPosts === 'function';
    setResolvedPostState({
      contextKey,
      entriesByKey: {},
      statusByKey: Object.fromEntries(
        visibleResult.entries.map((entry) => {
          const resolvable = resolveInputForEntry(entry, visibleResult.context) !== null;
          return [indexEntryKey(entry), canResolve && resolvable ? 'loading' : 'failed'];
        })
      ),
    });
    if (entries.length === 0 || !canResolve) return;

    void api
      .resolveCommunityIndexPosts(entries)
      .then((response) => {
        if (
          sequence !== detailSequence.current ||
          contextKey !== currentContextKeyRef.current
        ) {
          return;
        }
        const entriesByKey = Object.fromEntries(response.entries.map((entry) => [entry.key, entry]));
        setResolvedPostState({
          contextKey,
          entriesByKey,
          statusByKey: Object.fromEntries(
            visibleResult.entries.map((entry) => {
              const key = indexEntryKey(entry);
              return [key, entriesByKey[key]?.post ? 'resolved' : 'failed'];
            })
          ),
        });
      })
      .catch(() => {
        if (
          sequence === detailSequence.current &&
          contextKey === currentContextKeyRef.current
        ) {
          setResolvedPostState({
            contextKey,
            entriesByKey: {},
            statusByKey: Object.fromEntries(
              visibleResult.entries.map((entry) => [indexEntryKey(entry), 'failed'])
            ),
          });
        }
      });
  }, [api, visibleResult]);

  useEffect(() => {
    if (!visibleResult) {
      setResolvedAuthorState(null);
      return;
    }
    const contextKey = visibleResult.context.key;
    const sequence = ++authorSequence.current;
    const missingPubkeys = Array.from(
      new Set(
        visibleResult.entries
          .map((entry) => entry.author_pubkey)
          .filter(
            (pubkey) =>
              !(pubkey === localAuthorPubkey && localProfile) &&
              !knownAuthorsByPubkey[pubkey]
          )
      )
    );
    setResolvedAuthorState({
      contextKey,
      entriesByPubkey: Object.fromEntries(
        missingPubkeys.map((pubkey) => [pubkey, { status: 'loading', author: null }])
      ),
    });
    if (missingPubkeys.length === 0) return;

    void (async () => {
      const entriesByPubkey: ResolvedAuthorState['entriesByPubkey'] = {};
      for (let offset = 0; offset < missingPubkeys.length; offset += 4) {
        const chunk = missingPubkeys.slice(offset, offset + 4);
        const chunkResults = await Promise.all(
          chunk.map(async (pubkey) => {
            try {
              return {
                pubkey,
                status: 'resolved' as const,
                author: await api.getAuthorSocialView(pubkey),
              };
            } catch {
              return { pubkey, status: 'failed' as const, author: null };
            }
          })
        );
        for (const result of chunkResults) {
          entriesByPubkey[result.pubkey] = {
            status: result.status,
            author: result.author,
          };
        }
      }
      if (
        sequence !== authorSequence.current ||
        contextKey !== currentContextKeyRef.current
      ) {
        return;
      }
      setResolvedAuthorState({ contextKey, entriesByPubkey });
    })();
  }, [api, knownAuthorsByPubkey, localAuthorPubkey, localProfile, visibleResult]);

  const refreshResolvedEntry = useCallback(
    async (key: string) => {
      if (!visibleResult || typeof api.resolveCommunityIndexPosts !== 'function') return;
      const entry = visibleResult.entries.find((candidate) => indexEntryKey(candidate) === key);
      if (!entry) return;
      const input = resolveInputForEntry(entry, visibleResult.context);
      if (!input) return;
      const contextKey = visibleResult.context.key;
      setResolvedPostState((current) =>
        current?.contextKey === contextKey
          ? {
              ...current,
              statusByKey: { ...current.statusByKey, [key]: 'loading' },
            }
          : current
      );
      try {
        const response = await api.resolveCommunityIndexPosts([input]);
        if (contextKey !== currentContextKeyRef.current) return;
        const resolvedEntry = response.entries.find((candidate) => candidate.key === key);
        setResolvedPostState((current) =>
          current?.contextKey === contextKey
            ? {
                contextKey,
                entriesByKey: resolvedEntry
                  ? { ...current.entriesByKey, [key]: resolvedEntry }
                  : current.entriesByKey,
                statusByKey: {
                  ...current.statusByKey,
                  [key]: resolvedEntry?.post ? 'resolved' : 'failed',
                },
              }
            : current
        );
      } catch {
        // 操作側のエラー表示を維持し、直前の有効な解決結果は消さない。
        setResolvedPostState((current) =>
          current?.contextKey === contextKey
            ? {
                ...current,
                statusByKey: { ...current.statusByKey, [key]: 'failed' },
              }
            : current
        );
      }
    },
    [api, visibleResult]
  );

  async function runQuery(event?: FormEvent) {
    event?.preventDefault();
    if (disabled || !currentContext || retryDeadline > Date.now()) return;
    if (effectiveOperation === 'search' && !query.trim()) {
      setStatus('error');
      setError(t('shell:communityIndex.queryRequired'));
      return;
    }
    const request: CommunityNodeIndexQueryRequest = {
      base_url: currentContext.nodeBaseUrl,
      query: currentContext.operation === 'search' ? query.trim() : null,
      scope_kind: currentContext.scopeKind,
      scope_id: currentContext.scopeId,
      topic_id: currentContext.topicId,
      limit: 50,
    };
    const requestContext = currentContext;
    const sequence = ++requestSequence.current;
    setStatus('loading');
    setError(null);
    setQueryRecovery(null);
    try {
      const response = await operationMethod(api, requestContext.operation)(request);
      if (
        sequence !== requestSequence.current ||
        requestContext.key !== currentContextKeyRef.current
      ) {
        return;
      }
      setResultState({ context: requestContext, entries: response.entries, query: request.query ?? '' });
      setStatus('success');
    } catch (cause) {
      if (
        sequence !== requestSequence.current ||
        requestContext.key !== currentContextKeyRef.current
      ) {
        return;
      }
      setResultState(null);
      setError(communityIndexErrorMessage(cause, t));
      setQueryRecovery(cause instanceof InvokeError && cause.code === 'CONSENT_REQUIRED'
        ? 'consent' : cause instanceof InvokeError && cause.status === 403 ? 'settings' : 'metadata');
      if (cause instanceof InvokeError && (cause.status === 429 || cause.code === 'RATE_LIMITED') && cause.retryAfterSeconds) {
        const deadline = Date.now() + cause.retryAfterSeconds * 1000;
        setQueryClock(Date.now());
        setQueryRetryDeadlines((current) => ({ ...current, [requestContext.nodeBaseUrl]: deadline }));
      }
      setStatus('error');
    }
  }

  return (
    <div className='shell-column-content shell-community-index-workspace space-y-4' data-testid={`community-index-${mode}`}>
      {/* #1192: 「見つける」カラムではカラム見出しと重複するため、
          カード内の見出しと説明文を出さない。トピック内カードは従来どおり。 */}
      {mode === 'topic' ? (
        <div className='flex flex-wrap items-start justify-between gap-3'>
          <div className='space-y-1'>
            <h3 className='text-lg font-semibold'>{t('shell:communityIndex.title')}</h3>
            <p className='text-sm text-[var(--muted-foreground)]'>
              {t('shell:communityIndex.topicSummary')}
            </p>
          </div>
        </div>
      ) : null}

      {mode === 'explore' ? (
        <div
          className='shell-workspace-tabs shell-community-index-tabs'
          role='tablist'
          aria-label={t('shell:communityIndex.surfaces')}
        >
          {(['search', 'discovery', 'recommendations'] as const).map((value) => (
            <button
              key={value}
              className={`shell-tab${operation === value ? ' shell-tab-active' : ''}`}
              type='button'
              role='tab'
              aria-selected={operation === value}
              onClick={() => {
                invalidateResults();
                setOperation(value);
              }}
            >
              {t(`shell:communityIndex.operations.${value}`)}
            </button>
          ))}
        </div>
      ) : null}

      {consentPendingNodeBaseUrls.length > 0 && availability?.recovery !== 'consent' ? (
        <Notice className='shell-community-index-notice' tone='warning'>
          <div className='flex flex-wrap items-center justify-between gap-3'>
            <span>{t('shell:communityIndex.consentRequiredNotice')}</span>
            <div className='flex flex-wrap gap-2'>
              {consentPendingNodeBaseUrls.map((baseUrl) => (
                <Button
                  key={baseUrl}
                  variant='secondary'
                  type='button'
                  onClick={() => consentFlow.open(baseUrl)}
                >
                  {t('shell:communityIndex.reviewPolicies', { baseUrl })}
                </Button>
              ))}
            </div>
          </div>
        </Notice>
      ) : null}

      {availability && availability.reason !== 'ready' ? (
        <CommunityIndexAvailabilityNotice
          availability={availability}
          onRetry={onRetryNode}
          onReviewPolicies={consentFlow.open}
          onOpenSettings={onOpenCommunityNodeSettings}
          onAutomatic={onAutomaticNode}
        />
      ) : eligibleNodeBaseUrls.length === 0 ? (
        consentPendingNodeBaseUrls.length > 0 ? null : (
        <Notice className='shell-community-index-notice' tone='warning'>
          <div className='flex flex-wrap items-center justify-between gap-3'>
            <span>{t('shell:communityIndex.noEligibleNode')}</span>
            <Button variant='secondary' type='button' onClick={onOpenCommunityNodeSettings}>
              {t('shell:workspace.communityNodeUnavailableAction')}
            </Button>
          </div>
        </Notice>
        )
      ) : activeNodeBaseUrl === null ? (
        <Notice className='shell-community-index-notice' tone='warning'>
          <div className='flex flex-wrap items-center justify-between gap-3'>
            <span>{t('shell:communityIndex.selectedNodeUnavailable')}</span>
            <Button variant='secondary' type='button' onClick={onOpenCommunityNodeSettings}>
              {t('shell:workspace.communityNodeUnavailableAction')}
            </Button>
          </div>
        </Notice>
      ) : isAllJoined ? (
        <Notice className='shell-community-index-notice' tone='warning'>{t('shell:communityIndex.allJoinedDisabled')}</Notice>
      ) : (
        <form className='shell-community-index-form' onSubmit={(event) => void runQuery(event)}>
          {effectiveOperation === 'search' ? (
            <Input
              aria-label={t('shell:communityIndex.queryLabel')}
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              placeholder={t('shell:communityIndex.queryPlaceholder')}
            />
          ) : (
            <p className='flex-1 self-center text-sm text-[var(--muted-foreground)]'>
              {t(`shell:communityIndex.operationHints.${effectiveOperation}`)}
            </p>
          )}
          <Button type='submit' disabled={status === 'loading' || queryRetrySeconds > 0}>
            <Search className='size-4' aria-hidden='true' />
            {status === 'loading'
              ? t('shell:communityIndex.loading')
              : t('shell:communityIndex.run')}
          </Button>
        </form>
      )}

      {queryRetrySeconds > 0 ? <Notice className='shell-community-index-notice'>{t('shell:communityIndex.availability.retryAfter', { seconds: queryRetrySeconds })}</Notice> : null}
      {error ? <Notice className='shell-community-index-notice' tone='destructive'>
        <p>{error}</p>
        {queryRecovery ? <div className='mt-2 flex flex-wrap gap-2'>
          {queryRecovery === 'consent' && activeNodeBaseUrl ? (
            <Button variant='secondary' onClick={() => consentFlow.open(activeNodeBaseUrl)}>
              {t('shell:communityIndex.reviewPolicies', { baseUrl: activeNodeBaseUrl })}
            </Button>
          ) : queryRecovery === 'metadata' ? (
            <Button variant='secondary' disabled={queryRetrySeconds > 0} onClick={() => {
              void onRetryNode('metadata').catch(() => setError(t('shell:communityIndex.availability.retryFailed')));
            }}>{t('shell:communityIndex.availability.retry')}</Button>
          ) : null}
          <Button variant='secondary' onClick={onOpenCommunityNodeSettings}>
            {t('shell:workspace.communityNodeUnavailableAction')}
          </Button>
        </div> : null}
      </Notice> : null}
      {status === 'success' && visibleResult && visibleResult.entries.length === 0 ? (
        <CommunityIndexEmptyState
          guidance={communityIndexEmptyGuidance({
            mode,
            operation: visibleResult.context.operation,
            query: visibleResult.query,
            activeTopic,
            activeTimelineScope,
            activeChannelLabel,
            canRequestIndexing: typeof onRequestIndexing === 'function',
            indexStatus: emptyIndexStatus?.result === visibleResult ? emptyIndexStatus.state : null,
          })}
          nodeBaseUrl={visibleResult.context.nodeBaseUrl}
          retryDisabled={queryRetrySeconds > 0}
          onRetry={() => void runQuery()}
          onOpenAuthor={onOpenAuthor}
          onOpenTimeline={onOpenTimeline}
          onRequestIndexing={onRequestIndexing}
          onOpenCommunityNodeSettings={onOpenCommunityNodeSettings}
          onOpenConnectivitySettings={onOpenConnectivitySettings}
        />
      ) : null}
      {visiblePostCards.length > 0 ? (
        <ul className='post-list' aria-label={t('shell:communityIndex.results')}>
          {visiblePostCards.map(({ key, view, resolvedEntry }) => {
            const capabilities = resolvedEntry?.capabilities;
            return (
            <li key={key}>
              <PostCard
                enableLinkPreview
                view={view}
                readOnly={!view.actionPost}
                mediaObjectUrls={mediaObjectUrls}
                onOpenAuthor={onOpenAuthor}
                onOpenThread={onOpenThread ?? (() => undefined)}
                onOpenThreadInTopic={onOpenThreadInTopic}
                onReply={onReply ?? (() => undefined)}
                onRepost={capabilities?.repost ? onRepost : undefined}
                onQuoteRepost={capabilities?.quote_repost ? onQuoteRepost : undefined}
                localAuthorPubkey={localAuthorPubkey}
                ownedReactionAssets={ownedReactionAssets}
                bookmarkedReactionAssets={bookmarkedReactionAssets}
                recentReactions={recentReactions}
                onToggleReaction={
                  capabilities?.react && onToggleReaction
                    ? async (post, reactionKey) => {
                        await onToggleReaction(post, reactionKey);
                        await refreshResolvedEntry(key);
                      }
                    : undefined
                }
                onBookmarkCustomReaction={onBookmarkCustomReaction}
                onReactionPickerOpen={onReactionPickerOpen}
                showBookmarkAction={Boolean(capabilities?.bookmark) && showBookmarkAction}
                isBookmarked={bookmarkedPostIds?.has(view.post.object_id) ?? false}
                onToggleBookmark={
                  capabilities?.bookmark && onToggleBookmark
                    ? async (post) => {
                        await onToggleBookmark(post);
                        await refreshResolvedEntry(key);
                      }
                    : undefined
                }
                onWithdraw={
                  capabilities?.withdraw && onWithdraw
                    ? async (post) => {
                        await onWithdraw(post);
                        await refreshResolvedEntry(key);
                      }
                    : undefined
                }
                onActivateReference={onActivateReference}
                onCopyLink={capabilities?.copy_link ? onCopyPostLink : undefined}
                onSubmitReport={(request) => api.submitCommunityNodeReport(request)}
                onCopyReportContact={(value) => void copyTextToClipboard(value)}
                onFetchReportManifest={(baseUrl) => api.fetchCommunityNodeManifest(baseUrl)}
                onFetchNodePolicies={(baseUrl, language) => api.fetchCommunityNodePolicies(baseUrl, language)}
              />
            </li>
            );
          })}
        </ul>
      ) : null}

      {consentFlow.dialog ? <CommunityNodeConsentDialog {...consentFlow.dialog} /> : null}
    </div>
  );
}

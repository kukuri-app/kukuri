import {
  startTransition,
  useCallback,
  useEffect,
  useRef,
  useState,
  type MutableRefObject,
} from 'react';

import type {
  CommunityNodeNodeStatus,
  DesktopApi,
  BlobMediaPayload,
  GameRoomView,
} from '@/lib/api';
import { convertFileSrc } from '@tauri-apps/api/core';
import type { ShellChromeProjection } from '@/components/shell/types';

import {
  createObjectUrlFromPayload,
  logMediaDebug,
} from '@/shell/media';
import { MediaFetchLedger, type MediaResource } from '@/shell/data/mediaFetchLedger';
import type { PreviewableMediaAttachment } from '@/shell/data/usePreviewableMediaAttachments';
import {
  PUBLIC_CHANNEL_REF,
  PUBLIC_TIMELINE_SCOPE,
  CONNECTIVITY_STATUS_FALLBACK_INTERVAL_MS,
  REFRESH_INTERVAL_MS,
  STATUS_REFRESH_INTERVAL_MS,
  useDesktopShellFieldSetter,
  useDesktopShellStore,
  type DesktopShellState,
  type DesktopShellStateValue,
  type DesktopShellStoreApi,
} from '@/shell/store';
import { activeWorkspaceScope } from '@/shell/slices/workspace';
import { setRecordEntry } from '@/shell/stateUpdates';
import {
  createGameEditorDraft,
  profileInputFromProfile,
  upsertCommunityNodeStatus,
} from '@/shell/presentation';
import { applySyncStatusDelta } from '@/shell/slices/connectivity';
import { useRuntimeEventBridge, type SyncStatusDelta } from '@/shell/data/useRuntimeEventBridge';
import { isTauriRuntime } from '@/lib/releaseReadiness';

function payloadByteLength(base64: string): number {
  return Math.floor(base64.length * 3 / 4) - (base64.endsWith('==') ? 2 : base64.endsWith('=') ? 1 : 0);
}

type Setter<K extends keyof DesktopShellState> = (
  value: DesktopShellStateValue<K>
) => void;

type UseDesktopShellDataEffectsArgs = {
  api: DesktopApi;
  storeApi: DesktopShellStoreApi;
  trackedTopics: string[];
  activeTopic: string;
  selectedThread: string | null;
  activeGameRooms: GameRoomView[];
  activeJoinedChannels: DesktopShellState['joinedChannelsByTopic'][string];
  selectedPrivateChannelId: string | null;
  mediaObjectUrls: DesktopShellState['mediaObjectUrls'];
  shellChromeState: ShellChromeProjection;
  selectedAuthorPubkey: string | null;
  previewableMediaAttachments: PreviewableMediaAttachment[];
  /// #858: 表示設定 OFF の間にゲート対象となる成人向け添付 hash。
  gatedAdultMediaHashes: string[];
  draftPreviewUrlRef: MutableRefObject<Map<string, string>>;
  directMessageDraftPreviewUrlRef: MutableRefObject<Map<string, string>>;
  visibleRefreshInFlightRef: MutableRefObject<boolean>;
  /// 表示中の Column id 列(DesktopShellColumnWorkspace の IntersectionObserver 由来)。
  visibleColumnIdsRef?: MutableRefObject<string[]>;
  loadTopics: (topics: string[], activeTopic: string, currentThread: string | null) => Promise<void>;
  // section/通知の取得と結果反映は data/loaders/ の各loaderが所有する。
  // この hook は「いつ読むか」(section 遷移・interval)だけを持ち、
  // 「何をどう読むか」は loader を呼ぶ。
  loadProfileSection: () => Promise<void>;
  loadAuthorSection: (pubkey: string) => Promise<void>;
  loadMessagesSection: () => Promise<void>;
  loadNotificationsSection: (options?: { markAsRead?: boolean }) => Promise<void>;
  refreshNotificationStatus: () => Promise<void>;
  refreshNotificationsFromEvent: () => Promise<void>;
  loadCommunityIndexCapability: () => Promise<void>;
  refreshVisibleShellData: (
    topic: string,
    currentThread: string | null,
    mode?: 'apply' | 'buffer',
    scopeChannelId?: string | null
  ) => Promise<void>;
  refreshConnectivityStatus: () => Promise<CommunityNodeNodeStatus[] | null>;
  setCommunityNodeStatuses: Setter<'communityNodeStatuses'>;
  setSyncStatus: Setter<'syncStatus'>;
  setLocalProfile: Setter<'localProfile'>;
  setProfileDraft: Setter<'profileDraft'>;
  setGameDrafts: Setter<'gameDrafts'>;
  setSelectedChannelIdByTopic: (
    value:
      | Record<string, string | null>
      | ((current: Record<string, string | null>) => Record<string, string | null>)
  ) => void;
  setComposeChannelByTopic: Setter<'composeChannelByTopic'>;
  setTimelineScopeByTopic: Setter<'timelineScopeByTopic'>;
  setMediaObjectUrls: Setter<'mediaObjectUrls'>;
};

export function useDesktopShellDataEffects({
  api,
  storeApi,
  trackedTopics,
  activeTopic,
  selectedThread,
  activeGameRooms,
  activeJoinedChannels,
  selectedPrivateChannelId,
  mediaObjectUrls,
  shellChromeState,
  selectedAuthorPubkey,
  previewableMediaAttachments,
  gatedAdultMediaHashes,
  draftPreviewUrlRef,
  directMessageDraftPreviewUrlRef,
  visibleRefreshInFlightRef,
  visibleColumnIdsRef,
  loadTopics,
  loadProfileSection,
  loadAuthorSection,
  loadMessagesSection,
  loadNotificationsSection,
  refreshNotificationStatus,
  refreshNotificationsFromEvent,
  loadCommunityIndexCapability,
  refreshVisibleShellData,
  refreshConnectivityStatus,
  setCommunityNodeStatuses,
  setSyncStatus,
  setLocalProfile,
  setProfileDraft,
  setGameDrafts,
  setSelectedChannelIdByTopic,
  setComposeChannelByTopic,
  setTimelineScopeByTopic,
  setMediaObjectUrls,
}: UseDesktopShellDataEffectsArgs) {
  // #1207: 取得の要否は attachment の参照ではなく、hash 単位の試行台帳で決める。state の参照が
  // 変わるだけの再計算(3 秒 refresh・通知・advisory)では、失敗した hash を取り直さない。
  const mediaFetchLedgerRef = useRef(new MediaFetchLedger());
  const mediaFetchApiRef = useRef(api);
  const mediaFetchAccountRef = useRef<string | null>(null);
  const mediaRetryTimerRef = useRef<number | null>(null);
  const [mediaRetryTick, setMediaRetryTick] = useState(0);
  // #1107: ゲートで取得を無効にした回数を hash ごとに持ち、ゲート前に始まった取得の結果は
  // 表示に使わない。
  const mediaGateEpochRef = useRef(new Map<string, number>());
  const gatedMediaHashesRef = useRef<ReadonlySet<string>>(new Set());
  const mediaFetchMountedRef = useRef(true);
  useEffect(() => {
    mediaFetchMountedRef.current = true;
    return () => {
      mediaFetchMountedRef.current = false;
    };
  }, []);
  const setAdultContentEnabled = useDesktopShellFieldSetter('adultContentEnabled');
  const setMediaRetryingHashes = useDesktopShellFieldSetter('mediaRetryingHashes');
  const setDirectMessageStatusByPeer = useDesktopShellFieldSetter('directMessageStatusByPeer');

  // #858: 成人向け表現の表示設定(canonical は Rust 側ローカル JSON)を起動時に mirror する。
  useEffect(() => {
    let disposed = false;
    void api
      .getContentDisplaySettings()
      .then((settings) => {
        if (!disposed) {
          setAdultContentEnabled(settings.adult_content_enabled);
        }
      })
      .catch(() => {
        // 読めない場合は既定 OFF のまま(fail-closed)。
      });
    return () => {
      disposed = true;
    };
  }, [api, setAdultContentEnabled]);

  const revokeMediaHashes = useCallback((values: Iterable<string>) => {
    const hashes = [...values];
    if (hashes.length === 0) return;
    for (const hash of hashes) {
      mediaGateEpochRef.current.set(hash, (mediaGateEpochRef.current.get(hash) ?? 0) + 1);
      mediaFetchLedgerRef.current.forget(hash);
    }
    setMediaObjectUrls((current) => {
      let changed = false;
      const next = { ...current };
      for (const hash of hashes) {
        if (hash in next) {
          delete next[hash];
          changed = true;
        }
      }
      return changed ? next : current;
    });
  }, [setMediaObjectUrls]);
  // #858 / #1107: gate 後の取得結果も使わず、表示済み URL を解放する。
  useEffect(() => {
    gatedMediaHashesRef.current = new Set(gatedAdultMediaHashes);
    revokeMediaHashes(gatedAdultMediaHashes);
  }, [gatedAdultMediaHashes, revokeMediaHashes]);
  // 表示の対象から外れた取得は止め、取得済みの URL は直近の上限つきの分だけ残して、戻したときに取り直さない(#1419)。
  // 残した URL は、同じ attachment が再び表示の対象になったときだけ使う(gate は上の effect が解放する)。
  useEffect(() => {
    const visible = new Set(previewableMediaAttachments.map((attachment) => attachment.hash));
    revokeMediaHashes(mediaFetchLedgerRef.current.hiddenReleases(visible));
  }, [previewableMediaAttachments, revokeMediaHashes]);
  const handleAdultLabelEvicted = useCallback(
    (hash: string | null) => revokeMediaHashes(hash ? [hash] : mediaFetchLedgerRef.current.demandHashes()),
    [revokeMediaHashes]
  );
  // 非 active な Timeline Column が Bookmarks を表示しているか(bookmarks ロード gate 用、Issue #765)。
  const hasBookmarksTimelineColumn = useDesktopShellStore((state) =>
    state.workspaceState.columns.some((column) => column.timelineView === 'bookmarks')
  );
  const hasBackgroundNotificationsColumn = useDesktopShellStore((state) =>
    state.workspaceState.columns.some(
      (column) =>
        column.kind === 'notifications' && column.id !== state.workspaceState.activeColumnId
    )
  );
  const notificationAccount = useDesktopShellStore((state) => state.syncStatus.local_author_pubkey);
  useEffect(() => {
    let disposed = false;

    const refresh = async () => {
      if (disposed || visibleRefreshInFlightRef.current) {
        return;
      }
      // window が非表示の間は、Flow モードの Timeline Column だけを取得する(#1647)。
      const hidden = typeof document !== 'undefined' && document.visibilityState === 'hidden';
      visibleRefreshInFlightRef.current = true;
      try {
        if (!hidden) await refreshVisibleShellData(activeTopic, selectedThread, 'buffer');
        // Issue #765: 表示中の背景 Timeline Column の scope も定期 refresh する。
        // active scope(選択 channel と public)は上で取得済みなので除外し、非表示 Column は
        // Flow モードのものだけ取得する(API 呼び出し数の上限 = 表示中と Flow モードの Column 数)。
        const currentState = storeApi.getState();
        const visibleIds = new Set(hidden ? [] : visibleColumnIdsRef?.current ?? []);
        const activeScope = activeWorkspaceScope(currentState.workspaceState);
        const activeSelectedChannelId =
          activeScope.topicId === activeTopic ? activeScope.channelId : null;
        const seenScopeKeys = new Set<string>(hidden ? [] : [
          `${activeTopic}\u0000${activeSelectedChannelId ?? ''}`,
          `${activeTopic}\u0000`,
        ]);
        const backgroundScopes = currentState.workspaceState.columns.flatMap((column) => {
          if (column.kind !== 'timeline' || !column.scope) return [];
          if (!column.timelineFlow && !visibleIds.has(column.id)) return [];
          const key = `${column.scope.topicId}\u0000${column.scope.channelId ?? ''}`;
          if (seenScopeKeys.has(key)) return [];
          seenScopeKeys.add(key);
          return [column.scope];
        });
        for (const scope of backgroundScopes) {
          if (disposed) break;
          await refreshVisibleShellData(scope.topicId, null, 'buffer', scope.channelId);
        }
        // 表示中の profile 列は、自分の profile の読込みが失敗している間と、相手の名前がまだ無い間
        // (読込みの失敗を含む)だけ読み直す(起動直後の失敗と、背景で後から届く profile。#1221 R6-B)。
        for (const column of currentState.workspaceState.columns) {
          if (disposed) break;
          if (column.kind !== 'profile' || !visibleIds.has(column.id)) continue;
          const state = storeApi.getState();
          if (!column.entityId) {
            if (state.profilePanelState.status === 'error') await loadProfileSection();
            continue;
          }
          const author = state.knownAuthorsByPubkey[column.entityId];
          if (!(author?.display_name || author?.name)) await loadAuthorSection(column.entityId);
        }
      } finally {
        visibleRefreshInFlightRef.current = false;
      }
    };

    void refresh();
    const intervalId = window.setInterval(() => {
      void refresh();
    }, REFRESH_INTERVAL_MS);
    const handleFocus = () => {
      void refresh();
    };
    const handleVisibility = () => {
      if (typeof document !== 'undefined' && document.visibilityState === 'visible') {
        void refresh();
      }
    };
    window.addEventListener('focus', handleFocus);
    document.addEventListener('visibilitychange', handleVisibility);

    return () => {
      disposed = true;
      visibleRefreshInFlightRef.current = false;
      window.clearInterval(intervalId);
      window.removeEventListener('focus', handleFocus);
      document.removeEventListener('visibilitychange', handleVisibility);
    };
  }, [
    activeTopic,
    loadAuthorSection,
    loadProfileSection,
    refreshVisibleShellData,
    selectedThread,
    storeApi,
    visibleColumnIdsRef,
    visibleRefreshInFlightRef,
  ]);

  const applySyncStatusChange = useCallback(
    (delta: SyncStatusDelta) => {
      startTransition(() => {
        if (delta.sync_status) {
          setSyncStatus(applySyncStatusDelta(
            storeApi.getState().syncStatus, delta.sync_status, delta.removed_topics
          ));
          storeApi.getState().patchState({ syncStatusRead: {
            ...storeApi.getState().syncStatusRead, loaded: true, error: false,
          } });
        }
        if (delta.community_node_statuses.length > 0 || delta.removed_community_nodes.length > 0) {
          setCommunityNodeStatuses((current) => delta.community_node_statuses.reduce(
            upsertCommunityNodeStatus,
            current.filter((status) => !delta.removed_community_nodes.includes(status.base_url))
          ));
          storeApi.getState().patchState({
            communityNodeStatusesLoaded: true, communityNodeStatusError: null,
          });
        }
      });
    },
    [setCommunityNodeStatuses, setSyncStatus, storeApi]
  );

  // #1521 AC-1b: 自分を指す相手の follow の edge が届いたら、その相手の開いている profile の列と会話の列の送信の可否を
  // 読み直す(知らせが溢れたときの `null` は、開いている列を 1 回ずつ)。
  const refreshAuthorRelationship = useCallback(
    (pubkey: string | null) => {
      // 自分の pubkey は、本人の別の端末で変えた自分の profile（#1220 AC-3b）。
      if (pubkey === null || pubkey === storeApi.getState().syncStatus.local_author_pubkey) {
        void loadProfileSection().catch(() => undefined);
      }
      for (const column of storeApi.getState().workspaceState.columns) {
        const peer = column.entityId;
        if (!peer || (pubkey !== null && peer !== pubkey)) continue;
        if (column.kind === 'profile') void loadAuthorSection(peer).catch(() => undefined);
        if (column.kind === 'conversation') {
          void api.getDirectMessageStatus(peer)
            .then((status) => setDirectMessageStatusByPeer(setRecordEntry(peer, status)))
            .catch(() => undefined);
        }
      }
    },
    [api, loadAuthorSection, loadProfileSection, setDirectMessageStatusByPeer, storeApi]
  );

  useRuntimeEventBridge(
    refreshNotificationsFromEvent,
    applySyncStatusChange,
    handleAdultLabelEvicted,
    refreshAuthorRelationship
  );

  useEffect(() => {
    // 設定の読込はCN状態を使わないので、状態の反映を待たずに並べて始める(#1416)。
    void refreshConnectivityStatus();
    void loadCommunityIndexCapability();
    const intervalMs = isTauriRuntime()
      ? CONNECTIVITY_STATUS_FALLBACK_INTERVAL_MS
      : REFRESH_INTERVAL_MS;
    const intervalId = window.setInterval(() => {
      // 選択の整合はstatus更新と同じstore transactionで行う(event/受諾経路も共通)。
      void refreshConnectivityStatus();
    }, intervalMs);
    return () => {
      window.clearInterval(intervalId);
    };
  }, [loadCommunityIndexCapability, refreshConnectivityStatus, storeApi]);

  useEffect(() => {
    void refreshNotificationStatus();
    const intervalId = window.setInterval(() => {
      void refreshNotificationStatus();
    }, STATUS_REFRESH_INTERVAL_MS);
    return () => {
      window.clearInterval(intervalId);
    };
  }, [refreshNotificationStatus]);

  useEffect(() => {
    let disposed = false;
    const saveRevision = storeApi.getState().profileSaveRevision;
    void (async () => {
      try {
        const profile = await api.getMyProfile();
        if (disposed || storeApi.getState().profileHasLoaded ||
          storeApi.getState().profileSaveRevision !== saveRevision) {
          return;
        }
        setLocalProfile(profile);
        if (!storeApi.getState().profileDirty) {
          setProfileDraft(profileInputFromProfile(profile));
        }
      } catch {
        // best effort background bootstrap
      }
    })();
    return () => {
      disposed = true;
    };
  }, [api, setLocalProfile, setProfileDraft, storeApi]);

  useEffect(() => {
    if (shellChromeState.activePrimarySection !== 'live') {
      return;
    }
    void loadTopics(trackedTopics, activeTopic, selectedThread).catch(() => undefined);
  }, [activeTopic, loadTopics, selectedThread, shellChromeState.activePrimarySection, trackedTopics]);

  useEffect(() => {
    if (shellChromeState.activePrimarySection !== 'game') {
      return;
    }
    void loadTopics(trackedTopics, activeTopic, selectedThread).catch(() => undefined);
  }, [activeTopic, loadTopics, selectedThread, shellChromeState.activePrimarySection, trackedTopics]);

  useEffect(() => {
    // Bookmarks データは chrome projection(active Column)だけでなく、非 active な
    // Timeline Column が Bookmarks を表示している場合もロードする(Issue #765)。
    if (
      (shellChromeState.activePrimarySection !== 'timeline' ||
        shellChromeState.timelineView !== 'bookmarks') &&
      !hasBookmarksTimelineColumn
    ) {
      return;
    }
    void loadTopics(trackedTopics, activeTopic, selectedThread).catch(() => undefined);
  }, [
    activeTopic,
    hasBookmarksTimelineColumn,
    loadTopics,
    selectedThread,
    shellChromeState.activePrimarySection,
    shellChromeState.timelineView,
    trackedTopics,
  ]);

  useEffect(() => {
    if (!shellChromeState.settingsOpen) {
      return;
    }
    void loadTopics(trackedTopics, activeTopic, selectedThread).catch(() => undefined);
  }, [
    activeTopic,
    loadTopics,
    selectedThread,
    shellChromeState.activeSettingsSection,
    shellChromeState.settingsOpen,
    trackedTopics,
  ]);

  const hasOwnProfileColumn = useDesktopShellStore((state) =>
    state.workspaceState.columns.some((column) => column.kind === 'profile' && !column.entityId)
  );

  // 以下 4 つの section effect は live/game/bookmarks/settings と同じ委譲形:
  // トリガ判定だけを持ち、取得・state 反映は loaders/ の単一実装(SSoT)を呼ぶ。
  useEffect(() => {
    if (!hasOwnProfileColumn) {
      return;
    }
    void loadProfileSection().catch(() => undefined);
  }, [hasOwnProfileColumn, loadProfileSection]);

  useEffect(() => {
    if (!selectedAuthorPubkey) {
      return;
    }
    void loadAuthorSection(selectedAuthorPubkey).catch(() => undefined);
  }, [loadAuthorSection, selectedAuthorPubkey]);

  useEffect(() => {
    if (
      shellChromeState.activePrimarySection !== 'messages' &&
      !storeApi.getState().directMessagePaneOpen
    ) {
      return;
    }
    let disposed = false;
    const refresh = async () => {
      if (
        disposed ||
        (typeof document !== 'undefined' && document.visibilityState === 'hidden')
      ) {
        return;
      }
      await loadMessagesSection().catch(() => undefined);
    };

    void refresh();
    const intervalId = window.setInterval(() => {
      void refresh();
    }, REFRESH_INTERVAL_MS);
    const handleFocus = () => {
      void refresh();
    };
    const handleVisibility = () => {
      if (typeof document !== 'undefined' && document.visibilityState === 'visible') {
        void refresh();
      }
    };
    window.addEventListener('focus', handleFocus);
    document.addEventListener('visibilitychange', handleVisibility);
    return () => {
      disposed = true;
      window.clearInterval(intervalId);
      window.removeEventListener('focus', handleFocus);
      document.removeEventListener('visibilitychange', handleVisibility);
    };
  }, [loadMessagesSection, shellChromeState.activePrimarySection, storeApi]);

  useEffect(() => {
    const active = shellChromeState.activePrimarySection === 'notifications';
    if (!notificationAccount || (!active && !hasBackgroundNotificationsColumn)) {
      return;
    }
    void loadNotificationsSection({ markAsRead: active }).catch(() => undefined);
  }, [
    hasBackgroundNotificationsColumn,
    loadNotificationsSection,
    notificationAccount,
    shellChromeState.activePrimarySection,
  ]);

  useEffect(() => {
    const media = mediaFetchLedgerRef.current;
    const draftPreviewUrls = draftPreviewUrlRef.current;
    const directMessageDraftPreviewUrls = directMessageDraftPreviewUrlRef.current;

    return () => {
      media.clear();
      for (const url of draftPreviewUrls.values()) {
        URL.revokeObjectURL(url);
      }
      draftPreviewUrls.clear();
      for (const url of directMessageDraftPreviewUrls.values()) {
        URL.revokeObjectURL(url);
      }
      directMessageDraftPreviewUrls.clear();
    };
  }, [directMessageDraftPreviewUrlRef, draftPreviewUrlRef]);

  useEffect(() => {
    setGameDrafts((current) => {
      let changed = false;
      const next = { ...current };
      for (const room of activeGameRooms) {
        if (!next[room.room_id]) {
          next[room.room_id] = createGameEditorDraft(room);
          changed = true;
        }
      }
      return changed ? next : current;
    });
  }, [activeGameRooms, setGameDrafts]);

  useEffect(() => {
    if (!selectedPrivateChannelId) {
      return;
    }
    const selectedStillJoined = activeJoinedChannels.some(
      (channel) => channel.channel_id === selectedPrivateChannelId
    );
    if (selectedStillJoined) {
      return;
    }
    setSelectedChannelIdByTopic(setRecordEntry(activeTopic, null));
    setComposeChannelByTopic((current) =>
      current[activeTopic]?.kind === 'private_channel' &&
      current[activeTopic].channel_id === selectedPrivateChannelId
        ? {
            ...current,
            [activeTopic]: PUBLIC_CHANNEL_REF,
          }
        : current
    );
    setTimelineScopeByTopic((current) =>
      current[activeTopic]?.kind === 'channel' &&
      current[activeTopic].channel_id === selectedPrivateChannelId
        ? {
            ...current,
            [activeTopic]: PUBLIC_TIMELINE_SCOPE,
          }
        : current
    );
  }, [
    activeJoinedChannels,
    activeTopic,
    selectedPrivateChannelId,
    setComposeChannelByTopic,
    setSelectedChannelIdByTopic,
    setTimelineScopeByTopic,
  ]);

  useEffect(() => {
    const ledger = mediaFetchLedgerRef.current;
    if (mediaFetchApiRef.current !== api || mediaFetchAccountRef.current !== notificationAccount) {
      // api の差し替えは別の backend への接続を意味する。前の backend での失敗を引き継がない。
      mediaFetchApiRef.current = api;
      mediaFetchAccountRef.current = notificationAccount;
      ledger.clear();
      mediaGateEpochRef.current.clear();
      setMediaObjectUrls({});
    }
    const currentHashes = new Set(previewableMediaAttachments.map((attachment) => attachment.hash));
    for (const hash of mediaGateEpochRef.current.keys()) {
      if (
        !currentHashes.has(hash) &&
        !ledger.isInFlight(hash) &&
        !gatedMediaHashesRef.current.has(hash)
      ) {
        mediaGateEpochRef.current.delete(hash);
      }
    }

    const now = Date.now();
    let earliestRetryAt: number | null = null;
    for (const attachment of previewableMediaAttachments) {
      if (typeof mediaObjectUrls[attachment.hash] === 'string') {
        continue;
      }
      const nativeFile = isTauriRuntime() && Boolean(api.getBlobMediaFile && api.releaseBlobMediaFile);
      // file 表示は bytes を JS のメモリへ持たない。remote 取得の同時数は backend の受付が上限を持つので、
      // ここで件数を絞らない(手元にある画像の要求を、remote の取得待ちの後ろに並べない。#1419)。
      const reservedBytes = nativeFile ? 0 : Math.max(1, attachment.bytes) * 4;
      const decision = ledger.decide(attachment.hash, attachment.status ?? null, now, reservedBytes);
      if (decision.kind === 'wait') {
        earliestRetryAt =
          earliestRetryAt === null ? decision.retryAt : Math.min(earliestRetryAt, decision.retryAt);
        continue;
      }
      if (decision.kind === 'skip') {
        continue;
      }
      // 失敗が確定した hash を取り直す(明示再試行、status が `Available` へ変わった、api が替わった)。
      // 失敗表示は残したまま再試行中にし、操作の重複と focus の喪失を防ぐ。
      const retryingFailed = mediaObjectUrls[attachment.hash] === null;
      if (retryingFailed) {
        setMediaRetryingHashes((current) =>
          current[attachment.hash] ? current : { ...current, [attachment.hash]: true }
        );
      }
      const clearRetrying = () => {
        if (!retryingFailed) {
          return;
        }
        setMediaRetryingHashes((current) => {
          if (!current[attachment.hash]) {
            return current;
          }
          const next = { ...current };
          delete next[attachment.hash];
          return next;
        });
      };
      const gateEpoch = mediaGateEpochRef.current.get(attachment.hash) ?? 0;
      // 完了した結果を使ってよいか。取得後に一度でもゲートされた hash の bytes は使わない。
      const resultUsable = () =>
        mediaFetchMountedRef.current &&
        mediaFetchApiRef.current === api &&
        mediaFetchAccountRef.current === notificationAccount &&
        (mediaGateEpochRef.current.get(attachment.hash) ?? 0) === gateEpoch &&
        !gatedMediaHashesRef.current.has(attachment.hash);
      // 失敗を台帳へ記録し、上限に達したときだけ取得不可を表示へ出す。
      const recordFailure = () => {
        const outcome = ledger.fail(attachment.hash, Date.now());
        if (outcome.kind === 'retry') {
          setMediaRetryTick((tick) => tick + 1);
          return;
        }
        setMediaObjectUrls((current) =>
          typeof current[attachment.hash] === 'string' || current[attachment.hash] === null
            ? current
            : { ...current, [attachment.hash]: null }
        );
      };

      const nextAttempt = decision.attempt;
      logMediaDebug('info', 'remote media fetch start', {
        attempt: nextAttempt,
        hash: attachment.hash,
        mime: attachment.mime,
        role: attachment.role,
        status: attachment.status,
      });

      const requestId = nativeFile
        ? (globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`)
        : null;
      if (requestId) {
        ledger.trackCancel(attachment.hash, () => {
          void api.releaseBlobMediaFile?.(requestId).catch(() => undefined);
        });
      }
      const resourceRequest: Promise<{ kind: 'file'; resource: MediaResource } | { kind: 'payload'; payload: BlobMediaPayload } | null> = requestId && api.getBlobMediaFile
        ? api.getBlobMediaFile(attachment.hash, attachment.mime, attachment.source_object_id, requestId)
            .then((file) => file ? {
              kind: 'file' as const,
              resource: {
                url: convertFileSrc(file.path),
                memoryBytes: 0,
                release: () => { void api.releaseBlobMediaFile?.(file.request_id).catch(() => undefined); },
              },
            } : null)
        : api.getBlobMediaPayload(attachment.hash, attachment.mime, attachment.source_object_id)
            .then((payload) => {
              if (!payload) return null;
              const bytes = payloadByteLength(payload.bytes_base64);
              if (bytes * 4 > reservedBytes) return null;
              return { kind: 'payload' as const, payload };
            });
      void resourceRequest
        .then((fetched) => {
          clearRetrying();
          if (!resultUsable()) {
            if (fetched?.kind === 'file') fetched.resource.release();
            return;
          }
          const resource = fetched?.kind === 'file' ? fetched.resource : fetched?.kind === 'payload'
            ? (() => {
              const url = createObjectUrlFromPayload(fetched.payload);
              return {
                url,
                memoryBytes: payloadByteLength(fetched.payload.bytes_base64),
                release: () => URL.revokeObjectURL(url),
              };
            })()
            : null;
          if (!resource) {
            logMediaDebug('warn', 'remote media fetch missing', {
              attempt: nextAttempt,
              hash: attachment.hash,
              mime: attachment.mime,
              role: attachment.role,
              status: attachment.status,
            });
            recordFailure();
            return;
          }

          logMediaDebug('info', 'remote media fetch hit', {
            attempt: nextAttempt,
            hash: attachment.hash,
            mime: attachment.mime,
            object_url: resource.url,
            role: attachment.role,
            status: attachment.status,
          });

          if (typeof storeApi.getState().mediaObjectUrls[attachment.hash] === 'string') {
            resource.release();
            ledger.succeed(attachment.hash);
            return;
          }
          ledger.succeed(attachment.hash, resource);
          setMediaObjectUrls((current) => ({ ...current, [attachment.hash]: resource.url }));
        })
        .catch((fetchError: unknown) => {
          clearRetrying();
          if (!resultUsable()) {
            return;
          }
          logMediaDebug('warn', 'remote media fetch error', {
            attempt: nextAttempt,
            error: fetchError instanceof Error ? fetchError.message : 'unknown error',
            hash: attachment.hash,
            mime: attachment.mime,
            role: attachment.role,
            status: attachment.status,
          });
          recordFailure();
        });
    }

    if (mediaRetryTimerRef.current !== null) {
      window.clearTimeout(mediaRetryTimerRef.current);
      mediaRetryTimerRef.current = null;
    }
    if (earliestRetryAt !== null) {
      mediaRetryTimerRef.current = window.setTimeout(
        () => {
          mediaRetryTimerRef.current = null;
          setMediaRetryTick((tick) => tick + 1);
        },
        Math.max(0, earliestRetryAt - Date.now())
      );
    }
  }, [
    api,
    mediaObjectUrls,
    mediaRetryTick,
    notificationAccount,
    previewableMediaAttachments,
    setMediaObjectUrls,
    setMediaRetryingHashes,
    storeApi,
  ]);

  useEffect(
    () => () => {
      if (mediaRetryTimerRef.current !== null) {
        window.clearTimeout(mediaRetryTimerRef.current);
        mediaRetryTimerRef.current = null;
      }
    },
    []
  );

  // #1207: 利用者の明示再試行。対象 hash だけ台帳を戻し、失敗表示を取得中へ切り替える。
  const retryMediaFetch = useCallback(
    (hashes: readonly string[]) => {
      const accepted = hashes.filter(
        (hash) =>
          !gatedMediaHashesRef.current.has(hash) &&
          mediaFetchLedgerRef.current.requestManualRetry(hash)
      );
      if (accepted.length === 0) {
        return accepted;
      }
      setMediaRetryTick((tick) => tick + 1);
      return accepted;
    },
    []
  );

  return { retryMediaFetch };
}

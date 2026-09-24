import { createContext, useCallback, useContext } from 'react';
import { useStore } from 'zustand';
import { createStore } from 'zustand/vanilla';

import { type DesktopApi } from '@/lib/api';
import { reconcileCommunityIndexNodePreference } from '@/lib/api/communityIndex';
import type { DesktopTheme } from '@/lib/theme';
import { type ChromeSliceState, createInitialChromeSlice } from '@/shell/slices/chrome';
import {
  type ColumnDraftsSliceState,
  createInitialColumnDraftsSlice,
} from '@/shell/slices/columnDrafts';
import {
  type ConnectivitySliceState,
  createInitialConnectivitySlice,
} from '@/shell/slices/connectivity';
import {
  type DirectMessagesSliceState,
  createInitialDirectMessagesSlice,
} from '@/shell/slices/directMessages';
import { type LiveGameSliceState, createInitialLiveGameSlice } from '@/shell/slices/liveGame';
import { type MediaSliceState, createInitialMediaSlice } from '@/shell/slices/media';
import {
  type NotificationsSliceState,
  createInitialNotificationsSlice,
} from '@/shell/slices/notifications';
import {
  type PrivateChannelSliceState,
  createInitialPrivateChannelSlice,
} from '@/shell/slices/privateChannel';
import {
  type ProfileSocialSliceState,
  createInitialProfileSocialSlice,
} from '@/shell/slices/profileSocial';
import {
  type ReactionsBookmarksSliceState,
  createInitialReactionsBookmarksSlice,
} from '@/shell/slices/reactionsBookmarks';
import { type TimelineSliceState, createInitialTimelineSlice } from '@/shell/slices/timeline';
import { MAX_VISIBLE_POSTS } from '@/shell/pagination';
import { timelineStorageKeyForChannel } from '@/shell/slices/shared';
import {
  type WorkspaceSliceState,
  createInitialWorkspaceSlice,
} from '@/shell/slices/workspace';
import {
  readWorkspaceLayout,
  type WorkspaceStorage,
} from '@/shell/workspacePersistence';
import { readSavedWorkspaceLayouts } from '@/shell/savedWorkspaceLayouts';
import {
  readColumnDrafts,
  type ColumnDraftStorage,
} from '@/shell/columnDraftPersistence';
import {
  readCommunityIndexNodePreference,
  type CommunityIndexNodePreferenceStorage,
} from '@/shell/communityIndexNodePreference';

export type AppProps = {
  api?: DesktopApi;
};

// 状態の定義はドメインスライス(@/shell/slices/)へ分割した(WP-H6 PR3)。
// ここでは合成のみ行う。**実行時は従来どおり単一 store** であり、1 回の set() は
// スライスをまたいでも原子的(useRouteSynchronization の一括更新が壊れない)。
export type DesktopShellState = TimelineSliceState &
  PrivateChannelSliceState &
  ConnectivitySliceState &
  MediaSliceState &
  ProfileSocialSliceState &
  ReactionsBookmarksSliceState &
  NotificationsSliceState &
  DirectMessagesSliceState &
  LiveGameSliceState &
  ChromeSliceState &
  ColumnDraftsSliceState &
  WorkspaceSliceState;

export type DesktopShellStateValue<K extends keyof DesktopShellState> =
  | DesktopShellState[K]
  | ((current: DesktopShellState[K]) => DesktopShellState[K]);

export type DesktopShellStore = DesktopShellState & {
  patchState: (patch: Partial<DesktopShellState>) => void;
  resetState: () => void;
  setField: <K extends keyof DesktopShellState>(
    key: K,
    value: DesktopShellStateValue<K>
  ) => void;
};

export type DesktopShellPageProps = AppProps & {
  theme: DesktopTheme;
  onThemeChange: (theme: DesktopTheme) => void;
};

export const REFRESH_INTERVAL_MS = 3000;
export const CONNECTIVITY_STATUS_FALLBACK_INTERVAL_MS = 60000;
export const STATUS_REFRESH_INTERVAL_MS = 60000;
export const VIDEO_POSTER_TIMEOUT_MS = 5000;
export const MEDIA_DEBUG_STORAGE_KEY = 'kukuri:media-debug';
export const SHELL_WORKSPACE_ID = 'shell-primary-workspace';
export const SHELL_SETTINGS_ID = 'shell-settings-drawer';

export function createInitialShellState(): DesktopShellState {
  const timeline = createInitialTimelineSlice();
  const privateChannels = createInitialPrivateChannelSlice();
  return {
    ...timeline,
    ...privateChannels,
    ...createInitialConnectivitySlice(),
    ...createInitialMediaSlice(),
    ...createInitialProfileSocialSlice(),
    ...createInitialReactionsBookmarksSlice(),
    ...createInitialNotificationsSlice(),
    ...createInitialDirectMessagesSlice(),
    ...createInitialLiveGameSlice(),
    ...createInitialChromeSlice(),
    ...createInitialColumnDraftsSlice(),
    ...createInitialWorkspaceSlice({
      topicId: timeline.trackedTopics[0],
      channelId: null,
    }),
  };
}

type CreateDesktopShellStoreOptions = {
  workspaceStorage?: WorkspaceStorage;
  draftStorage?: ColumnDraftStorage;
  communityIndexPreferenceStorage?: CommunityIndexNodePreferenceStorage;
};

// 検索先は接続stateの派生値。受諾・event・poll・manifest・設定変更の各入口で
// 同じ更新と同時に解決し、利用可能なNodeだけ更新されて選択がnullに残るのを防ぐ。
function withCommunityIndexSelection(
  current: DesktopShellStore,
  patch: Partial<DesktopShellState>
): Partial<DesktopShellState> {
  if (!['communityNodeConfig', 'communityNodeStatuses', 'communityNodeManifests',
    'communityIndexNodePreference'].some((key) => key in patch)) return patch;
  const next = { ...current, ...patch };
  if ('communityNodeConfig' in patch) next.communityNodeConfigLoaded = true;
  // 保存済みmanual preferenceを初回config取得前の空配列で消さない。
  if (!next.communityNodeConfigLoaded) return patch;
  const resolution = reconcileCommunityIndexNodePreference(next);
  return {
    ...patch,
    communityNodeConfigLoaded: next.communityNodeConfigLoaded,
    ...('communityNodeConfig' in patch ? { communityNodeConfigError: null } : {}),
    communityIndexNodePreference: resolution.preference,
    communityIndexNodeBaseUrl: resolution.selectedBaseUrl,
  };
}

function boundVisibleLists(
  current: DesktopShellStore,
  patch: Partial<DesktopShellState>
): Partial<DesktopShellState> {
  const fields = ['timelinesByKey', 'threadsById', 'authorTimelinesByPubkey',
    'profileTimeline', 'selectedAuthorTimeline', 'bookmarkedPosts'] as const;
  if (!('workspaceState' in patch) && !('visibleListColumnIds' in patch) &&
    !fields.some((field) => field in patch)) return patch;
  const next = { ...current, ...patch };
  const columns = next.workspaceState.columns;
  const active = columns.find((column) => column.id === next.workspaceState.activeColumnId);
  const visible = new Set(next.visibleListColumnIds.slice(0, 8));
  visible.add(next.workspaceState.activeColumnId);
  const eligible = new Set<string>();
  const preferred = new Set<string>();
  const include = (id: string, columnId: string) => {
    eligible.add(id);
    if (visible.has(columnId)) preferred.add(id);
  };
  for (const column of columns) {
    if (column.kind === 'timeline' && column.scope) {
      include(`timeline:${timelineStorageKeyForChannel(column.scope.topicId, column.scope.channelId)}`, column.id);
      include(`timeline:${timelineStorageKeyForChannel(column.scope.topicId, null)}`, column.id);
      if (column.timelineView === 'bookmarks') include('bookmarks', column.id);
    }
    if (column.kind === 'thread' && column.entityId) include(`thread:${column.entityId}`, column.id);
    if (column.kind === 'profile') {
      include(column.entityId ? `author:${column.entityId}` : 'profile', column.id);
    }
  }
  if (next.selectedThread) eligible.add(`thread:${next.selectedThread}`);
  if (next.selectedAuthorPubkey) eligible.add(`author:${next.selectedAuthorPubkey}`);
  if (next.shellChromeState.settingsOpen && next.shellChromeState.activeSettingsSection === 'reactions') {
    eligible.add('bookmarks');
  }
  const activeId = active?.kind === 'timeline'
    ? active.timelineView === 'bookmarks' ? 'bookmarks'
      : active.scope ? `timeline:${timelineStorageKeyForChannel(active.scope.topicId, active.scope.channelId)}` : null
    : active?.kind === 'thread' ? `thread:${active.entityId}`
      : active?.kind === 'profile' ? active.entityId ? `author:${active.entityId}` : 'profile'
        : null;
  type Window = { id: string; rows: unknown[]; priority: number };
  const windows: Window[] = [];
  const add = (id: string, rows: unknown[], changed: boolean) => {
    if (rows.length && (!('workspaceState' in patch) && !('visibleListColumnIds' in patch) || eligible.has(id))) {
      windows.push({ id, rows, priority: (id === activeId ? 4 : preferred.has(id) ? 2 : 0) + (changed ? 1 : 0) });
    }
  };
  for (const [key, rows] of Object.entries(next.timelinesByKey)) {
    add(`timeline:${key}`, rows, next.timelinesByKey[key] !== current.timelinesByKey[key]);
  }
  for (const [key, rows] of Object.entries(next.threadsById)) {
    add(`thread:${key}`, rows, next.threadsById[key] !== current.threadsById[key]);
  }
  for (const [key, rows] of Object.entries(next.authorTimelinesByPubkey)) {
    add(`author:${key}`, rows, next.authorTimelinesByPubkey[key] !== current.authorTimelinesByPubkey[key]);
  }
  if (next.selectedAuthorPubkey && !next.authorTimelinesByPubkey[next.selectedAuthorPubkey]) {
    add(`author:${next.selectedAuthorPubkey}`, next.selectedAuthorTimeline,
      next.selectedAuthorTimeline !== current.selectedAuthorTimeline);
  }
  add('profile', next.profileTimeline, next.profileTimeline !== current.profileTimeline);
  add('bookmarks', next.bookmarkedPosts, next.bookmarkedPosts !== current.bookmarkedPosts);
  windows.sort((left, right) => right.priority - left.priority);
  const kept = new Set(windows.slice(0, 8).map((window) => window.id));
  const trim = <T,>(rows: T[]) => rows.length > MAX_VISIBLE_POSTS ? rows.slice(0, MAX_VISIBLE_POSTS) : rows;
  const record = <T,>(rows: Record<string, T[]>, prefix: string) => Object.fromEntries(
    Object.entries(rows).filter(([key, value]) => !value.length || kept.has(`${prefix}:${key}`))
      .map(([key, value]) => [key, trim(value)])
  ) as Record<string, T[]>;
  const authors = record(next.authorTimelinesByPubkey, 'author');
  const pendingTimelineSnapshotsByKey = Object.fromEntries(
    Object.entries(next.pendingTimelineSnapshotsByKey)
      .filter(([key]) => kept.has(`timeline:${key}`))
      .map(([key, rows]) => [key, trim(rows)])
  );
  return {
    ...patch,
    timelinesByKey: record(next.timelinesByKey, 'timeline'),
    pendingTimelineSnapshotsByKey,
    threadsById: record(next.threadsById, 'thread'),
    authorTimelinesByPubkey: authors,
    profileTimeline: kept.has('profile') ? trim(next.profileTimeline) : [],
    profileTimelineEvicted: !kept.has('profile') && next.profileTimeline.length > 0
      ? true : 'profileTimeline' in patch ? false : next.profileTimelineEvicted,
    selectedAuthorTimeline: next.selectedAuthorPubkey
      ? authors[next.selectedAuthorPubkey] ?? (kept.has(`author:${next.selectedAuthorPubkey}`)
        ? trim(next.selectedAuthorTimeline) : []) : [],
    bookmarkedPosts: kept.has('bookmarks') ? next.bookmarkedPosts.slice(0, 20) : [],
  };
}

export function createDesktopShellStore(options: CreateDesktopShellStoreOptions = {}) {
  const initialState = createInitialShellState();
  const workspaceState = options.workspaceStorage
    ? readWorkspaceLayout(options.workspaceStorage, initialState.workspaceState)
    : initialState.workspaceState;
  const savedWorkspaceLayouts = options.workspaceStorage
    ? readSavedWorkspaceLayouts(options.workspaceStorage)
    : initialState.savedWorkspaceLayouts;
  const normalizedWorkspaceState =
    workspaceState.activeLayoutId &&
    !savedWorkspaceLayouts.some((layout) => layout.id === workspaceState.activeLayoutId)
      ? { ...workspaceState, activeLayoutId: null }
      : workspaceState;
  const columnDraftsByKey = options.draftStorage
    ? readColumnDrafts(options.draftStorage)
    : initialState.columnDraftsByKey;
  const communityIndexNodePreference = options.communityIndexPreferenceStorage
    ? readCommunityIndexNodePreference(options.communityIndexPreferenceStorage)
    : initialState.communityIndexNodePreference;
  return createStore<DesktopShellStore>((set) => ({
    ...initialState,
    workspaceState: normalizedWorkspaceState,
    savedWorkspaceLayouts,
    columnDraftsByKey,
    communityIndexNodePreference,
    patchState: (patch) => set((current) => boundVisibleLists(current, withCommunityIndexSelection(current, patch))),
    resetState: () => set(createInitialShellState()),
    setField: (key, value) =>
      set((current) => {
        const nextValue =
          typeof value === 'function'
            ? (value as (currentValue: DesktopShellState[typeof key]) => DesktopShellState[typeof key])(
                current[key]
              )
            : value;
        if (Object.is(current[key], nextValue)) {
          return current;
        }
        return boundVisibleLists(current, withCommunityIndexSelection(current, {
          [key]: nextValue,
        }));
      }),
  }));
}

export type DesktopShellStoreApi = ReturnType<typeof createDesktopShellStore>;

export const DesktopShellStoreContext = createContext<DesktopShellStoreApi | null>(null);

export function useDesktopShellStoreApi() {
  const store = useContext(DesktopShellStoreContext);
  if (!store) {
    throw new Error('desktop shell store is not available');
  }

  return store;
}

export function useDesktopShellStore(): DesktopShellStore;
export function useDesktopShellStore<T>(selector: (state: DesktopShellStore) => T): T;
export function useDesktopShellStore<T>(selector?: (state: DesktopShellStore) => T) {
  const resolvedSelector =
    selector ?? ((state: DesktopShellStore) => state as unknown as T);
  return useStore(
    useDesktopShellStoreApi(),
    resolvedSelector
  );
}

export function useDesktopShellFieldSetter<K extends keyof DesktopShellState>(key: K) {
  const setField = useDesktopShellStore((state) => state.setField);

  return useCallback(
    (value: DesktopShellStateValue<K>) => {
      setField(key, value);
    },
    [key, setField]
  );
}

// 既存の import 面の互換 re-export(型・定数はスライス側が定義元)。
export {
  DEFAULT_ASYNC_PANEL_STATE,
  PUBLIC_CHANNEL_REF,
  PUBLIC_TIMELINE_SCOPE,
  activeTimelineStorageKey,
  timelineScopeStorageKey,
  timelineStorageKeyForChannel,
} from '@/shell/slices/shared';
export type { AsyncPanelState, DraftMediaItem } from '@/shell/slices/shared';
export { DEFAULT_COMMUNITY_NODE_CONFIG } from '@/shell/slices/connectivity';
export type {
  CommunityNodeDraftNode,
  CommunityNodeManifestEntry,
  CommunityNodePoliciesEntry,
} from '@/shell/slices/connectivity';
export type { KnownAuthorsByPubkey } from '@/shell/slices/profileSocial';
export type { GameEditorDraft } from '@/shell/slices/liveGame';

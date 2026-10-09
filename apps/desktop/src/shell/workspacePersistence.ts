import type { DesktopShellStoreApi } from '@/shell/store';
import {
  normalizeColumnSpan,
  type ColumnKind,
  type ColumnScope,
  type ColumnState,
  type WorkspaceState,
} from '@/shell/slices/workspace';

export const WORKSPACE_LAYOUT_STORAGE_KEY = 'kukuri:workspace-layout:v1';
const WORKSPACE_LAYOUT_VERSION = 1;

export type WorkspaceStorage = Pick<Storage, 'getItem' | 'setItem'>;

const COLUMN_KINDS = new Set<ColumnKind>([
  'timeline',
  'notifications',
  'thread',
  'profile',
  'explore',
  'messages',
  'conversation',
  'stream',
  'game',
  'metaverse',
]);

type PersistedWorkspaceLayout = {
  version: typeof WORKSPACE_LAYOUT_VERSION;
  activeColumnId: string;
  activeLayoutId?: string | null;
  columns: ColumnState[];
};

export type WorkspaceLayoutSnapshot = Pick<
  PersistedWorkspaceLayout,
  'activeColumnId' | 'columns'
>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseScope(value: unknown): ColumnScope | undefined {
  if (!isRecord(value) || typeof value.topicId !== 'string' || !value.topicId) {
    return undefined;
  }
  if (value.channelId !== null && typeof value.channelId !== 'string') {
    return undefined;
  }
  return { topicId: value.topicId, channelId: value.channelId };
}

function parseColumn(value: unknown): ColumnState | null {
  if (
    !isRecord(value) ||
    typeof value.id !== 'string' ||
    !value.id ||
    typeof value.kind !== 'string' ||
    !COLUMN_KINDS.has(value.kind as ColumnKind) ||
    typeof value.pinned !== 'boolean'
  ) {
    return null;
  }
  const kind = value.kind as ColumnKind;
  const column: ColumnState = {
    id: value.id,
    kind,
    pinned: value.pinned,
    preferredDesktopSpan: normalizeColumnSpan(
      kind,
      typeof value.preferredDesktopSpan === 'number' ? value.preferredDesktopSpan : undefined
    ),
  };
  const scope = parseScope(value.scope);
  if (scope) column.scope = scope;
  if (typeof value.entityId === 'string' && value.entityId) column.entityId = value.entityId;
  if (typeof value.parentColumnId === 'string' && value.parentColumnId) {
    column.parentColumnId = value.parentColumnId;
  }
  // schema v1 への後方互換 optional field(Issue #765)。旧 layout / 不正値は未設定(既定 feed)として読む。
  if (kind === 'timeline' && (value.timelineView === 'feed' || value.timelineView === 'bookmarks')) {
    column.timelineView = value.timelineView;
  }
  if (kind === 'timeline' && value.timelineFlow === true) column.timelineFlow = true;
  if (kind === 'timeline' && (value.timelineFilter === 'mutual' || value.timelineFilter === 'following')) {
    column.timelineFilter = value.timelineFilter;
  }
  return column;
}

export function captureWorkspaceLayoutSnapshot(
  state: Pick<WorkspaceState, 'activeColumnId' | 'columns'>
): WorkspaceLayoutSnapshot {
  return {
    activeColumnId: state.activeColumnId,
    columns: state.columns.map((column) => ({
      id: column.id,
      kind: column.kind,
      ...(column.scope ? { scope: column.scope } : {}),
      ...(column.entityId ? { entityId: column.entityId } : {}),
      ...(column.parentColumnId ? { parentColumnId: column.parentColumnId } : {}),
      ...(column.timelineView ? { timelineView: column.timelineView } : {}),
      ...(column.timelineFlow ? { timelineFlow: true } : {}),
      ...(column.timelineFilter ? { timelineFilter: column.timelineFilter } : {}),
      pinned: column.pinned,
      preferredDesktopSpan: normalizeColumnSpan(column.kind, column.preferredDesktopSpan),
    })),
  };
}

export function normalizeWorkspaceLayoutSnapshot(
  value: unknown
): WorkspaceLayoutSnapshot | null {
  if (!isRecord(value) || !Array.isArray(value.columns)) return null;
  const ids = new Set<string>();
  const columns = value.columns.flatMap((candidate) => {
    const column = parseColumn(candidate);
    if (!column || ids.has(column.id)) return [];
    ids.add(column.id);
    return [column];
  });
  if (columns.length === 0) return null;
  const normalizedColumns = columns.map((column) =>
    column.parentColumnId &&
    (!ids.has(column.parentColumnId) || column.parentColumnId === column.id)
      ? { ...column, parentColumnId: undefined }
      : column
  );
  const activeColumnId =
    typeof value.activeColumnId === 'string' && ids.has(value.activeColumnId)
      ? value.activeColumnId
      : normalizedColumns[0].id;
  return { columns: normalizedColumns, activeColumnId };
}

function persistedLayout(state: WorkspaceState): PersistedWorkspaceLayout {
  return {
    version: WORKSPACE_LAYOUT_VERSION,
    ...captureWorkspaceLayoutSnapshot(state),
    activeLayoutId: state.activeLayoutId,
  };
}

function serializedLayout(state: WorkspaceState) {
  return JSON.stringify(persistedLayout(state));
}

export function readWorkspaceLayout(
  storage: WorkspaceStorage,
  fallback: WorkspaceState
): WorkspaceState {
  try {
    const raw = storage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY);
    if (!raw) return fallback;
    const parsed: unknown = JSON.parse(raw);
    if (
      !isRecord(parsed) ||
      parsed.version !== WORKSPACE_LAYOUT_VERSION ||
      !Array.isArray(parsed.columns)
    ) {
      return fallback;
    }
    const normalized = normalizeWorkspaceLayoutSnapshot(parsed);
    if (!normalized) return fallback;
    return {
      ...normalized,
      controlCenterOpen: false,
      activeLayoutId:
        typeof parsed.activeLayoutId === 'string' && parsed.activeLayoutId
          ? parsed.activeLayoutId
          : null,
    };
  } catch {
    return fallback;
  }
}

export function writeWorkspaceLayout(storage: WorkspaceStorage, state: WorkspaceState) {
  try {
    storage.setItem(WORKSPACE_LAYOUT_STORAGE_KEY, serializedLayout(state));
    return true;
  } catch {
    return false;
  }
}

export function startWorkspaceLayoutPersistence(
  store: DesktopShellStoreApi,
  storage: WorkspaceStorage
) {
  let previous = serializedLayout(store.getState().workspaceState);
  try {
    if (storage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY) !== previous) {
      writeWorkspaceLayout(storage, store.getState().workspaceState);
    }
  } catch {
    // Storage access can be denied independently of the in-memory workspace.
  }
  return store.subscribe((state) => {
    const next = serializedLayout(state.workspaceState);
    if (next === previous) return;
    previous = next;
    writeWorkspaceLayout(storage, state.workspaceState);
  });
}

import { STARTER_TOPICS } from '@/shell/slices/shared';
import type { PrimarySection } from '@/components/shell/types';

export type ColumnSpan = 1 | 2 | 3 | 4;

export type ColumnKind =
  | 'timeline'
  | 'notifications'
  | 'thread'
  | 'profile'
  | 'explore'
  | 'messages'
  | 'conversation'
  | 'stream'
  | 'game'
  | 'metaverse';

export type ColumnScope = {
  topicId: string;
  channelId: string | null;
};

// Timeline Column の表示 view。正本は Column 単位で持つ。
export type ColumnTimelineView = 'feed' | 'bookmarks';

export type ColumnState = {
  id: string;
  kind: ColumnKind;
  scope?: ColumnScope;
  entityId?: string;
  parentColumnId?: string;
  pinned: boolean;
  preferredDesktopSpan: ColumnSpan;
  // kind === 'timeline' の Column のみ意味を持つ。未設定は 'feed'。
  timelineView?: ColumnTimelineView;
};

export type ColumnStateInput = Omit<ColumnState, 'preferredDesktopSpan'> & {
  preferredDesktopSpan?: number;
};

export type WorkspaceState = {
  columns: ColumnState[];
  activeColumnId: string;
  controlCenterOpen: boolean;
  activeLayoutId: string | null;
};

export type WorkspaceSliceState = {
  workspaceState: WorkspaceState;
  visibleListColumnIds: string[];
  savedWorkspaceLayouts: import('@/shell/savedWorkspaceLayouts').SavedWorkspaceLayout[];
};

type ColumnSpanPolicy = {
  default: ColumnSpan;
  min: ColumnSpan;
  max: ColumnSpan;
};

const FIXED_COLUMN_SPAN_POLICY: ColumnSpanPolicy = { default: 1, min: 1, max: 1 };

const COLUMN_SPAN_POLICIES: Record<ColumnKind, ColumnSpanPolicy> = {
  timeline: FIXED_COLUMN_SPAN_POLICY,
  notifications: FIXED_COLUMN_SPAN_POLICY,
  thread: FIXED_COLUMN_SPAN_POLICY,
  profile: FIXED_COLUMN_SPAN_POLICY,
  explore: FIXED_COLUMN_SPAN_POLICY,
  messages: { default: 1, min: 1, max: 2 },
  conversation: { default: 1, min: 1, max: 2 },
  stream: { default: 2, min: 1, max: 2 },
  game: FIXED_COLUMN_SPAN_POLICY,
  metaverse: { default: 3, min: 1, max: 4 },
};

export function columnSpanPolicy(kind: ColumnKind): ColumnSpanPolicy {
  return COLUMN_SPAN_POLICIES[kind];
}

// 親なし transient(deep link / section 遷移で開く Column)同士の置換対象。
// scope を持つ会話系(timeline / thread / profile / conversation)は、close 時の
// 孫付け替え等で親を失っても deep link に巻き込まれて消えないよう対象外とする(Issue #765)。
const PARENTLESS_REPLACEABLE_COLUMN_KINDS: ReadonlySet<ColumnKind> = new Set([
  'stream',
  'game',
  'metaverse',
  'notifications',
  'messages',
  'explore',
]);

export function isParentlessReplaceableColumnKind(kind: ColumnKind): boolean {
  return PARENTLESS_REPLACEABLE_COLUMN_KINDS.has(kind);
}

export function defaultColumnSpan(kind: ColumnKind): ColumnSpan {
  return columnSpanPolicy(kind).default;
}

export function normalizeColumnSpan(kind: ColumnKind, requestedSpan?: number): ColumnSpan {
  const policy = columnSpanPolicy(kind);
  if (!Number.isFinite(requestedSpan)) return policy.default;
  return Math.min(policy.max, Math.max(policy.min, Math.round(requestedSpan!))) as ColumnSpan;
}

function resolveColumnInput(
  requestedColumn: ColumnStateInput,
  fallbackSpan?: ColumnSpan
): ColumnState {
  return {
    ...requestedColumn,
    preferredDesktopSpan: normalizeColumnSpan(
      requestedColumn.kind,
      requestedColumn.preferredDesktopSpan ?? fallbackSpan
    ),
  };
}

function identityPart(value: string | null | undefined) {
  return value ? encodeURIComponent(value) : '-';
}

export function columnIdentityId(
  kind: ColumnKind,
  scope?: ColumnScope,
  entityId?: string
): string {
  return [
    'column',
    kind,
    identityPart(scope?.topicId),
    identityPart(scope?.channelId),
    identityPart(entityId),
  ].join(':');
}

export const INITIAL_TIMELINE_COLUMN_ID = columnIdentityId('timeline', {
  topicId: STARTER_TOPICS[0],
  channelId: null,
});

const INITIAL_WORKSPACE_COLUMN_KINDS = [
  'timeline',
  'profile',
  'explore',
  'notifications',
  'messages',
] as const satisfies readonly ColumnKind[];

export function createInitialWorkspaceState(
  scope: ColumnScope = { topicId: STARTER_TOPICS[0], channelId: null }
): WorkspaceState {
  const initialTimelineColumnId = columnIdentityId('timeline', scope);
  return {
    columns: INITIAL_WORKSPACE_COLUMN_KINDS.map((kind) => ({
      id: columnIdentityId(kind, scope),
      kind,
      scope,
      pinned: true,
      preferredDesktopSpan: defaultColumnSpan(kind),
    })),
    activeColumnId: initialTimelineColumnId,
    controlCenterOpen: false,
    activeLayoutId: null,
  };
}

export function createInitialWorkspaceSlice(scope?: ColumnScope): WorkspaceSliceState {
  return { workspaceState: createInitialWorkspaceState(scope), visibleListColumnIds: [], savedWorkspaceLayouts: [] };
}

export function activeWorkspaceColumn(state: WorkspaceState): ColumnState {
  return (
    state.columns.find((column) => column.id === state.activeColumnId) ??
    state.columns[0] ??
    createInitialWorkspaceState().columns[0]
  );
}

export function activeWorkspaceScope(state: WorkspaceState): ColumnScope {
  return activeWorkspaceColumn(state).scope ?? {
    topicId: STARTER_TOPICS[0],
    channelId: null,
  };
}

// scope に一致する Timeline Column の id。topic 切替後の Timeline は id を変えず scope だけ
// 変わるため、identity id だけで探すと既存 Column を見落として重複を開く(Issue #1053)。
// active → identity id → 先頭の順に一致を探し、無ければ identity id を返す。
export function timelineColumnIdForScope(state: WorkspaceState, scope: ColumnScope): string {
  const identityId = columnIdentityId('timeline', scope);
  const matches = (column: ColumnState) =>
    column.kind === 'timeline' &&
    column.scope?.topicId === scope.topicId &&
    column.scope.channelId === scope.channelId;
  return (
    state.columns.find((column) => column.id === state.activeColumnId && matches(column))?.id ??
    state.columns.find((column) => column.id === identityId && matches(column))?.id ??
    state.columns.find(matches)?.id ??
    identityId
  );
}

// 一覧 Column(Messages)から開いている Conversation Column の相手。
export function childConversationPeer(state: WorkspaceState, parentColumnId: string) {
  return state.columns.find(
    (column) => column.kind === 'conversation' && column.parentColumnId === parentColumnId
  )?.entityId;
}

export function primarySectionForColumn(column: ColumnState): PrimarySection {
  switch (column.kind) {
    case 'notifications':
      return 'notifications';
    case 'profile':
      return 'profile';
    case 'explore':
      return 'explore';
    case 'messages':
    case 'conversation':
      return 'messages';
    case 'stream':
      return 'live';
    case 'game':
    case 'metaverse':
      return 'game';
    case 'timeline':
    case 'thread':
    default:
      return 'timeline';
  }
}

export function activateColumn(state: WorkspaceState, columnId: string): WorkspaceState {
  if (
    state.activeColumnId === columnId ||
    !state.columns.some((column) => column.id === columnId)
  ) {
    return state;
  }
  return { ...state, activeColumnId: columnId };
}

export function openTransientColumn(
  state: WorkspaceState,
  requestedColumn: ColumnStateInput
): WorkspaceState {
  const existingIndex = state.columns.findIndex((candidate) => candidate.id === requestedColumn.id);
  const existing = existingIndex >= 0 ? state.columns[existingIndex] : undefined;
  // 自分自身を parent に指定された場合は自己参照を生成せず、既存 Column の parent(無ければ undefined)へ正規化する。
  // 自己参照が残ると親一致による置換対象から外れ、同じ parent の一時 Column が蓄積してしまう。
  const parentColumnId =
    requestedColumn.parentColumnId === requestedColumn.id
      ? existing?.parentColumnId
      : requestedColumn.parentColumnId;
  const column = {
    ...resolveColumnInput(
      { ...requestedColumn, parentColumnId },
      existing?.preferredDesktopSpan
    ),
    pinned: false,
  };
  if (existing) {
    if (existing.pinned) return activateColumn(state, column.id);

    const columns = state.columns.filter((candidate) => candidate.id !== column.id);
    const parentIndex = column.parentColumnId
      ? columns.findIndex((candidate) => candidate.id === column.parentColumnId)
      : -1;
    const insertIndex = parentIndex >= 0 ? parentIndex + 1 : columns.length;
    columns.splice(insertIndex, 0, column);
    return {
      ...state,
      columns,
      activeColumnId: column.id,
    };
  }

  // 「同じ親の未固定 Column を置換」する。ただし親なし(parentColumnId === undefined)同士は
  // immersive kind 同士に限定する: 親なし transient が unpinned な Timeline Column を巻き込んだり、
  // close 時の孫付け替えで親を失った profile 等を置換したりしないため(Issue #765)。
  const replaceIndex = state.columns.findIndex(
    (candidate) =>
      !candidate.pinned &&
      candidate.parentColumnId === column.parentColumnId &&
      (column.parentColumnId !== undefined ||
        (isParentlessReplaceableColumnKind(column.kind) &&
          isParentlessReplaceableColumnKind(candidate.kind)))
  );
  if (replaceIndex < 0) {
    const parentIndex = column.parentColumnId
      ? state.columns.findIndex((candidate) => candidate.id === column.parentColumnId)
      : -1;
    const insertIndex = parentIndex >= 0 ? parentIndex + 1 : state.columns.length;
    const columns = [...state.columns];
    columns.splice(insertIndex, 0, column);
    return {
      ...state,
      columns,
      activeColumnId: column.id,
    };
  }

  const replacedId = state.columns[replaceIndex].id;
  return {
    ...state,
    columns: state.columns.map((candidate, index) => {
      if (index === replaceIndex) return column;
      if (candidate.parentColumnId === replacedId) {
        return { ...candidate, parentColumnId: column.id };
      }
      return candidate;
    }),
    activeColumnId: column.id,
  };
}

export function openPinnedColumn(
  state: WorkspaceState,
  requestedColumn: ColumnStateInput
): WorkspaceState {
  const existing = state.columns.find((column) => column.id === requestedColumn.id);
  if (existing) {
    const pinned = setColumnPinned(state, existing.id, true);
    return activateColumn(pinned, existing.id);
  }

  const column = { ...resolveColumnInput(requestedColumn), pinned: true };
  return {
    ...state,
    columns: [...state.columns, column],
    activeColumnId: column.id,
  };
}

export function setColumnSpan(
  state: WorkspaceState,
  columnId: string,
  requestedSpan: number
): WorkspaceState {
  let changed = false;
  const columns = state.columns.map((column) => {
    if (column.id !== columnId) return column;
    const preferredDesktopSpan = normalizeColumnSpan(column.kind, requestedSpan);
    if (preferredDesktopSpan === column.preferredDesktopSpan) return column;
    changed = true;
    return { ...column, preferredDesktopSpan };
  });
  return changed ? { ...state, columns } : state;
}

export function moveColumn(
  state: WorkspaceState,
  columnId: string,
  requestedIndex: number
): WorkspaceState {
  const currentIndex = state.columns.findIndex((column) => column.id === columnId);
  if (currentIndex < 0 || !Number.isFinite(requestedIndex)) return state;
  const targetIndex = Math.min(
    state.columns.length - 1,
    Math.max(0, Math.round(requestedIndex))
  );
  if (targetIndex === currentIndex) return state;
  const columns = [...state.columns];
  const [column] = columns.splice(currentIndex, 1);
  columns.splice(targetIndex, 0, column);
  return { ...state, columns };
}

export function setColumnPinned(
  state: WorkspaceState,
  columnId: string,
  pinned: boolean
): WorkspaceState {
  let changed = false;
  const columns = state.columns.map((column) => {
    if (column.id !== columnId || column.pinned === pinned) return column;
    changed = true;
    return { ...column, pinned };
  });
  return changed ? { ...state, columns } : state;
}

export function setColumnTimelineView(
  state: WorkspaceState,
  columnId: string,
  view: ColumnTimelineView
): WorkspaceState {
  let changed = false;
  const columns = state.columns.map((column) => {
    if (column.id !== columnId || column.kind !== 'timeline') return column;
    if ((column.timelineView ?? 'feed') === view) return column;
    changed = true;
    return { ...column, timelineView: view };
  });
  return changed ? { ...state, columns } : state;
}

export function setTimelineColumnTopic(
  state: WorkspaceState,
  columnId: string,
  topicId: string
): WorkspaceState {
  if (!topicId) return state;
  let changed = false;
  const columns = state.columns.map((column) => {
    if (column.id !== columnId || column.kind !== 'timeline') return column;
    if (column.scope?.topicId === topicId && column.scope.channelId === null) return column;
    changed = true;
    return { ...column, scope: { topicId, channelId: null } };
  });
  return changed ? { ...state, columns } : state;
}

export function closeColumn(state: WorkspaceState, columnId: string): WorkspaceState {
  if (state.columns.length <= 1) return state;

  const closingIndex = state.columns.findIndex((column) => column.id === columnId);
  if (closingIndex < 0) return state;

  const closingColumn = state.columns[closingIndex];
  const columns = state.columns
    .filter((column) => column.id !== columnId)
    .map((column) =>
      column.parentColumnId === columnId
        ? { ...column, parentColumnId: closingColumn.parentColumnId }
        : column
    );
  if (state.activeColumnId !== columnId) return { ...state, columns };

  const parent = closingColumn.parentColumnId
    ? columns.find((column) => column.id === closingColumn.parentColumnId)
    : undefined;
  const neighbor = columns[Math.min(closingIndex, columns.length - 1)];
  return {
    ...state,
    columns,
    activeColumnId: parent?.id ?? neighbor.id,
  };
}

export function updateColumnScope(
  state: WorkspaceState,
  columnId: string,
  scope: ColumnScope
): WorkspaceState {
  let changed = false;
  const columns = state.columns.map((column) => {
    if (column.id !== columnId) return column;
    if (
      column.scope?.topicId === scope.topicId &&
      column.scope.channelId === scope.channelId
    ) {
      return column;
    }
    changed = true;
    return { ...column, scope };
  });
  return changed ? { ...state, columns } : state;
}

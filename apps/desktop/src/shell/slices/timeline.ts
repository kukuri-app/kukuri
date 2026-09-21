import type {
  ChannelRef,
  PostView,
  TimelineCursor,
  TimelineScope,
} from '@/lib/api';

import {
  PUBLIC_CHANNEL_REF,
  PUBLIC_TIMELINE_SCOPE,
  STARTER_TOPICS,
  buildStarterTopicRecord,
  timelineScopeStorageKey,
} from '@/shell/slices/shared';

/// タイムライン・スレッド・投稿コンポーザ(WP-H6 PR3 のドメインスライス)。
export type TimelineSliceState = {
  trackedTopics: string[];
  topicInput: string;
  timelinesByKey: Record<string, PostView[]>;
  timelineNextCursorByKey: Record<string, TimelineCursor | null>;
  timelineLoadingMoreByKey: Record<string, boolean>;
  /** 直近に読んだページの範囲で、まだ取得できていない投稿の数(#1239 AC-4)。 */
  timelineUnavailableByKey: Record<string, number>;
  pendingTimelineSnapshotsByKey: Record<string, PostView[]>;
  pendingTimelineCountsByKey: Record<string, number>;
  pendingTimelineNextCursorByKey: Record<string, TimelineCursor | null>;
  /** 保留した先頭のページの、まだ取得できていない投稿の数(#1239 AC-4)。適用で読んだ範囲を捨てるときに使う。 */
  pendingTimelineUnavailableByKey: Record<string, number>;
  timelineScopeByTopic: Record<string, TimelineScope>;
  composeChannelByTopic: Record<string, ChannelRef>;
  threadsById: Record<string, PostView[]>;
  threadNextCursorById: Record<string, TimelineCursor | null>;
  threadLoadingMoreById: Record<string, boolean>;
  /** 直近に読んだページの範囲で、まだ取得できていない返信の数(#1239 AC-4)。 */
  threadUnavailableById: Record<string, number>;
  selectedThread: string | null;
  focusedObjectId: string | null;
  /** Session-only identity of an explicit post focus request, not a refresh. */
  threadFocusRequestId: number;
};

export function createInitialTimelineSlice(): TimelineSliceState {
  return {
    trackedTopics: [...STARTER_TOPICS],
    topicInput: '',
    timelinesByKey: Object.fromEntries(
      STARTER_TOPICS.map((topic) => [timelineScopeStorageKey(topic, PUBLIC_TIMELINE_SCOPE), []])
    ),
    timelineNextCursorByKey: Object.fromEntries(
      STARTER_TOPICS.map((topic) => [timelineScopeStorageKey(topic, PUBLIC_TIMELINE_SCOPE), null])
    ),
    timelineLoadingMoreByKey: Object.fromEntries(
      STARTER_TOPICS.map((topic) => [timelineScopeStorageKey(topic, PUBLIC_TIMELINE_SCOPE), false])
    ),
    timelineUnavailableByKey: {},
    pendingTimelineSnapshotsByKey: {},
    pendingTimelineCountsByKey: {},
    pendingTimelineNextCursorByKey: {},
    pendingTimelineUnavailableByKey: {},
    timelineScopeByTopic: buildStarterTopicRecord(() => ({ ...PUBLIC_TIMELINE_SCOPE })),
    composeChannelByTopic: buildStarterTopicRecord(() => ({ ...PUBLIC_CHANNEL_REF })),
    threadsById: {},
    threadNextCursorById: {},
    threadLoadingMoreById: {},
    threadUnavailableById: {},
    selectedThread: null,
    focusedObjectId: null,
    threadFocusRequestId: 0,
  };
}

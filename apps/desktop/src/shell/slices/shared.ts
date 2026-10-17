import type { ChannelRef, CreateAttachmentInput, TimelineScope } from '@/lib/api';
import { type ExtendedPanelStatus } from '@/components/extended/types';

// スライス横断で使う基本型・定数(WP-H6 PR3)。
// スライス → shared の一方向依存に固定し、store.ts との循環を作らない。

export type DraftMediaItem = {
  id: string;
  source_name: string;
  preview_url: string;
  attachments: CreateAttachmentInput[];
};

export type AsyncPanelState = {
  status: ExtendedPanelStatus;
  error: string | null;
};

export const DEFAULT_ASYNC_PANEL_STATE: AsyncPanelState = {
  status: 'loading',
  error: null,
};

export const DEFAULT_TOPIC = 'kukuri:topic:general';
export const STARTER_TOPICS = [
  DEFAULT_TOPIC,
  'kukuri:topic:dev',
  'kukuri:topic:test',
] as const;
export const PUBLIC_CHANNEL_REF: ChannelRef = { kind: 'public' };
export const PUBLIC_TIMELINE_SCOPE: TimelineScope = { kind: 'public' };

export function timelineScopeStorageKey(topic: string, scope: TimelineScope): string {
  // 公開スレッド（#1840）と private channel の ID は重ならないので、どちらも channel の ID で分ける。
  if (scope.kind === 'channel' || scope.kind === 'public_channel') {
    return `${topic}::channel::${scope.channel_id}`;
  }
  return `${topic}::${scope.kind}`;
}

export function timelineStorageKeyForChannel(topic: string, channelId: string | null): string {
  return timelineScopeStorageKey(
    topic,
    channelId ? { kind: 'channel', channel_id: channelId } : PUBLIC_TIMELINE_SCOPE
  );
}

export function activeTimelineStorageKey(
  state: { timelineScopeByTopic: Record<string, TimelineScope> },
  topic: string
): string {
  return timelineScopeStorageKey(topic, state.timelineScopeByTopic[topic] ?? PUBLIC_TIMELINE_SCOPE);
}

export function buildStarterTopicRecord<T>(factory: () => T): Record<string, T> {
  return Object.fromEntries(STARTER_TOPICS.map((topic) => [topic, factory()])) as Record<
    string,
    T
  >;
}

import type { ChannelRef, TimelineScope } from '@/lib/api';
import type {
  PrimarySection,
  ProfileConnectionsView,
  ProfileWorkspaceMode,
  SettingsSection,
  TimelineWorkspaceView,
} from '@/components/shell/types';

export type DesktopShellRouteOverrides = {
  activeTopic?: string;
  composeTarget?: ChannelRef;
  primarySection?: PrimarySection;
  profileMode?: ProfileWorkspaceMode;
  profileConnectionsView?: ProfileConnectionsView;
  selectedAuthorPubkey?: string | null;
  directMessagePaneOpen?: boolean;
  selectedDirectMessagePeerPubkey?: string | null;
  selectedThread?: string | null;
  focusedObjectId?: string | null;
  settingsOpen?: boolean;
  settingsSection?: SettingsSection;
  timelineScope?: TimelineScope;
  timelineView?: TimelineWorkspaceView;
  selectedChannelId?: string | null;
  selectedLiveSessionId?: string | null;
  selectedGameRoomId?: string | null;
};

export type OpenThreadOptions = {
  /** 明示的な通知操作の世代を取得・navigation完了まで引き継ぐ。永続化しない。 */
  isCurrent?: () => boolean;
  /**
   * Thread を開く時点で topic の選択 channel として固定する channel。
   * 指定(null 含む)があれば global の選択 channel ではなくこの値で scope / route を同期する。
   * 未指定なら従来どおり現在の選択 channel を引き継ぐ。
   */
  channelId?: string | null;
  focusObjectId?: string | null;
  historyMode?: 'push' | 'replace';
  normalizeOnEmpty?: boolean;
  parentColumnId?: string;
  topic?: string;
};

export type OpenAuthorOptions = {
  fromThread?: boolean;
  historyMode?: 'push' | 'replace';
  normalizeOnError?: boolean;
  parentColumnId?: string;
  threadId?: string | null;
  preserveDirectMessageContext?: boolean;
  directMessagePeerPubkey?: string | null;
};

export const PRIMARY_SECTION_ITEMS: Array<{
  id: PrimarySection;
  label: string;
}> = [
  { id: 'timeline', label: 'Timeline' },
  { id: 'explore', label: 'Explore' },
  { id: 'live', label: 'Live' },
  { id: 'game', label: 'Game' },
  { id: 'messages', label: 'Messages' },
  { id: 'profile', label: 'Profile' },
];

export const SETTINGS_SECTION_COPY: Array<{
  id: SettingsSection;
  label: string;
  description: string;
}> = [
  {
    id: 'about',
    label: 'About / Legal',
    description: 'Terms, privacy, app version, and consent status.',
  },
  {
    id: 'appearance',
    label: 'Appearance',
    description: 'Local light and dark theme selection.',
  },
  {
    id: 'system',
    label: 'System',
    description: 'Window close behavior and desktop integration.',
  },
  {
    id: 'keyboard',
    label: 'Keyboard',
    description: 'Assigned keys and the screen or focus each one needs.',
  },
  {
    id: 'safety',
    label: 'Safety',
    description: 'Adult material display preference.',
  },
  {
    id: 'notifications',
    label: 'Notifications',
    description: 'OS notification categories, quiet mode, and preview text.',
  },
  {
    id: 'backup',
    label: 'Backup & restore',
    description: 'Create and restore an encrypted backup of this device\'s data.',
  },
  {
    id: 'account',
    label: 'Account',
    description: 'Key-only export, import, and account switching.',
  },
  {
    id: 'connectivity',
    label: 'Connectivity',
    description: 'Sync summary, peer tickets, and global error visibility.',
  },
  {
    id: 'discovery',
    label: 'Discovery',
    description: 'Seeded DHT configuration and discovery diagnostics.',
  },
  {
    id: 'community-node',
    label: 'Community Node',
    description: 'Configured community nodes, auth, consent, and refresh actions.',
  },
  {
    id: 'reactions',
    label: 'Reactions',
    description: 'Custom reaction creation and saved reaction management.',
  },
  {
    id: 'release',
    label: 'Release',
    description: 'Preview updates, diagnostics, and resources.',
  },
  {
    id: 'developer',
    label: 'Developer',
    description: 'Developer mode and work-in-progress feature visibility.',
  },
];

export const PRIMARY_SECTION_PATHS: Record<PrimarySection, string> = {
  timeline: '/timeline',
  explore: '/explore',
  live: '/live',
  game: '/game',
  messages: '/messages',
  profile: '/profile',
  notifications: '/notifications',
};

export function isSettingsSection(value: string | null): value is SettingsSection {
  return (
    value === 'about' ||
    value === 'appearance' ||
    value === 'system' ||
    value === 'keyboard' ||
    value === 'safety' ||
    value === 'notifications' ||
    value === 'backup' ||
    value === 'connectivity' ||
    value === 'discovery' ||
    value === 'community-node' ||
    value === 'reactions' ||
    value === 'release' ||
    value === 'developer' ||
    value === 'account'
  );
}

export function isProfileConnectionsView(
  value: string | null
): value is ProfileConnectionsView {
  return (
    value === 'following' ||
    value === 'followed' ||
    value === 'muted' ||
    value === 'blocking'
  );
}

export function parsePrimarySectionPath(pathname: string): PrimarySection | null {
  const normalizedPath = pathname === '/' ? '/timeline' : pathname;
  if (normalizedPath === '/channels') {
    return null;
  }
  const match = (
    Object.entries(PRIMARY_SECTION_PATHS) as Array<[PrimarySection, string]>
  ).find(([, path]) => path === normalizedPath);
  return match?.[0] ?? null;
}

export type HashRouteLocation = {
  pathname: string;
  search: string;
};

export type ParsedRouteState = {
  primarySection: PrimarySection | null;
  requestedTopic: string | null;
  requestedChannel: string | null;
  requestedTimelineView: string | null;
  requestedTimelineScope: string | null;
  requestedComposeTarget: string | null;
  requestedSettingsSection: string | null;
  requestedContext: string | null;
  requestedProfileMode: string | null;
  requestedConnectionsView: string | null;
  requestedThreadId: string | null;
  requestedFocusObjectId: string | null;
  requestedAuthorPubkey: string | null;
  requestedPeerPubkey: string | null;
  requestedSessionId: string | null;
  requestedRoomId: string | null;
};

export function parseHashRouteLocation(hash: string): HashRouteLocation {
  const normalizedHash = hash.startsWith('#') ? hash.slice(1) : hash;
  if (!normalizedHash) {
    return {
      pathname: '/',
      search: '',
    };
  }

  const searchIndex = normalizedHash.indexOf('?');
  if (searchIndex === -1) {
    return {
      pathname: normalizedHash || '/',
      search: '',
    };
  }

  return {
    pathname: normalizedHash.slice(0, searchIndex) || '/',
    search: normalizedHash.slice(searchIndex),
  };
}

export function parseShellRouteState(location: HashRouteLocation): ParsedRouteState {
  const params = new URLSearchParams(location.search);
  return {
    primarySection: parsePrimarySectionPath(location.pathname),
    requestedTopic: params.get('topic')?.trim() || null,
    requestedChannel: params.get('channel')?.trim() || null,
    requestedTimelineView: params.get('timelineView'),
    requestedTimelineScope: params.get('timelineScope'),
    requestedComposeTarget: params.get('composeTarget'),
    requestedSettingsSection: params.get('settings'),
    requestedContext: params.get('context'),
    requestedProfileMode: params.get('profileMode'),
    requestedConnectionsView: params.get('connectionsView'),
    requestedThreadId: params.get('threadId'),
    requestedFocusObjectId: params.get('focusObjectId')?.trim() || null,
    requestedAuthorPubkey: params.get('authorPubkey'),
    requestedPeerPubkey: params.get('peerPubkey'),
    requestedSessionId: params.get('sessionId')?.trim() || null,
    requestedRoomId: params.get('roomId')?.trim() || null,
  };
}

export function resolveHashBackedRouteLocation(
  pathname: string,
  search: string,
  hash: string
): HashRouteLocation {
  const hashRouteLocation = parseHashRouteLocation(hash);
  const shouldUseHashPathname =
    parsePrimarySectionPath(pathname) === null ||
    (pathname === '/' && hashRouteLocation.pathname !== '/');

  return {
    pathname: shouldUseHashPathname ? hashRouteLocation.pathname : pathname,
    search: search || hashRouteLocation.search,
  };
}

export function parseLegacyRequestedChannel(
  requestedTimelineScopeValue: string | null,
  requestedComposeTargetValue: string | null
): string | null {
  return [requestedComposeTargetValue, requestedTimelineScopeValue]
    .filter((value): value is string => Boolean(value))
    .map((value) => {
      if (value.startsWith('channel:')) {
        return value.slice('channel:'.length);
      }
      return null;
    })
    .find((value): value is string => value !== null) ?? null;
}

export type RouteState = {
  activeTopic: string;
  primarySection: PrimarySection;
  timelineView: TimelineWorkspaceView;
  profileMode: ProfileWorkspaceMode;
  profileConnectionsView: ProfileConnectionsView;
  selectedThread: string | null;
  focusedObjectId: string | null;
  selectedAuthorPubkey: string | null;
  selectedDirectMessagePeerPubkey: string | null;
  settingsOpen: boolean;
  settingsSection: SettingsSection;
  selectedChannelId: string | null;
  selectedLiveSessionId: string | null;
  selectedGameRoomId: string | null;
};

export function buildShellUrl(options: RouteState): string {
  const search = new URLSearchParams();
  search.set('topic', options.activeTopic);

  if (
    options.primarySection !== 'messages' &&
    options.primarySection !== 'notifications' &&
    options.selectedChannelId &&
    !(options.primarySection === 'timeline' && options.timelineView === 'bookmarks')
  ) {
    search.set('channel', options.selectedChannelId);
  }

  if (options.primarySection === 'timeline' && options.timelineView === 'bookmarks') {
    search.set('timelineView', 'bookmarks');
  }

  if (options.primarySection === 'messages') {
    if (options.selectedDirectMessagePeerPubkey) {
      search.set('peerPubkey', options.selectedDirectMessagePeerPubkey);
    }
    if (options.selectedAuthorPubkey) {
      search.set('authorPubkey', options.selectedAuthorPubkey);
    }
  } else if (options.primarySection !== 'notifications' && options.selectedThread) {
    search.set('context', 'thread');
    search.set('threadId', options.selectedThread);
    if (options.focusedObjectId) {
      search.set('focusObjectId', options.focusedObjectId);
    }
    if (options.selectedAuthorPubkey) {
      search.set('authorPubkey', options.selectedAuthorPubkey);
    }
  } else if (options.primarySection !== 'notifications' && options.selectedAuthorPubkey) {
    search.set('context', 'author');
    search.set('authorPubkey', options.selectedAuthorPubkey);
  }

  if (options.primarySection === 'profile' && options.profileMode === 'edit') {
    search.set('profileMode', 'edit');
  }
  if (options.primarySection === 'profile' && options.profileMode === 'connections') {
    search.set('profileMode', 'connections');
    search.set('connectionsView', options.profileConnectionsView);
  }
  if (options.settingsOpen) {
    search.set('settings', options.settingsSection);
  }
  if (options.primarySection === 'live' && options.selectedLiveSessionId) {
    search.set('sessionId', options.selectedLiveSessionId);
  }
  if (options.primarySection === 'game' && options.selectedGameRoomId) {
    search.set('roomId', options.selectedGameRoomId);
  }

  const nextPath = PRIMARY_SECTION_PATHS[options.primarySection];
  const nextSearch = search.toString();
  return nextSearch ? `${nextPath}?${nextSearch}` : nextPath;
}

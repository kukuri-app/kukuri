import { useCallback, type MutableRefObject } from 'react';
import type { NavigateFunction } from 'react-router-dom';

import {
  buildShellUrl,
  parseHashRouteLocation,
  type DesktopShellRouteOverrides,
  type HashRouteLocation,
} from '@/shell/routes';
import { channelIdOfRef, channelIdOfScope } from '@/shell/presentation';
import type { DesktopShellStoreApi } from '@/shell/store';
import { routeStateForColumn } from '@/shell/routing/initialWorkspaceRoute';
import { activeWorkspaceColumn } from '@/shell/slices/workspace';

type UseSyncRouteArgs = {
  navigate: NavigateFunction;
  pendingRouteUrlRef: MutableRefObject<string | null>;
  resolvedRouteLocation: HashRouteLocation;
  storeApi: DesktopShellStoreApi;
};

export function useSyncRoute({
  navigate,
  pendingRouteUrlRef,
  resolvedRouteLocation,
  storeApi,
}: UseSyncRouteArgs) {
  return useCallback(
    (mode: 'push' | 'replace' = 'replace', overrides?: DesktopShellRouteOverrides) => {
      const currentState = storeApi.getState();
      const activeColumn = activeWorkspaceColumn(currentState.workspaceState);
      const columnRoute = routeStateForColumn(activeColumn);
      const hasOverride = <K extends keyof DesktopShellRouteOverrides>(key: K) =>
        overrides ? Object.prototype.hasOwnProperty.call(overrides, key) : false;
      const nextTopic = overrides?.activeTopic ?? columnRoute?.activeTopic ?? activeColumn.scope?.topicId;
      if (!nextTopic) return;
      const nextPrimarySection =
        overrides?.primarySection ?? columnRoute?.primarySection ?? 'timeline';
      const nextTimelineView =
        overrides?.timelineView ?? columnRoute?.timelineView ?? 'feed';
      const nextPublicThreadList = hasOverride('publicThreadList')
        ? Boolean(overrides?.publicThreadList)
        : Boolean(columnRoute?.publicThreadList);
      const nextProfileMode =
        overrides?.profileMode ?? currentState.shellChromeState.profileMode;
      const nextProfileConnectionsView =
        overrides?.profileConnectionsView ?? currentState.shellChromeState.profileConnectionsView;
      const nextSelectedThread = hasOverride('selectedThread')
        ? overrides?.selectedThread ?? null
        : columnRoute?.selectedThread ?? currentState.selectedThread;
      const nextFocusedObjectId = hasOverride('focusedObjectId')
        ? overrides?.focusedObjectId ?? null
        : currentState.focusedObjectId;
      const nextSelectedAuthorPubkey = hasOverride('selectedAuthorPubkey')
        ? overrides?.selectedAuthorPubkey ?? null
        : columnRoute?.selectedAuthorPubkey ?? currentState.selectedAuthorPubkey;
      const nextSelectedDirectMessagePeerPubkey = hasOverride('selectedDirectMessagePeerPubkey')
        ? overrides?.selectedDirectMessagePeerPubkey ?? null
        : columnRoute?.selectedDirectMessagePeerPubkey ??
          currentState.selectedDirectMessagePeerPubkey;
      const nextSelectedLiveSessionId = hasOverride('selectedLiveSessionId')
        ? overrides?.selectedLiveSessionId ?? null
        : columnRoute?.selectedLiveSessionId ?? currentState.selectedLiveSessionId;
      const nextSelectedLivePlace = hasOverride('selectedLivePlace')
        ? overrides?.selectedLivePlace ?? null
        : columnRoute?.selectedLivePlace ?? null;
      const nextSelectedGameRoomId = hasOverride('selectedGameRoomId')
        ? overrides?.selectedGameRoomId ?? null
        : columnRoute?.selectedGameRoomId ?? currentState.selectedGameRoomId;
      const nextSettingsOpen = hasOverride('settingsOpen')
        ? overrides?.settingsOpen ?? false
        : currentState.shellChromeState.settingsOpen;
      const nextSettingsSection =
        overrides?.settingsSection ?? currentState.shellChromeState.activeSettingsSection;
      let nextSelectedChannelId = columnRoute?.selectedChannelId ?? null;

      if (hasOverride('composeTarget')) {
        nextSelectedChannelId = channelIdOfRef(overrides?.composeTarget);
      } else if (hasOverride('timelineScope')) {
        nextSelectedChannelId = channelIdOfScope(overrides?.timelineScope);
      }

      const nextUrl = buildShellUrl({
        activeTopic: nextTopic,
        focusedObjectId: nextFocusedObjectId,
        primarySection: nextPrimarySection,
        profileConnectionsView: nextProfileConnectionsView,
        profileMode: nextProfileMode,
        selectedAuthorPubkey: nextSelectedAuthorPubkey,
        selectedChannelId: nextSelectedChannelId,
        selectedDirectMessagePeerPubkey: nextSelectedDirectMessagePeerPubkey,
        selectedGameRoomId: nextSelectedGameRoomId,
        selectedLiveSessionId: nextSelectedLiveSessionId,
        selectedLivePlace: nextSelectedLivePlace,
        selectedThread: nextSelectedThread,
        settingsOpen: nextSettingsOpen,
        settingsSection: nextSettingsSection,
        timelineView: nextTimelineView,
        publicThreadList: nextPublicThreadList,
      });
      // HashRouter は hash を同期更新し、location の render は後から追いつく。
      // A → B → A の再選択を古い render の A と比較すると、最後の navigate を落とす。
      const renderedUrl = `${resolvedRouteLocation.pathname}${resolvedRouteLocation.search}`;
      const currentLocation = typeof window !== 'undefined' && window.location.hash
        ? parseHashRouteLocation(window.location.hash)
        : null;
      const currentUrl = currentLocation
        ? `${currentLocation.pathname}${currentLocation.search}`
        : renderedUrl;

      if (currentUrl !== nextUrl) {
        pendingRouteUrlRef.current = nextUrl;
        navigate(nextUrl, { replace: mode === 'replace' });
        return;
      }

      // 同じ遷移先への再要求は history を増やさず、router の観測までは pending を保つ。
      pendingRouteUrlRef.current = renderedUrl === nextUrl ? null : nextUrl;
    },
    [
      navigate,
      pendingRouteUrlRef,
      resolvedRouteLocation.pathname,
      resolvedRouteLocation.search,
      storeApi,
    ]
  );
}

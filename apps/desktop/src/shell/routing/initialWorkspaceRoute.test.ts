import { describe, expect, it } from 'vitest';

import {
  initialHashForRestoredWorkspace,
  isDefaultStartupHash,
} from './initialWorkspaceRoute';
import type { ColumnState, WorkspaceState } from '@/shell/slices/workspace';

const SCOPE = { topicId: 'kukuri:topic:general', channelId: null };
const PRIVATE_SCOPE = { topicId: 'kukuri:topic:general', channelId: 'channel-1' };

function workspaceWith(column: Partial<ColumnState> & Pick<ColumnState, 'id' | 'kind'>): WorkspaceState {
  const full: ColumnState = {
    scope: SCOPE,
    pinned: false,
    preferredDesktopSpan: 1,
    ...column,
  } as ColumnState;
  return {
    columns: [full],
    activeColumnId: full.id,
    controlCenterOpen: false,
    activeLayoutId: null,
  };
}

describe('isDefaultStartupHash', () => {
  it('treats empty and root hashes as default, explicit routes as deep links', () => {
    expect(isDefaultStartupHash('')).toBe(true);
    expect(isDefaultStartupHash('#')).toBe(true);
    expect(isDefaultStartupHash('#/')).toBe(true);
    expect(isDefaultStartupHash('#/timeline?topic=t')).toBe(false);
    expect(isDefaultStartupHash('#/notifications')).toBe(false);
  });
});

describe('initialHashForRestoredWorkspace', () => {
  it('keeps a public-thread channel when a saved column still selects bookmarks', () => {
    const channelId = 'pc-1700000000000-0123456789abcdef';
    const workspace = workspaceWith({
      id: 'detail', kind: 'timeline', scope: { ...SCOPE, channelId },
      timelineView: 'bookmarks', timelineFilter: 'following',
    });
    expect(initialHashForRestoredWorkspace(workspace))
      .toBe(`#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=${channelId}`);
    expect(workspace.columns[0].timelineView).toBe('bookmarks');
  });

  it('maps each Column kind to its canonical target', () => {
    expect(
      initialHashForRestoredWorkspace(workspaceWith({ id: 'c', kind: 'timeline', scope: PRIVATE_SCOPE }))
    ).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
    expect(
      initialHashForRestoredWorkspace(
        workspaceWith({ id: 'c', kind: 'thread', entityId: 'post-1' })
      )
    ).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-1');
    expect(
      initialHashForRestoredWorkspace(
        workspaceWith({ id: 'c', kind: 'profile', entityId: 'a'.repeat(64) })
      )
    ).toBe(`#/timeline?topic=kukuri%3Atopic%3Ageneral&context=author&authorPubkey=${'a'.repeat(64)}`);
    expect(
      initialHashForRestoredWorkspace(workspaceWith({ id: 'c', kind: 'profile' }))
    ).toBe('#/profile?topic=kukuri%3Atopic%3Ageneral');
    expect(
      initialHashForRestoredWorkspace(workspaceWith({ id: 'c', kind: 'notifications' }))
    ).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
    expect(
      initialHashForRestoredWorkspace(workspaceWith({ id: 'c', kind: 'messages' }))
    ).toBe('#/messages?topic=kukuri%3Atopic%3Ageneral');
    expect(
      initialHashForRestoredWorkspace(
        workspaceWith({ id: 'c', kind: 'conversation', entityId: 'b'.repeat(64) })
      )
    ).toBe(`#/messages?topic=kukuri%3Atopic%3Ageneral&peerPubkey=${'b'.repeat(64)}`);
    expect(
      initialHashForRestoredWorkspace(workspaceWith({ id: 'c', kind: 'explore' }))
    ).toBe('#/explore?topic=kukuri%3Atopic%3Ageneral');
    expect(
      initialHashForRestoredWorkspace(
        workspaceWith({ id: 'c', kind: 'stream', entityId: 'session-1' })
      )
    ).toBe('#/live?topic=kukuri%3Atopic%3Ageneral&sessionId=session-1');
    expect(
      initialHashForRestoredWorkspace(
        workspaceWith({ id: 'c', kind: 'metaverse', entityId: 'room-1' })
      )
    ).toBe('#/game?topic=kukuri%3Atopic%3Ageneral&roomId=room-1');
  });

  it('returns null when the active Column is missing, has no scope, or lacks a required entity', () => {
    const missingActive = {
      ...workspaceWith({ id: 'c', kind: 'timeline' }),
      activeColumnId: 'other',
    };
    expect(initialHashForRestoredWorkspace(missingActive)).toBeNull();
    expect(
      initialHashForRestoredWorkspace(workspaceWith({ id: 'c', kind: 'thread' }))
    ).toBeNull();
    expect(
      initialHashForRestoredWorkspace(
        workspaceWith({ id: 'c', kind: 'conversation' })
      )
    ).toBeNull();
    expect(
      initialHashForRestoredWorkspace(
        workspaceWith({ id: 'c', kind: 'timeline', scope: undefined })
      )
    ).toBeNull();
  });
});

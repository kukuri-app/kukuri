/**
 * #1528: channel を選ぶ操作(handleSelectPrivateChannel)は、呼び出し直後(route の同期より前)に
 * 選んだ channel の列を active にし、公開の列を経由しない。
 * ハーネスの syncRoute は stub なので、ここで見える状態は handler 自身の結果だけで、
 * route の同期による投影のやり直しは含まない。
 */
import { act } from '@testing-library/react';
import { beforeEach, describe, expect, test } from 'vitest';

import type { JoinedPrivateChannelView } from '@/lib/api';
import { columnIdentityId, openTransientColumn, type WorkspaceState } from '@/shell/slices/workspace';
import { renderActionsHook } from '@/shell/testSupport/renderShellActions';
import { resetWindowHash } from '@/shell/testSupport/renderShellHook';

const GENERAL = 'kukuri:topic:general';
const DEV = 'kukuri:topic:dev';

function joinedChannel(topicId: string, channelId: string): JoinedPrivateChannelView {
  return {
    topic_id: topicId,
    channel_id: channelId,
    label: 'core',
    creator_pubkey: 'c'.repeat(64),
    owner_pubkey: 'c'.repeat(64),
    joined_via_pubkey: null,
    audience_kind: 'invite_only',
    is_owner: false,
    current_epoch_id: 'epoch-1',
    archived_epoch_ids: [],
    sharing_state: 'open',
    rotation_required: false,
    participant_count: 2,
    stale_participant_count: 0,
  };
}

function timelineColumnId(topicId: string, channelId: string | null) {
  return columnIdentityId('timeline', { topicId, channelId });
}

function renderWithJoinedChannel(
  topicId: string,
  channelId: string,
  workspace?: (current: WorkspaceState) => WorkspaceState
) {
  return renderActionsHook({
    preset: (current) => ({
      joinedChannelsByTopic: { [topicId]: [joinedChannel(topicId, channelId)] },
      channelPanelStateByTopic: { [topicId]: { status: 'ready', error: null } },
      ...(workspace ? { workspaceState: workspace(current.workspaceState) } : {}),
    }),
  });
}

function columnIds(view: ReturnType<typeof renderActionsHook>) {
  return view.store.getState().workspaceState.columns.map((column) => column.id);
}

beforeEach(() => {
  resetWindowHash();
});

describe('handleSelectPrivateChannel (#1528)', () => {
  test('TR-1: 公開の列が active のとき、呼び出し直後に channel の列が active になり、公開の列を経由しない', () => {
    const view = renderWithJoinedChannel(GENERAL, 'channel-1');
    const channelColumnId = timelineColumnId(GENERAL, 'channel-1');
    expect(view.store.getState().workspaceState.activeColumnId).toBe(timelineColumnId(GENERAL, null));
    const activated: string[] = [];
    const unsubscribe = view.store.subscribe((state, previous) => {
      if (state.workspaceState.activeColumnId !== previous.workspaceState.activeColumnId) {
        activated.push(state.workspaceState.activeColumnId);
      }
    });

    act(() => {
      view.result.current.handleSelectPrivateChannel(GENERAL, 'channel-1');
      // 呼び出し直後(React の flush より前)の active な列。
      expect(view.store.getState().workspaceState.activeColumnId).toBe(channelColumnId);
    });
    unsubscribe();

    expect(activated).toEqual([channelColumnId]);
    expect(view.mocks.syncRoute).toHaveBeenCalledWith('replace', {
      activeTopic: GENERAL,
      primarySection: 'timeline',
      timelineScope: { kind: 'channel', channel_id: 'channel-1' },
      composeTarget: { kind: 'private_channel', channel_id: 'channel-1' },
    });
    // 旧順序では route の同期が投影をやり直す際に読み込んでいた分を、handler 自身が読み込む。
    expect(view.mocks.loadTopics).toHaveBeenCalledWith(
      view.store.getState().trackedTopics,
      GENERAL,
      null
    );
  });

  test('TR-2: channel の列が active のまま同じ channel を選ぶと、active のままで列の並びも変わらない', () => {
    const channelColumnId = timelineColumnId(GENERAL, 'channel-1');
    const view = renderWithJoinedChannel(GENERAL, 'channel-1', (current) =>
      openTransientColumn(current, {
        id: channelColumnId,
        kind: 'timeline',
        scope: { topicId: GENERAL, channelId: 'channel-1' },
        pinned: false,
      })
    );
    expect(view.store.getState().workspaceState.activeColumnId).toBe(channelColumnId);
    const idsBefore = columnIds(view);

    act(() => {
      view.result.current.handleSelectPrivateChannel(GENERAL, 'channel-1');
      expect(view.store.getState().workspaceState.activeColumnId).toBe(channelColumnId);
    });

    expect(view.store.getState().workspaceState.activeColumnId).toBe(channelColumnId);
    expect(columnIds(view)).toEqual(idsBefore);
  });

  test('TR-3: 別の topic の channel を選ぶと、その topic の公開の列と channel の列がこの順で末尾に開き、channel の列が呼び出し直後に active になる', () => {
    const view = renderWithJoinedChannel(DEV, 'channel-dev');
    const idsBefore = columnIds(view);

    act(() => {
      view.result.current.handleSelectPrivateChannel(DEV, 'channel-dev');
      expect(view.store.getState().workspaceState.activeColumnId).toBe(
        timelineColumnId(DEV, 'channel-dev')
      );
    });

    expect(columnIds(view)).toEqual([
      ...idsBefore,
      timelineColumnId(DEV, null),
      timelineColumnId(DEV, 'channel-dev'),
    ]);
    expect(view.mocks.syncRoute).toHaveBeenCalledWith(
      'replace',
      expect.objectContaining({ activeTopic: DEV })
    );
  });
});

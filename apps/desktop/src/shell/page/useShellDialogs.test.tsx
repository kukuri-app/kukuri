import type { ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';
import { useShellDialogs } from '@/shell/page/useShellDialogs';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

type DialogRouteProps = {
  activePrimarySection: 'timeline' | 'live' | 'game';
};

describe('useShellDialogs', () => {
  test('closes the game dialog when leaving the game section', () => {
    const store = createDesktopShellStore();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>
    );
    const hook = renderHook(
      ({ activePrimarySection }: DialogRouteProps) => useShellDialogs({ activePrimarySection }),
      {
        initialProps: {
          activePrimarySection: 'timeline',
        },
        wrapper,
      }
    );

    hook.rerender({ activePrimarySection: 'game' });
    act(() => hook.result.current.setGameCreateDialogOpen(true));
    hook.rerender({ activePrimarySection: 'timeline' });
    expect(hook.result.current.gameCreateDialogOpen).toBe(false);
  });

  test('owns pending leave state across confirm and cancel', async () => {
    const store = createDesktopShellStore();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>
    );
    const hook = renderHook(
      () =>
        useShellDialogs({ activePrimarySection: 'timeline' }),
      { wrapper }
    );
    const leaveChannel = vi.fn().mockResolvedValue(undefined);

    act(() => hook.result.current.openLeaveChannelDialog('topic-a', 'channel-a'));
    expect(hook.result.current.leaveChannelDialogOpen).toBe(true);
    await act(async () => hook.result.current.confirmLeaveChannel(leaveChannel));
    expect(leaveChannel).toHaveBeenCalledWith('topic-a', 'channel-a');
    expect(hook.result.current.leaveChannelDialogOpen).toBe(false);

    act(() => hook.result.current.openLeaveChannelDialog('topic-b', 'channel-b'));
    act(() => hook.result.current.setLeaveChannelDialogOpen(false));
    await act(async () => hook.result.current.confirmLeaveChannel(leaveChannel));
    expect(leaveChannel).toHaveBeenCalledTimes(1);
  });
});

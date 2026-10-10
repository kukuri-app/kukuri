import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import type { NotificationView } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import {
  buildNotification,
  buildPaginatedPost,
  getDetailPane,
  openControlCenter,
  openSettingsDrawer,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

const activation = vi.hoisted(() => ({
  open: undefined as ((notification: NotificationView) => void | Promise<void>) | undefined,
}));

vi.mock('@/shell/useOsNotificationActivation', () => ({
  useOsNotificationActivation: (
    _api: unknown,
    _account: string | null,
    onActivate: (notification: NotificationView) => void | Promise<void>
  ) => {
    activation.open = onActivate;
  },
}));

beforeEach(() => {
  setViewportWidth(1280);
  window.history.replaceState(null, '', '/');
  activation.open = undefined;
});

test.each(['settings', 'control-center'] as const)(
  'OS notification opens its thread above %s without marking unrelated notifications read',
  async (overlay) => {
    const user = userEvent.setup();
    const target = buildNotification({
      notification_id: 'os-click-target',
      object_id: 'os-thread-root',
      thread_root_object_id: 'os-thread-root',
    });
    const api = createDesktopMockApi({
      notifications: [target],
      seedPosts: {
        'kukuri:topic:general': [
          buildPaginatedPost(1, {
            object_id: 'os-thread-root',
            root_id: 'os-thread-root',
            content: 'OS notification thread root',
          }),
        ],
      },
    });
    const markAllRead = vi.fn(api.markAllNotificationsRead);
    api.markAllNotificationsRead = markAllRead;
    renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral', api);
    if (overlay === 'settings') await openSettingsDrawer(user);
    else await openControlCenter(user);
    expect(activation.open).toBeDefined();

    await act(async () => {
      await activation.open?.(target);
    });

    await waitFor(() => {
      expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
      expect(screen.queryByRole('complementary', { name: 'Control Center' })).not.toBeInTheDocument();
      expect(getDetailPane('Thread')).toBeInTheDocument();
    });
    expect(window.location.hash).toContain('threadId=os-thread-root');
    expect(markAllRead).not.toHaveBeenCalled();
  }
);

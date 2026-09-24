import type { NotificationCursor, NotificationStatusView, NotificationView } from '@/lib/api';

import { type AsyncPanelState, DEFAULT_ASYNC_PANEL_STATE } from '@/shell/slices/shared';

/// 通知(WP-H6 PR3 のドメインスライス)。
export type NotificationsSliceState = {
  notifications: NotificationView[];
  notificationsNewerCursor: NotificationCursor | null;
  notificationsOlderCursor: NotificationCursor | null;
  notificationsPageRequestCursor: NotificationCursor | null;
  notificationsPageRequestBefore: boolean;
  notificationsLoadingPage: boolean;
  notificationStatus: NotificationStatusView;
  notificationPanelState: AsyncPanelState;
  notificationAutoReadError: string | null;
};

export const DEFAULT_NOTIFICATION_STATUS: NotificationStatusView = {
  unread_count: 0,
};

export function createInitialNotificationsSlice(): NotificationsSliceState {
  return {
    notifications: [],
    notificationsNewerCursor: null,
    notificationsOlderCursor: null,
    notificationsPageRequestCursor: null,
    notificationsPageRequestBefore: false,
    notificationsLoadingPage: false,
    notificationStatus: DEFAULT_NOTIFICATION_STATUS,
    notificationPanelState: DEFAULT_ASYNC_PANEL_STATE,
    notificationAutoReadError: null,
  };
}

import { type DesktopApi, type NotificationStatusView } from '@/lib/api';

import { cloneNotification } from '../desktopMockModel';
import { type MockRuntime } from '../mockRuntime';

type NotificationsMock = Pick<
  DesktopApi,
  'listNotificationsPage' | 'markNotificationRead' | 'markAllNotificationsRead' | 'getNotificationStatus'
>;

export function createNotificationsMock(runtime: MockRuntime): NotificationsMock {
  return {
    async listNotificationsPage(cursor, before = false) {
      const sorted = [...runtime.notifications].sort((a, b) =>
        Number(b.received_at - a.received_at) || b.notification_id.localeCompare(a.notification_id));
      const filtered = cursor ? sorted.filter((item) => {
        const comparison = item.received_at === cursor.received_at
          ? item.notification_id.localeCompare(cursor.notification_id)
          : item.received_at < cursor.received_at ? -1 : 1;
        return before ? comparison > 0 : comparison < 0;
      }) : sorted;
      const rows = before ? filtered.slice(-21) : filtered.slice(0, 21);
      const hasMore = rows.length > 20;
      const items = (before ? rows.slice(-20) : rows.slice(0, 20)).map(cloneNotification);
      const first = items[0];
      const last = items.at(-1);
      const key = (item: typeof last) => item ? { received_at: item.received_at, notification_id: item.notification_id } : null;
      return { items,
        newer_cursor: before ? hasMore ? key(first) : null : cursor ? key(first) : null,
        older_cursor: before ? cursor ? key(last) : null : hasMore ? key(last) : null,
      };
    },
    async markNotificationRead(notificationId) {
      runtime.notifications = runtime.notifications.map((notification) =>
        notification.notification_id === notificationId && !notification.read_at
          ? { ...notification, read_at: Date.now() }
          : notification
      );
      return {
        unread_count: runtime.notifications.filter((notification) => !notification.read_at).length,
      } satisfies NotificationStatusView;
    },
    async markAllNotificationsRead() {
      const readAt = Date.now();
      runtime.notifications = runtime.notifications.map((notification) =>
        notification.read_at ? notification : { ...notification, read_at: readAt }
      );
      return {
        unread_count: 0,
      } satisfies NotificationStatusView;
    },
    async getNotificationStatus() {
      return {
        unread_count: runtime.notifications.filter((notification) => !notification.read_at).length,
      } satisfies NotificationStatusView;
    },
  };
}

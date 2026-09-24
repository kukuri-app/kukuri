import { startTransition, useCallback, useRef } from 'react';

import type { DesktopApi, NotificationCursor, NotificationView } from '@/lib/api';
import type { ShellChromeProjection } from '@/components/shell/types';
import { messageFromError } from '@/shell/presentation';
import { useDesktopShellFieldSetter, useDesktopShellStoreApi } from '@/shell/store';

export type LoadNotificationsSection = (options?: {
  markAsRead?: boolean;
  cursor?: NotificationCursor | null;
  before?: boolean;
  preserveCurrent?: boolean;
}) => Promise<void>;

type NotificationLoaderArgs = {
  api: DesktopApi;
  activePrimarySection: ShellChromeProjection['activePrimarySection'];
  translate: (key: string, options?: Record<string, unknown>) => string;
};

const sameCursor = (left: NotificationCursor | null, right: NotificationCursor | null) =>
  left?.received_at === right?.received_at && left?.notification_id === right?.notification_id;

export function useNotificationLoaders({ api, activePrimarySection, translate }: NotificationLoaderArgs) {
  const storeApi = useDesktopShellStoreApi();
  const requestId = useRef(0);
  const setNotificationStatus = useDesktopShellFieldSetter('notificationStatus');
  const setNotifications = useDesktopShellFieldSetter('notifications');
  const setNewerCursor = useDesktopShellFieldSetter('notificationsNewerCursor');
  const setOlderCursor = useDesktopShellFieldSetter('notificationsOlderCursor');
  const setRequestCursor = useDesktopShellFieldSetter('notificationsPageRequestCursor');
  const setRequestBefore = useDesktopShellFieldSetter('notificationsPageRequestBefore');
  const setLoadingPage = useDesktopShellFieldSetter('notificationsLoadingPage');
  const setNotificationAutoReadError = useDesktopShellFieldSetter('notificationAutoReadError');
  const setNotificationPanelState = useDesktopShellFieldSetter('notificationPanelState');

  const loadNotificationsSection: LoadNotificationsSection = useCallback(async (options = {}) => {
    const state = storeApi.getState();
    const account = state.syncStatus.local_author_pubkey;
    const cursor = options.preserveCurrent ? state.notificationsPageRequestCursor : options.cursor ?? null;
    const before = options.preserveCurrent ? state.notificationsPageRequestBefore : options.before ?? false;
    const currentRequest = ++requestId.current;
    if (!options.preserveCurrent) setLoadingPage(true);
    try {
      const [status, page] = await Promise.all([
        api.getNotificationStatus(), api.listNotificationsPage(cursor, before),
      ]);
      if (currentRequest !== requestId.current ||
        storeApi.getState().syncStatus.local_author_pubkey !== account) return;
      let nextStatus = status;
      let nextNotifications: NotificationView[] = page.items;
      if ((options.markAsRead ?? true) && status.unread_count > 0) {
        try {
          nextStatus = await api.markAllNotificationsRead();
          const readAt = Date.now();
          nextNotifications = page.items.map((item) => item.read_at ? item : { ...item, read_at: readAt });
          setNotificationAutoReadError(null);
        } catch (error) {
          setNotificationAutoReadError(messageFromError(error, translate('shell:notifications.errors.failedAutoRead')));
        }
      }
      if (currentRequest !== requestId.current ||
        storeApi.getState().syncStatus.local_author_pubkey !== account) return;
      startTransition(() => {
        setNotificationStatus((current) => current.unread_count === nextStatus.unread_count ? current : nextStatus);
        setNotifications((current) => current.length === nextNotifications.length &&
          current.every((item, index) => JSON.stringify(item) === JSON.stringify(nextNotifications[index]))
          ? current : nextNotifications);
        setNewerCursor((current) => sameCursor(current, page.newer_cursor) ? current : page.newer_cursor);
        setOlderCursor((current) => sameCursor(current, page.older_cursor) ? current : page.older_cursor);
        setRequestCursor((current) => sameCursor(current, cursor) ? current : cursor);
        setRequestBefore(before);
        setNotificationPanelState((current) => current.status === 'ready' && current.error === null
          ? current : { status: 'ready', error: null });
      });
    } catch (error) {
      if (currentRequest !== requestId.current ||
        storeApi.getState().syncStatus.local_author_pubkey !== account) return;
      if (options.preserveCurrent && state.notificationPanelState.status === 'ready') return;
      setNotificationPanelState({
        status: 'error',
        error: messageFromError(error, translate('shell:notifications.errors.failedToLoad')),
      });
    } finally {
      if (currentRequest === requestId.current) setLoadingPage(false);
    }
  }, [api, setLoadingPage, setNewerCursor, setNotificationAutoReadError, setNotificationPanelState,
    setNotificationStatus, setNotifications, setOlderCursor, setRequestBefore, setRequestCursor, storeApi, translate]);

  const refreshNotificationStatus = useCallback(async () => {
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    try {
      const status = await api.getNotificationStatus();
      setNotificationStatus((current) => current.unread_count === status.unread_count ? current : status);
    } catch { /* best effort badge refresh */ }
  }, [api, setNotificationStatus]);

  const refreshNotificationsFromEvent = useCallback(async () => {
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    if (activePrimarySection === 'notifications' ||
      storeApi.getState().workspaceState.columns.some((column) => column.kind === 'notifications')) {
      await loadNotificationsSection({ markAsRead: false, preserveCurrent: true });
    } else {
      await refreshNotificationStatus();
    }
  }, [activePrimarySection, loadNotificationsSection, refreshNotificationStatus, storeApi]);

  const navigateNotificationPage = useCallback(async (before: boolean) => {
    const state = storeApi.getState();
    const cursor = before ? state.notificationsNewerCursor : state.notificationsOlderCursor;
    if (!cursor || state.notificationsLoadingPage) return;
    await loadNotificationsSection({ cursor, before, markAsRead: false });
  }, [loadNotificationsSection, storeApi]);

  return { refreshNotificationStatus, refreshNotificationsFromEvent, loadNotificationsSection, navigateNotificationPage };
}

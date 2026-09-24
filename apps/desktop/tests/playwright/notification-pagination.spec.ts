import { expect, test } from '@playwright/test';

test('notification history uses bounded previous and next pages', async ({ page }) => {
  await page.addInitScript(() => {
    let desktopApi = window.__KUKURI_DESKTOP__;
    (window as typeof window & { __notificationPageReads: number }).__notificationPageReads = 0;
    Object.defineProperty(window, '__KUKURI_DESKTOP__', {
      configurable: true,
      get: () => desktopApi,
      set: (api: typeof desktopApi) => {
        desktopApi = api;
        if (!api) return;
        const source = api.listNotificationsPage();
        api.getNotificationStatus = async () => ({ unread_count: 0 });
        api.listNotificationsPage = async (cursor, before) => {
          (window as typeof window & { __notificationPageReads: number }).__notificationPageReads++;
          const base = (await source).items[0];
          if (!base) throw new Error('browser fixture lacks a notification');
          const first = !cursor || before;
          const start = first ? 0 : 20;
          const count = first ? 20 : 5;
          return {
            items: Array.from({ length: count }, (_, offset) => {
              const index = start + offset;
              const id = `notification-page-${index}`;
              return { ...base, notification_id: id, received_at: 100 - index, preview_text: id };
            }),
            newer_cursor: first ? null : { received_at: 80, notification_id: 'notification-page-20' },
            older_cursor: first ? { received_at: 81, notification_id: 'notification-page-19' } : null,
          };
        };
      },
    });
  });
  await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
  const column = page.getByRole('region', { name: /^Notifications Column,/ });
  await expect(column.locator('.notification-list li')).toHaveCount(20);
  await expect(column.getByText(/20\+ notifications/)).toBeVisible();
  await page.evaluate(() => { (window as typeof window & { __notificationPageReads: number }).__notificationPageReads = 0; });
  await column.getByRole('button', { name: 'Next page' }).click();
  await expect(column.locator('.notification-list li')).toHaveCount(5);
  await column.getByRole('button', { name: 'Previous page' }).click();
  await expect(column.locator('.notification-list li')).toHaveCount(20);
  expect(await page.evaluate(() =>
    (window as typeof window & { __notificationPageReads: number }).__notificationPageReads
  )).toBe(2);
});

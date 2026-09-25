import { expect, test } from '@playwright/test';
import { DESKTOP_THEME_STORAGE_KEY } from '../../src/lib/theme';

for (const options of [
  { width: 1400, target: 40, theme: 'dark', reducedMotion: 'no-preference' },
  { width: 1400, target: 10, theme: 'light', reducedMotion: 'reduce' },
  { width: 390, target: 40, theme: 'dark', reducedMotion: 'reduce' },
  { width: 390, target: 10, theme: 'light', reducedMotion: 'no-preference' },
] as const) {
  test(`notification scroll targets its reply at ${options.width}px, post ${options.target}`, async ({ page }) => {
    // Two notification -> thread navigations over a 60-row thread and timeline are bounded but
    // CPU-heavy (~14s on the standard runner); slower CI runners take ~2.1-2.4x as long.
    test.slow();
    await page.setViewportSize({ width: options.width, height: 850 });
    await page.emulateMedia({ reducedMotion: options.reducedMotion });
    await page.addInitScript(({ target, theme, themeKey }) => {
      window.localStorage.setItem(themeKey, theme);
      let api: typeof window.__KUKURI_DESKTOP__;
      Object.defineProperty(window, '__KUKURI_DESKTOP__', {
        configurable: true,
        get: () => api,
        set: (value: NonNullable<typeof window.__KUKURI_DESKTOP__>) => {
          const originalThread = value.listThread.bind(value);
          const originalNotifications = value.listNotificationsPage.bind(value);
          const rows = originalThread('kukuri:topic:general', 'browser-seed-post').then((view) =>
            Array.from({ length: 60 }, (_, index) => ({
              ...view.items[0], object_id: `focus-post-${index + 1}`,
              root_id: 'focus-post-1', reply_to: index === 0 ? null : 'focus-post-1',
              created_at: index + 1, content: `Notification reply ${index + 1}: ${'Reading context. '.repeat(8)}`,
            }))
          );
          value.listThread = async (_topic, _thread, cursor, limit = 30) => {
            const posts = [...await rows].reverse();
            const start = cursor ? posts.findIndex((post) => post.object_id === cursor.object_id) + 1 : 0;
            const items = posts.slice(start, start + limit);
            const last = items.at(-1);
            return { items, next_cursor: start + limit < posts.length && last
              ? { created_at: last.created_at, object_id: last.object_id } : null };
          };
          value.listTimeline = async () => ({ items: await rows, next_cursor: null });
          value.listNotificationsPage = async () => {
            const result = await originalNotifications();
            return { ...result, items: result.items.map((notification) => ({
              ...notification, object_id: `focus-post-${target}`, thread_root_object_id: 'focus-post-1',
              preview_text: 'Jump to notification reply',
            })) };
          };
          api = value;
        },
      });
    }, { ...options, themeKey: DESKTOP_THEME_STORAGE_KEY });
    await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
    // Keep a second, inactive Column at a nonzero position to catch global selectors/scrolling.
    const background = page.locator('[data-column-id]').filter({ has: page.locator('h2', { hasText: /^Timeline$/ }) }).first()
      .locator('.shell-column-body');
    await background.evaluate((element) => { element.scrollTop = 150; });
    const backgroundTop = await background.evaluate((element) => element.scrollTop);
    await page.getByText('Jump to notification reply', { exact: true }).click();
    const thread = page.getByRole('region', { name: /^Thread Column,.*Active,/ });
    const post = thread.locator(`[data-post-object-id="focus-post-${options.target}"]`);
    await expect(post).toBeFocused();
    await expect(post).toHaveClass(/post-card-targeted/);
    await expect.poll(() => post.evaluate((element) => {
      const body = element.closest('.shell-column-body')!;
      const targetRect = element.getBoundingClientRect();
      const bodyRect = body.getBoundingClientRect();
      return targetRect.top >= bodyRect.top - 2 && targetRect.bottom <= bodyRect.bottom + 2;
    })).toBe(true);
    expect(await thread.locator('.shell-column-body').evaluate((element) => element.scrollTop)).toBeGreaterThan(150);
    expect(await background.evaluate((element) => element.scrollTop)).toBe(backgroundTop);
    expect(await page.evaluate(() => window.scrollY)).toBe(0);
    // A second explicit click on the same notification is not suppressed as a refresh.
    await thread.locator('.shell-column-body').evaluate((element) => { element.scrollTop = 0; });
    await page.getByTestId('control-center-trigger').click();
    await page.getByRole('complementary', { name: 'Control Center' })
      .getByRole('button', { name: /^Notifications/ }).click();
    await page.getByText('Jump to notification reply', { exact: true }).click();
    await expect(post).toBeFocused();
    await expect.poll(() => thread.locator('.shell-column-body').evaluate((element) => element.scrollTop))
      .toBeGreaterThan(150);
  });
}

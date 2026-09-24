import { expect, test } from '@playwright/test';

for (const theme of ['dark', 'light'] as const) {
  test(`columns have no top glow and unread keeps theme accent (${theme})`, async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 980 });
    await page.addInitScript(theme => {
      localStorage.setItem('kukuri.desktop.theme', theme);
      localStorage.setItem('kukuri.desktop.locale', 'en');
      let api: typeof window.__KUKURI_DESKTOP__;
      Object.defineProperty(window, '__KUKURI_DESKTOP__', {
        configurable: true, get: () => api,
        set: (value: NonNullable<typeof api>) => {
          value.getNotificationStatus = async () => ({ unread_count: 2 });
          value.markAllNotificationsRead = async () => { throw new Error('Preview unread state'); };
          const list = value.listNotificationsPage.bind(value);
          value.listNotificationsPage = async () => {
            const page = await list();
            return { ...page, items: page.items.map(row => ({ ...row, read_at: null })) };
          };
          api = value;
        },
      });
    }, theme);
    await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
    const borderAccent = theme === 'dark' ? 'rgb(3, 218, 197)' : 'rgb(164, 74, 33)';
    const textAccent = theme === 'dark' ? borderAccent : 'rgb(113, 55, 23)';
    const notifications = page.locator('.shell-column-surface[data-active]');
    const header = notifications.locator('.shell-column-header');
    await expect(header).toHaveCSS('box-shadow', 'none');
    await expect(header).toHaveCSS('border-top-width', '0px');
    await expect(notifications).not.toHaveCSS('box-shadow', /inset/);
    const height = await header.evaluate(element => element.getBoundingClientRect().height);
    await notifications.locator('.shell-column-pin-button').click();
    await expect(header).toHaveCSS('box-shadow', 'none');
    expect(await header.evaluate(element => element.getBoundingClientRect().height)).toBe(height);
    for (const inactive of await page.locator('.shell-column-surface:not([data-active]) .shell-column-header').all()) {
      await expect(inactive).toHaveCSS('box-shadow', 'none');
      await expect(inactive).toHaveCSS('border-top-width', '0px');
    }
    const badge = page.locator('.shell-control-center-trigger-badge');
    await expect(badge).toHaveCSS('color', textAccent);
    const unread = page.locator('.notification-item[data-unread="true"]').first();
    await expect(unread).toHaveCSS('border-top-color', borderAccent);
    await unread.hover();
    await expect(unread).toHaveCSS('border-top-color', borderAccent);
    await unread.focus();
    await expect(unread).toHaveCSS('border-top-color', borderAccent);
    await page.screenshot({ path: `test-results/orange-${theme}.png` });
  });

  test(`primary filled buttons alone use orange (${theme})`, async ({ page }) => {
    await page.addInitScript(theme => {
      localStorage.setItem('kukuri.desktop.theme', theme);
      localStorage.setItem('kukuri.desktop.locale', 'en');
    }, theme);
    await page.goto('/');
    const timeline = page.locator('.shell-column-surface[data-active]');
    await timeline.getByRole('button', { name: 'Post to Public · general' }).click();
    const post = timeline.locator('.shell-column-composer').getByRole('button', { name: 'Post', exact: true });
    // 展開後の投稿ボタンはクリック位置の直下に来るため、hover 色へ遷移する前に
    // pointer を外して通常色を確定させる。
    await page.mouse.move(0, 0);
    await expect(post).toHaveCSS('background-color', 'rgb(215, 125, 69)');
    await expect(post).toHaveCSS('color', 'rgb(32, 22, 14)');
    await timeline.getByPlaceholder('Write a post').fill('Orange button draft');
    await expect(post).toBeEnabled();
    await post.hover();
    await expect(post).toHaveCSS('background-color', 'rgb(200, 111, 56)');
    await expect(timeline.locator('.button-ghost').first()).not.toHaveCSS('background-color', 'rgb(215, 125, 69)');
  });
}

import { expect, test } from '@playwright/test';

const layoutKey = 'kukuri:workspace-layout:v1';
const general = 'kukuri:topic:general';
const dev = 'kukuri:topic:dev';

test('topic-independent Columns omit the topic row while keeping their own context', async ({ page }, testInfo) => {
  await page.goto('/');
  for (const title of ['Profile', 'Notifications', 'Messages', 'Explore']) {
    const header = page.getByRole('region', { name: new RegExp(`^${title} Column,`) })
      .locator('.shell-column-header');
    await expect(header).toBeAttached();
    await expect(header).not.toContainText(/Public|general/);
    if (title !== 'Explore') await expect(header.locator('.shell-column-scope-row')).toHaveCount(0);
  }
  await expect(page.getByRole('combobox', { name: 'Explore Community Node' })).toBeAttached();
  await page.screenshot({ path: testInfo.outputPath('topic-independent.png') });
});

for (const kind of ['timeline', 'publicThreads', 'stream', 'game', 'metaverse']) {
  test(`${kind} list switches topic in place and restores it on reload`, async ({ page }, testInfo) => {
    await page.addInitScript(({ layoutKey, general, kind }) => {
      localStorage.setItem('kukuri.desktop.developer-mode', 'true');
      if (kind === 'publicThreads') {
        Object.defineProperty(window, '__KUKURI_DESKTOP__', {
          configurable: true,
          set(api: typeof window.__KUKURI_DESKTOP__) {
            if (!api) return;
            api.listPublicChannels = async (topic) => ({
              items: [{ channel_id: 'pc-1700000000000-cccccccccccccccc', topic_id: topic,
                title: `Thread in ${topic}`, creator_pubkey: 'c'.repeat(64), created_at: 1, last_post_at: 1, live_now: false }],
              next_cursor: null, unavailable_count: 0,
            });
            Object.defineProperty(window, '__KUKURI_DESKTOP__', { configurable: true, writable: true, value: api });
          },
        });
      }
      if (localStorage.getItem(layoutKey)) return;
      localStorage.setItem(layoutKey, JSON.stringify({
        version: 1,
        activeColumnId: 'topic-list',
        columns: [
          { id: 'topic-list', kind, scope: { topicId: general, channelId: null }, pinned: true, preferredDesktopSpan: 1 },
          { id: 'other', kind: 'timeline', scope: { topicId: general, channelId: null }, pinned: true, preferredDesktopSpan: 1 },
        ],
      }));
    }, { layoutKey, general, kind });
    await page.goto('/');
    const column = page.locator('[data-column-id="topic-list"]');
    const selector = column.locator('.shell-column-header select');
    await expect(selector).toHaveValue(general);
    if (kind === 'publicThreads') await expect(column.getByText(`Thread in ${general}`, { exact: true })).toBeVisible();
    await column.screenshot({ path: testInfo.outputPath('general.png') });
    await selector.selectOption(dev);
    await expect(selector).toHaveValue(dev);
    if (kind === 'publicThreads') {
      await expect(column.getByText(`Thread in ${general}`, { exact: true })).toHaveCount(0);
      await expect(column.getByText(`Thread in ${dev}`, { exact: true })).toBeVisible();
    }
    await expect(column).toHaveAttribute('aria-current', 'true');
    await expect(page).toHaveURL(/topic=kukuri%3Atopic%3Adev/);
    await expect(page.locator('[data-column-id]')).toHaveCount(2);
    await expect(page.locator('[data-column-id="other"] select')).toHaveValue(general);
    await expect.poll(() => page.evaluate(key => {
      const layout = JSON.parse(localStorage.getItem(key)!);
      return layout.columns.find((item: { id: string }) => item.id === 'topic-list').scope.topicId;
    }, layoutKey)).toBe(dev);
    await selector.focus();
    await selector.press('Home');
    await expect(selector).toHaveValue(general);
    await selector.press('ArrowDown');
    await expect(selector).toHaveValue(dev);
    await page.reload();
    await expect(selector).toHaveValue(dev);
    await expect(page.locator('[data-column-id]')).toHaveCount(2);
    await column.screenshot({ path: testInfo.outputPath('dev.png') });
  });
}

test('Japanese topic controls work in a narrow Column without unrelated context', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => localStorage.setItem('kukuri.desktop.locale', 'ja'));
  await page.goto('/');
  const selector = page.getByRole('combobox', { name: 'カラムのトピック' });
  await selector.selectOption(dev);
  await expect(selector).toHaveValue(dev);
  await expect(page).toHaveURL(/topic=kukuri%3Atopic%3Adev/);
  const profile = page.getByRole('region', { name: /^プロフィール / });
  await expect(profile.locator('.shell-column-scope-row')).toHaveCount(0);
  await expect(page.locator('html')).toHaveAttribute('lang', 'ja');
  await page.screenshot({ path: testInfo.outputPath('narrow-ja.png') });
});

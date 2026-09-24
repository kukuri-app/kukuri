import { expect, test } from '@playwright/test';

test('an older 200-row timeline window returns to latest through refresh', async ({ page }) => {
  test.setTimeout(90_000);
  await page.addInitScript(() => {
    let desktopApi = window.__KUKURI_DESKTOP__;
    Object.defineProperty(window, '__KUKURI_DESKTOP__', {
      configurable: true,
      get: () => desktopApi,
      set: (api: typeof desktopApi) => {
        desktopApi = api;
        if (!api) return;
        const original = api.listTimeline.bind(api);
        api.listTimeline = async (topic, cursor, limit, scope) => {
          if (topic !== 'kukuri:topic:general' || scope?.kind !== 'public') {
            return original(topic, cursor, limit, scope);
          }
          const seed = await original(topic, null, 20, scope);
          const base = seed.items[0];
          if (!base) throw new Error('browser fixture lacks a post');
          const start = cursor ? Number(cursor.object_id.replace('window-post-', '')) + 1 : 0;
          const count = Math.min(limit ?? 20, 230 - start);
          const items = Array.from({ length: count }, (_, offset) => {
            const index = start + offset;
            const id = `window-post-${index}`;
            return { ...base, object_id: id, envelope_id: id,
              created_at: 230 - index, content: id };
          });
          const last = start + count - 1;
          return { items, next_cursor: last < 229
            ? { created_at: 230 - last, object_id: `window-post-${last}` } : null,
            unavailable_count: 0 };
        };
      },
    });
  });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  const column = page.getByRole('region', { name: /^Timeline Column,/ });
  const cards = column.locator('.post-list article');
  await expect(cards).toHaveCount(20);
  const scroll = column.locator('.shell-column-body');
  for (let pageIndex = 1; pageIndex <= 10; pageIndex++) {
    const button = column.getByRole('button', { name: 'Load more' });
    if (await button.isVisible()) {
      await button.click();
    } else {
      await scroll.evaluate((body) => { body.scrollTop = 0; });
      await scroll.evaluate((body) => { body.scrollTop = body.scrollHeight; });
    }
    await expect(column.getByText(`window-post-${pageIndex * 20 + 19}`, { exact: true }))
      .toBeVisible();
    await expect(cards).toHaveCount(Math.min(200, (pageIndex + 1) * 20));
  }
  await expect(column.getByText('window-post-219', { exact: true })).toBeVisible();
  await expect(cards).toHaveCount(200);
  await column.getByRole('button', { name: 'Return to latest' }).click();
  await expect(column.getByText('window-post-0', { exact: true })).toBeVisible();
  await expect(column.getByRole('button', { name: 'Return to latest' })).toHaveCount(0);
});

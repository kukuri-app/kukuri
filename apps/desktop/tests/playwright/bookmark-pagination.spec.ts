import { expect, test } from '@playwright/test';

test('bookmark navigation reads one page and keeps previous/next without a total count', async ({ page }) => {
  await page.addInitScript(() => {
    let desktopApi = window.__KUKURI_DESKTOP__;
    Object.defineProperty(window, '__KUKURI_DESKTOP__', {
      configurable: true,
      get: () => desktopApi,
      set: (api: typeof desktopApi) => {
        desktopApi = api;
        if (!api) return;
        api.listBookmarkedPostsPage = async (cursor, before) => {
          const source = await api.listTimeline('kukuri:topic:general', null, 20, { kind: 'public' });
          const base = source.items[0];
          if (!base) throw new Error('browser fixture lacks a post');
          const first = !cursor || before;
          const start = first ? 0 : 20;
          const count = first ? 20 : 5;
          return {
            items: Array.from({ length: count }, (_, index) => {
              const id = `bookmark-page-${start + index}`;
              return { bookmarked_at: 100 - start - index,
                post: { ...base, object_id: id, envelope_id: id, content: id } };
            }),
            newer_cursor: first ? null : { bookmarked_at: 80, source_object_id: 'bookmark-page-20' },
            older_cursor: first ? { bookmarked_at: 81, source_object_id: 'bookmark-page-19' } : null,
          };
        };
      },
    });
  });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&timelineView=bookmarks');
  const column = page.getByRole('region', { name: /^Timeline Column,/ });
  await expect(column.getByText('bookmark-page-0', { exact: true })).toBeVisible();
  await expect(column.locator('article')).toHaveCount(20);
  await expect(column.getByText(/Page 1 of/)).toHaveCount(0);

  await column.getByRole('button', { name: 'Next page' }).click();
  await expect(column.locator('article')).toHaveCount(5);
  await expect(column.getByText('bookmark-page-20', { exact: true })).toBeVisible();
  await column.getByRole('button', { name: 'Previous page' }).click();
  await expect(column.locator('article')).toHaveCount(20);
});

test('removing the last bookmark returns to the previous page with one read', async ({ page }) => {
  await page.addInitScript(() => {
    let desktopApi = window.__KUKURI_DESKTOP__;
    const removedIds = new Set<string>();
    (window as typeof window & { __bookmarkReads: number }).__bookmarkReads = 0;
    Object.defineProperty(window, '__KUKURI_DESKTOP__', {
      configurable: true,
      get: () => desktopApi,
      set: (api: typeof desktopApi) => {
        desktopApi = api;
        if (!api) return;
        api.removeBookmarkedPost = async (id) => { removedIds.add(id); };
        api.listBookmarkedPostsPage = async (cursor, before) => {
          (window as typeof window & { __bookmarkReads: number }).__bookmarkReads++;
          const source = await api.listTimeline('kukuri:topic:general', null, 20, { kind: 'public' });
          const base = source.items[0];
          if (!base) throw new Error('browser fixture lacks a post');
          const first = !cursor || before;
          const ids = (first ? Array.from({ length: 20 }, (_, index) => index) : [20])
            .filter((index) => !removedIds.has(`bookmark-page-${index}`));
          return {
            items: ids.map((index) => {
              const id = `bookmark-page-${index}`;
              return { bookmarked_at: 100 - index,
                post: { ...base, object_id: id, envelope_id: id, content: id } };
            }),
            newer_cursor: first ? null : removedIds.has('bookmark-page-20') ? null :
              { bookmarked_at: 80, source_object_id: 'bookmark-page-20' },
            older_cursor: first && !removedIds.has('bookmark-page-20')
              ? { bookmarked_at: 81, source_object_id: 'bookmark-page-19' } : null,
          };
        };
      },
    });
  });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&timelineView=bookmarks');
  const column = page.getByRole('region', { name: /^Timeline Column,/ });
  await expect(column.locator('article')).toHaveCount(20);
  await column.getByRole('button', { name: 'Next page' }).click();
  await expect(column.locator('article')).toHaveCount(1);
  await column.getByRole('button', { name: 'Remove bookmark' }).click();
  await expect(column.locator('article')).toHaveCount(20);
  await expect(column.getByRole('button', { name: 'Next page' })).toHaveCount(0);
  await column.getByRole('button', { name: 'Remove bookmark' }).first().click();
  await expect(column.locator('article')).toHaveCount(19);
  await expect(column.getByRole('button', { name: 'Next page' })).toHaveCount(0);
  expect(await page.evaluate(() =>
    (window as typeof window & { __bookmarkReads: number }).__bookmarkReads
  )).toBe(4);
});

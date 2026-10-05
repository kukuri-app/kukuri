import { expect, test, type Page } from '@playwright/test';

import { columnIdentityId, type ColumnState } from '../../src/shell/slices/workspace';
import { WORKSPACE_LAYOUT_STORAGE_KEY } from '../../src/shell/workspacePersistence';

// 幅狭(759px以下)の下部ボタン: 投稿ボタンは右寄せ、アバター・Control Center・
// フィードバックの cluster は左寄せで、4つの高さと下辺を揃える。
async function bottomBarGeometry(page: Page) {
  const post = page.locator('.shell-column-primary-action').first();
  await expect(post).toBeVisible();
  await expect(page.getByTestId('tester-feedback-trigger')).toBeVisible();
  return post.evaluate((postButton) => {
    const rect = (selector: string) =>
      document.querySelector(selector)!.getBoundingClientRect();
    const post = postButton.getBoundingClientRect();
    const cluster = rect('.shell-control-cluster');
    const avatar = rect('[data-testid="account-menu-trigger"]');
    const controlCenter = rect('[data-testid="control-center-trigger"]');
    const feedback = rect('[data-testid="tester-feedback-trigger"]');
    const footer = rect('.shell-column-footer');
    return {
      viewportWidth: window.innerWidth,
      post: { left: post.left, right: post.right, bottom: post.bottom, height: post.height },
      cluster: { left: cluster.left, right: cluster.right },
      heights: [avatar.height, controlCenter.height, feedback.height],
      bottoms: [avatar.bottom, controlCenter.bottom, feedback.bottom],
      footerRight: footer.right,
      footerPaddingRight: parseFloat(getComputedStyle(document.querySelector('.shell-column-footer')!).paddingRight),
    };
  });
}

test('narrow bottom bar aligns the cluster left and the post button right', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  const geometry = await bottomBarGeometry(page);

  // 高さは投稿ボタンに合わせる。
  for (const height of geometry.heights) {
    expect(Math.abs(height - geometry.post.height)).toBeLessThanOrEqual(1);
  }
  // 下辺も揃える。
  for (const bottom of geometry.bottoms) {
    expect(Math.abs(bottom - geometry.post.bottom)).toBeLessThanOrEqual(1);
  }
  // cluster は左寄せ、投稿ボタンは右寄せで重ならない。
  expect(geometry.cluster.left).toBeLessThan(geometry.viewportWidth / 4);
  expect(geometry.cluster.right).toBeLessThanOrEqual(geometry.post.left);
  expect(Math.abs(geometry.post.right - (geometry.footerRight - geometry.footerPaddingRight))).toBeLessThanOrEqual(1);
});

test('desktop keeps the cluster at the bottom left', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/');
  const geometry = await bottomBarGeometry(page);
  expect(geometry.cluster.left).toBeLessThan(geometry.viewportWidth / 4);
});

test.describe('seven-column mobile workspace', () => {
  test.use({ hasTouch: true, isMobile: true });

  for (const viewport of [{ width: 412, height: 783 }, { width: 390, height: 844 }]) {
    test(`page indicator leaves primary actions accessible at ${viewport.width}px`, async ({ page }) => {
      await page.setViewportSize(viewport);
      const scope = { topicId: 'kukuri:topic:general', channelId: null };
      const kinds = ['timeline', 'profile', 'explore', 'notifications', 'messages', 'thread', 'conversation'] as const;
      const columns: ColumnState[] = kinds.map((kind) => {
        const entityId = kind === 'thread' ? 'browser-seed-post' : kind === 'conversation' ? 'b'.repeat(64) : undefined;
        return {
          id: columnIdentityId(kind, scope, entityId),
          kind, scope, entityId, pinned: true, preferredDesktopSpan: 1,
        };
      });
      await page.addInitScript(({ key, columns }) => {
        localStorage.setItem(key, JSON.stringify({ version: 1, columns, activeColumnId: columns[0].id }));
        localStorage.setItem('kukuri.desktop.locale', 'en');
        localStorage.setItem('kukuri.desktop.theme', 'dark');
      }, { key: WORKSPACE_LAYOUT_STORAGE_KEY, columns });
      await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
      await expect(page.locator('.shell-column-surface')).toHaveCount(7);
      // Android's expanded browser controls leave body (100vh) 56px taller than
      // the visible Canvas (100dvh). Headless mobile emulation has no browser
      // controls, so reproduce that document scroll range explicitly.
      await page.addStyleTag({ content: 'body { min-height: calc(100vh + 56px); }' });
      const indicator = page.getByRole('navigation', { name: 'Column pages' });

      for (const [kind, placeholder] of [['conversation', 'Write a message'], ['thread', 'Write a reply'], ['timeline', 'Write a post']]) {
        const index = columns.findIndex((column) => column.kind === kind);
        await indicator.getByRole('button', { name: `Go to Column ${index + 1} of 7`, exact: true }).tap();
        const column = page.locator(`[data-column-id="${columns[index].id}"]`);
        await expect(column).toHaveAttribute('data-active', 'true');
        await expect.poll(() => column.evaluate(element => Math.abs(element.getBoundingClientRect().left))).toBeLessThanOrEqual(1);
        await page.evaluate(() => window.scrollTo(0, 56));
        await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(56);
        const action = column.locator('.shell-column-primary-action');
        await expect(action).toBeVisible();
        expect(await action.evaluate(button => {
          const bounds = button.getBoundingClientRect();
          return button.contains(document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2));
        }), `${kind} primary action must receive its center tap`).toBe(true);
        const actionBounds = (await action.boundingBox())!;
        const indicatorBounds = (await indicator.boundingBox())!;
        expect(indicatorBounds.y + indicatorBounds.height).toBeLessThanOrEqual(actionBounds.y);
        for (const button of await indicator.getByRole('button').all()) {
          const bounds = (await button.boundingBox())!;
          expect(bounds.width).toBeGreaterThanOrEqual(44);
          expect(bounds.height).toBeGreaterThanOrEqual(44);
        }
        await action.tap();
        await expect(column.getByPlaceholder(placeholder)).toBeVisible();
        await expect(indicator).toBeHidden();
        await column.locator('.shell-column-composer').getByRole('button', { name: 'Close', exact: true }).tap();
        await expect(action).toBeFocused();
        await expect(indicator).toBeVisible();
      }
    });
  }
});

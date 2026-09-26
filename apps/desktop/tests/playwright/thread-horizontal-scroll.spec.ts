import { expect, test } from '@playwright/test';

// 返信を 1 段ずつ重ねた深いスレッドを返す。Playwright が page へ直列化できるよう自己完結させる。
function installDeepThreadFixture() {
  let api: typeof window.__KUKURI_DESKTOP__;
  Object.defineProperty(window, '__KUKURI_DESKTOP__', {
    configurable: true,
    get: () => api,
    set: (value: NonNullable<typeof window.__KUKURI_DESKTOP__>) => {
      const original = value.listThread.bind(value);
      value.listThread = async (...args) => {
        const result = await original(...args);
        const root = result.items[0];
        if (!root) return result;
        const replies = Array.from({ length: 8 }, (_, index) => ({
          ...root,
          object_id: `deep-reply-${index + 1}`,
          content: `deep reply ${index + 1}`,
          created_at: root.created_at + index + 1,
          root_id: root.object_id,
          reply_to: index === 0 ? root.object_id : `deep-reply-${index}`,
        }));
        return { ...result, items: [root, ...replies] };
      };
      api = value;
    },
  });
}

test('deep thread replies keep their card width and scroll horizontally in the column', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 900 });
  await page.addInitScript(installDeepThreadFixture);
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=browser-seed-post');
  const column = page.locator('.shell-column-surface', { has: page.locator('.thread-tree') });
  const deepest = column.locator('[data-post-id="deep-reply-8"]');
  await expect(deepest).toContainText('deep reply 8');

  const geometry = await column.locator('.shell-column-body').evaluate((body) => {
    const rootFont = parseFloat(getComputedStyle(document.documentElement).fontSize);
    const bodies = Array.from(body.querySelectorAll<HTMLElement>('.thread-tree-body'));
    const widths = bodies.map((element) => element.getBoundingClientRect().width);
    body.scrollLeft = body.scrollWidth;
    const deepestCard = body.querySelector('[data-post-id="deep-reply-8"] .thread-tree-body')!;
    return {
      widths,
      floor: Math.min(body.clientWidth - parseFloat(getComputedStyle(body).paddingLeft) * 2, rootFont * 22),
      overflow: body.scrollWidth - body.clientWidth,
      deepestRight: deepestCard.getBoundingClientRect().right,
      bodyRight: body.getBoundingClientRect().right,
    };
  });
  for (const width of geometry.widths) expect(width).toBeGreaterThanOrEqual(geometry.floor - 1);
  expect(geometry.overflow).toBeGreaterThan(0);
  expect(geometry.deepestRight).toBeLessThanOrEqual(geometry.bodyRight);
});

import { expect, test, type Locator } from '@playwright/test';
import { createRequire } from 'node:module';
import { dirname } from 'node:path';
import { browserDesktopMockSeed } from '../../src/mocks/browserSeed';

const require = createRequire(import.meta.url);
const axePath = require.resolve('axe-core/axe.min.js', { paths: [dirname(require.resolve('@storybook/addon-a11y/package.json'))] });
const TOPIC = 'kukuri:topic:general';
const IMAGES = 6;
const svg = (fill: string) => Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="320" height="320"><rect width="320" height="320" fill="${fill}"/><circle cx="160" cy="160" r="90" fill="#e18955"/></svg>`).toString('base64');

// 他人の画像つきの投稿。タイムラインで表示すると、mock もキャッシュの一覧へ載せる(#1949)。
const hashes = Array.from({ length: IMAGES }, (_, index) => `${index + 1}`.repeat(64));
const seed = { ...browserDesktopMockSeed,
  seedPosts: { [TOPIC]: [...browserDesktopMockSeed.seedPosts![TOPIC], ...hashes.map((hash, index) => ({ ...browserDesktopMockSeed.seedPosts![TOPIC][0],
    object_id: `peer-image-${index}`, envelope_id: `peer-image-envelope-${index}`, root_id: `peer-image-${index}`, content: `peer image ${index}`, created_at: 10 + index,
    attachments: [{ hash, mime: 'image/svg+xml', bytes: 4096, role: 'image_original', status: 'Available' as const }] }))] },
  seedBlobPayloads: Object.fromEntries(hashes.map((hash, index) => [hash, { mime: 'image/svg+xml', bytes_base64: svg(['#19323c', '#2d4a3e', '#3c2d4a', '#4a3c2d', '#2d3c4a', '#4a2d3c'][index]) }])),
};

async function gridColumns(grid: Locator) {
  return grid.evaluate((element) => getComputedStyle(element).gridTemplateColumns.split(' ').length);
}

for (const scenario of [{ width: 1280, theme: 'dark', locale: 'en' }, { width: 390, theme: 'light', locale: 'ja' }] as const) {
  test(`media column lists cached media, deletes it and keeps the shelf at ${scenario.width} ${scenario.theme}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: scenario.width, height: 844 });
    await page.addInitScript(({ theme, locale, seed }) => {
      if (sessionStorage.getItem('media-column-seeded')) return;
      sessionStorage.setItem('media-column-seeded', '1');
      localStorage.setItem('kukuri.desktop.locale', locale);
      localStorage.setItem('kukuri.desktop.theme', theme);
      Object.assign(window, { __KUKURI_PROMO_MOCK_SEED__: seed });
    }, { ...scenario, seed });
    await page.goto('/#/timeline');
    const timeline = page.locator('[data-column-id][aria-current="true"]');
    // 画像は画面に入った投稿だけを取得する。すべての投稿を表示して、キャッシュへ置かせる。
    for (let index = 0; index < IMAGES; index += 1) {
      const card = timeline.locator('.post-card').filter({ hasText: `peer image ${index}` });
      await card.scrollIntoViewIfNeeded();
      await expect(card.locator('img').first()).toBeVisible();
    }

    await page.getByTestId('control-center-trigger').click();
    await page.getByRole('button', { name: scenario.locale === 'en' ? 'Add Media Column' : 'メディア カラムを追加' }).click();
    const column = page.locator('[data-column-id^="column:media:"]');
    await expect(column).toHaveAttribute('aria-current', 'true');
    const tiles = column.locator('.media-shelf-thumbnail');
    await expect(tiles).toHaveCount(IMAGES);
    await expect(tiles.locator('img')).toHaveCount(IMAGES);
    const grid = column.locator('.media-bookmark-grid');
    const perRow = await gridColumns(grid);
    const width = await grid.evaluate((element) => element.clientWidth);
    // 1 行の数は列の幅に合わせる(最小 6rem = 96px、間隔 8px)。
    expect(perRow).toBe(Math.max(1, Math.floor((width + 8) / (96 + 8))));
    expect(await column.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
    await page.addScriptTag({ path: axePath });
    const violations = await column.evaluate(async (element) => {
      const engine = (window as unknown as { axe: { run: (node: Element) => Promise<{ violations: Array<{ id: string }> }> } }).axe;
      return (await engine.run(element)).violations.map(({ id }) => id);
    });
    expect(violations).toEqual([]);
    await testInfo.attach('cache', { body: await column.screenshot({ path: testInfo.outputPath('cache.png') }), contentType: 'image/png' });

    await column.getByRole('button', { name: scenario.locale === 'en' ? 'Delete from cache' : 'キャッシュから削除' }).first().click();
    await expect(tiles).toHaveCount(IMAGES - 1);
    const tabs = column.getByRole('tab');
    await tabs.nth(2).click();
    await expect(tabs.nth(2)).toHaveAttribute('aria-selected', 'true');
    await expect(column.getByText(scenario.locale === 'en' ? 'No posts with images or videos.' : '画像・動画のある投稿はありません。')).toBeVisible();
    await page.reload();
    await expect(page.locator('[data-column-id^="column:media:"]').getByRole('tab').nth(2)).toHaveAttribute('aria-selected', 'true');
  });
}

import { readFileSync } from 'node:fs';
import { expect, test, type Page } from '@playwright/test';

const png = readFileSync(new URL('../fixtures/overlay-transparent.png', import.meta.url)).toString('base64');
const gif = readFileSync(new URL('../fixtures/overlay-animated.gif', import.meta.url)).toString('base64');

async function openImages(page: Page, width: number, theme: string, locale: string, large = false) {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(({ png, gif, theme, locale, large }) => {
      localStorage.setItem('kukuri.desktop.theme', theme);
      localStorage.setItem('kukuri.desktop.locale', locale);
      let api: typeof window.__KUKURI_DESKTOP__;
      Object.defineProperty(window, '__KUKURI_DESKTOP__', {
        configurable: true, get: () => api,
        set: (value: NonNullable<typeof api>) => {
          api = value;
          const original = value.listTimeline.bind(value);
          value.listTimeline = async (...args) => {
            const result = await original(...args);
            const base = result.items[0];
            if (!base) return result;
            return { ...result, items: (large ? Array.from({ length: 5 }, (_, index) => `large-${index}`) : ['png', 'gif']).map((kind) => ({
              ...base, object_id: `overlay-${kind}`, content: large ? `Author ${kind}: ${'A normal size image must leave this post and its author visible. '.repeat(4)}` : kind === 'png' ? 'Transparent PNG' : '',
              attachments: [{ hash: kind, mime: `image/${kind === 'gif' ? 'gif' : 'png'}`, bytes: large ? 200000 : 2048, role: 'image_original', status: 'Available' as const },
                { hash: 'second', mime: 'image/png', bytes: 100, role: 'image_original', status: 'Available' as const }],
              withdrawal: null, live: null, reply_to: null,
            })) };
          };
          if (large) {
            const canvas = document.createElement('canvas'); canvas.width = canvas.height = 1200;
            const context = canvas.getContext('2d')!;
            context.fillStyle = '#ed2020'; context.fillRect(0, 0, 600, 1200);
            png = canvas.toDataURL().split(',')[1];
          }
          value.getBlobMediaPayload = async (_hash, mime) => ({ mime, bytes_base64: mime === 'image/gif' ? gif : png });
        },
      });
    }, { png, gif, theme, locale, large });
    await page.goto('/');
    await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
    await page.evaluate(async () => {
      const id = await window.__KUKURI_DESKTOP__!.createLiveSession('kukuri:topic:general', 'Overlay images', '');
      location.hash = `/live?topic=kukuri%3Atopic%3Ageneral&sessionId=${encodeURIComponent(id)}`;
    });
}

for (const width of [1440, 390]) for (const theme of ['dark', 'light']) for (const locale of ['ja', 'en', 'zh-CN']) {
  test(`transparent PNG and animated GIF ${width} ${theme} ${locale}`, async ({ page }, info) => {
    await openImages(page, width, theme, locale);
    const stage = page.locator('.live-stage');
    const images = stage.locator('.live-overlay-image img');
    await expect(images).toHaveCount(2);
    await expect.poll(() => images.first().evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(48);
    const animated = stage.locator('[data-post-object-id="overlay-gif"] .live-overlay-image');
    const initial = await animated.screenshot();
    await expect.poll(async () => (await animated.screenshot()).equals(initial), { timeout: 3000 }).toBe(false);
    const still = stage.locator('[data-post-object-id="overlay-png"] .live-overlay-image');
    await still.evaluate((element) => {
      element.closest<HTMLElement>('.live-viewer-screen')!.style.background = 'rgb(21, 105, 210)';
    });
    const box = (await still.boundingBox())!;
    const corner = { x: Math.floor(box.x + box.width - 2), y: Math.floor(box.y + 2), width: 1, height: 1 };
    const throughImage = await page.screenshot({ clip: corner });
    await still.evaluate((element) => { (element as HTMLElement).style.visibility = 'hidden'; });
    expect(await page.screenshot({ clip: corner })).toEqual(throughImage);
    await still.evaluate((element) => { (element as HTMLElement).style.visibility = ''; });
    await page.screenshot({ path: info.outputPath('transparent-overlay.png') });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    // 設定変更を描画へ反映するため、投稿レイヤーを閉じて開く。
    const toggle = stage.locator('button[aria-pressed]').first();
    await toggle.click(); await toggle.click();
    await expect(stage.locator('.live-overlay-image img')).toHaveCount(0);
    const frozen = stage.locator('[data-post-object-id="overlay-gif"] .live-overlay-image canvas');
    await expect.poll(() => frozen.evaluate((canvas: HTMLCanvasElement) => canvas.width)).toBe(48);
    const frame = await frozen.screenshot();
    await expect(frozen).toBeVisible();
    await page.waitForTimeout(550);
    expect(await frozen.screenshot()).toEqual(frame);

  });
}

test('five normal-size images preserve every post and author inside a narrow video', async ({ page }, info) => {
  await openImages(page, 390, 'dark', 'ja', true);
  const stage = page.locator('.live-stage');
  const images = stage.locator('.live-overlay-image img');
  await expect(images).toHaveCount(5);
  await expect.poll(() => images.evaluateAll((images) => images.every((image) => (image as HTMLImageElement).naturalWidth === 1200))).toBe(true);
  const bounds = (await stage.locator('.live-viewer-screen').boundingBox())!;
  for (const copy of await stage.locator('.live-stage-post-copy').all()) {
    const box = (await copy.boundingBox())!;
    expect(box.y).toBeGreaterThanOrEqual(bounds.y);
    expect(box.y + box.height).toBeLessThanOrEqual(bounds.y + bounds.height);
  }
  await page.screenshot({ path: info.outputPath('five-large-images.png') });
});

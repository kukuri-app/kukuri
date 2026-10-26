import { readFileSync, writeFileSync } from 'node:fs';
import { expect, test, type Locator } from '@playwright/test';

const png = readFileSync(new URL('../fixtures/overlay-transparent.png', import.meta.url)).toString('base64');
const gif = readFileSync(new URL('../fixtures/overlay-long-frames.gif', import.meta.url)).toString('base64');

async function inkPixel(image: Locator) {
  return image.evaluate((image: HTMLImageElement) => {
    const box = image.getBoundingClientRect();
    const fit = getComputedStyle(image).objectFit;
    const scale = (fit === 'cover' ? Math.max : Math.min)(box.width / image.naturalWidth, box.height / image.naturalHeight);
    const width = fit === 'fill' ? box.width : image.naturalWidth * scale;
    return { x: Math.floor(box.x + (box.width - width) / 2 + width / 4), y: Math.floor(box.y + box.height / 2), width: 1, height: 1 };
  });
}

for (const width of [1440, 390]) for (const theme of ['dark', 'light']) for (const locale of ['ja', 'en', 'zh-CN']) {
  test(`Push an old attachment repeatedly ${width} ${theme} ${locale}`, async ({ page }, info) => {
    if (width === 1440 && theme === 'dark' && locale === 'en') test.setTimeout(100000);
    const copy = JSON.parse(readFileSync(new URL(`../../src/i18n/locales/${locale}/live.json`, import.meta.url), 'utf8'));
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(({ theme, locale }) => {
      localStorage.setItem('kukuri.desktop.theme', theme);
      localStorage.setItem('kukuri.desktop.locale', locale);
    }, { theme, locale });
    await page.goto('/');
    await expect(page.getByTestId('control-center-trigger')).toBeVisible();
    const seeded = await page.evaluate(async ({ png, gif }) => {
      const api = window.__KUKURI_DESKTOP__!;
      const reads: Record<string, number> = {};
      Object.assign(window, { __overlayReads: reads });
      api.getBlobMediaPayload = async (hash, mime) => {
        reads[hash] = (reads[hash] ?? 0) + 1;
        return { mime, bytes_base64: mime === 'image/gif' ? gif : png };
      };
      const topic = 'kukuri:topic:general';
      const thread = await api.createPublicChannel(topic, 'Push thread', '');
      const ref = { kind: 'public_channel' as const, channel_id: thread.channel_id };
      const post = await api.createPost(topic, 'An older post with two attachments', null, [
        { file_name: 'first.png', mime: 'image/png', byte_size: 2048, data_base64: png, role: 'image_original' },
        { file_name: 'second.gif', mime: 'image/gif', byte_size: 2048, data_base64: gif, role: 'image_original' },
      ], ref);
      for (let index = 0; index < 7; index++) await api.createPost(topic, `Newer post ${index}`, null, [], ref);
      const live = await api.createLiveSession(topic, 'Push Live', '', ref);
      location.hash = `/timeline?topic=${encodeURIComponent(topic)}&channel=${encodeURIComponent(thread.channel_id)}`;
      return { topic, channel: thread.channel_id, post, live };
    }, { png, gif });
    await expect(page.locator('.public-thread-live-carousel')).toBeVisible();
    await page.evaluate(({ topic, channel, live }) => {
      location.hash = `/live?topic=${encodeURIComponent(topic)}&channel=${encodeURIComponent(channel)}&sessionId=${encodeURIComponent(live)}`;
    }, seeded);
    const column = page.locator('.shell-column-surface:has(.live-stage)');
    await column.getByRole('button', { name: copy.stage.split, exact: true }).click();
    await column.getByRole('tab', { name: copy.stage.publicThread, exact: true }).click();
    const buttons = column.locator('.live-column-side .media-push-control button');
    await expect(buttons).toHaveCount(2);
    const button = buttons.nth(1);
    await expect(button).toBeEnabled();
    await expect(column.locator(`.live-stage-post[data-post-object-id="${seeded.post}"]`)).toHaveCount(0);
    const source = column.locator('.live-column-side .media-asset-actions img').nth(1);
    await expect.poll(() => source.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(48);
    const sourceUrl = await source.getAttribute('src');
    const before = await page.evaluate(() => ({ ...(window as unknown as { __overlayReads: Record<string, number> }).__overlayReads }));
    for (let index = 0; index < 18; index++) await button.click();
    await button.press('Enter'); await button.press('Space');
    const overlay = column.locator('.live-stage-pushes .live-overlay-image');
    await expect(overlay.locator('.live-overlay-push-count')).toHaveText('x20');
    expect((await overlay.locator('img').getAttribute('src'))?.split('#')[0]).toBe(sourceUrl);
    await expect(button).toBeFocused();
    expect(await page.evaluate(() => ({ ...(window as unknown as { __overlayReads: Record<string, number> }).__overlayReads }))).toEqual(before);
    const imageBox = (await overlay.boundingBox())!;
    const countBox = (await overlay.locator('.live-overlay-push-count').boundingBox())!;
    expect(Math.abs(imageBox.x + imageBox.width - countBox.x - countBox.width)).toBeLessThanOrEqual(1);
    expect(Math.abs(imageBox.y + imageBox.height - countBox.y - countBox.height)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath('push-x20.png') });
    const toggle = column.locator('.live-stage-controls button[aria-pressed]').first();
    await toggle.click();
    await expect(column.locator('.live-stage-pushes')).toHaveCount(0);
    await toggle.click();
    await expect(column.locator('.live-overlay-push-count')).toHaveCount(0);
    await button.click();
    await expect(column.locator('.live-overlay-push-count')).toHaveText('x1');
    if (width === 1440 && theme === 'dark' && locale === 'en') {
      await expect(column.locator('.live-overlay-push-count')).toHaveCount(0, { timeout: 7000 });
      const red = await page.screenshot({ clip: await inkPixel(column.locator('.live-column-side .media-asset-actions img').first()) });
      const pixel = await inkPixel(source);
      await expect.poll(async () => (await page.screenshot({ clip: pixel })).equals(red)).toBe(true);
      await expect.poll(async () => (await page.screenshot({ clip: pixel })).equals(red)).toBe(false);
      await button.evaluate((button: HTMLButtonElement) => button.click());
      await expect(column.locator('.live-overlay-push-count')).toHaveText('x1');
      await expect.poll(() => column.locator('.live-stage-pushes img').evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(48);
      const freshBox = (await column.locator('.live-stage-pushes img').boundingBox())!;
      const fresh = await page.screenshot({ clip: { x: Math.floor(freshBox.x + freshBox.width / 4), y: Math.floor(freshBox.y + freshBox.height / 2), width: 1, height: 1 } });
      await info.attach('first-frame-expected', { body: red, contentType: 'image/png' });
      await info.attach('first-frame-shown', { body: fresh, contentType: 'image/png' });
      writeFileSync(info.outputPath('frame-expected.png'), red);
      writeFileSync(info.outputPath('frame-shown.png'), fresh);
      expect(fresh).toEqual(red);
      expect((await page.screenshot({ clip: pixel })).equals(red)).toBe(false);
      const targets = await page.evaluate(async ({ png, topic, channel }) => {
        const api = window.__KUKURI_DESKTOP__!;
        const targets = [];
        for (let index = 0; index < 7; index++) {
          const object = await api.createPost(topic, `Congestion image ${index}`, null, [{ mime: 'image/png', byte_size: 2048, data_base64: png, role: 'image_original', file_name: `${index}.png` }], { kind: 'public_channel', channel_id: channel });
          const post = (await api.listThread(topic, object, null, 1)).items.find((post) => post.object_id === object)!;
          targets.push({ topic, channel_id: channel, object_id: object, attachment_index: 0, attachment_hash: post.attachments[0].hash });
        }
        return targets;
      }, { png, topic: seeded.topic, channel: seeded.channel });
      for (const target of targets) {
        await page.evaluate((target) => window.__KUKURI_DESKTOP__!.pushLiveOverlay(target), target);
        await expect.poll(() => column.locator(`.live-stage-pushes [data-asset-hash="${target.attachment_hash}"] img`).evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(48);
      }
      const reads = await page.evaluate(() => ({ ...(window as unknown as { __overlayReads: Record<string, number> }).__overlayReads }));
      const measured = await page.evaluate(async (targets) => {
        const stats = { sent: 0, failed: 0, peak: 0, paint: [] as number[] };
        let ticks = 0;
        await new Promise<void>((done) => {
          const timer = setInterval(() => {
            const start = performance.now();
            for (let sender = 0; sender < 24; sender++) {
              const target = targets[stats.sent % targets.length];
              stats.sent++;
              void window.__KUKURI_DESKTOP__!.pushLiveOverlay(target).catch(() => stats.failed++);
            }
            requestAnimationFrame(() => {
              stats.paint.push(performance.now() - start);
              stats.peak = Math.max(stats.peak, document.querySelectorAll('.live-stage-pushes .live-overlay-image').length);
            });
            if (++ticks === 600) { clearInterval(timer); done(); }
          }, 100);
        });
        await new Promise<void>((done) => requestAnimationFrame(() => done()));
        return stats;
      }, targets);
      expect(measured.sent).toBe(14400);
      expect(measured.failed).toBe(0);
      expect(measured.peak).toBeLessThanOrEqual(6);
      expect(measured.paint.sort((a, b) => a - b)[Math.ceil(measured.paint.length * 0.95) - 1]).toBeLessThanOrEqual(100);
      const after = await page.evaluate(() => ({ ...(window as unknown as { __overlayReads: Record<string, number> }).__overlayReads }));
      for (const target of targets) expect(after[target.attachment_hash]).toBe(reads[target.attachment_hash]);
      await expect(column.locator('.live-stage-pushes .live-overlay-image')).toHaveCount(0, { timeout: 7000 });
    }
  });
}

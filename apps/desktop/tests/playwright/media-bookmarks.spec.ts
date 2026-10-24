import { expect, test, type Page } from '@playwright/test';
import { createRequire } from 'node:module';
import { dirname } from 'node:path';
const require = createRequire(import.meta.url);
const axePath = require.resolve('axe-core/axe.min.js', { paths: [dirname(require.resolve('@storybook/addon-a11y/package.json'))] });

async function checkDialog(page: Page) {
  const violations = await page.getByRole('dialog').evaluate(async (dialog) => {
    const engine = (window as unknown as { axe: { run: (node: Element) => Promise<{ violations: Array<{ id: string; description: string }> }> } }).axe;
    return (await engine.run(dialog)).violations.map(({ id, description }) => ({ id, description }));
  });
  expect(violations).toEqual([]);
}

const image = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360"><rect width="640" height="360" fill="#19323c"/><path d="M0 320 180 90l120 170 130-130 210 190v40H0z" fill="#03dac5"/><circle cx="510" cy="80" r="40" fill="#e18955"/></svg>');

for (const scenario of [{ width: 1280, theme: 'dark', locale: 'en' }, { width: 390, theme: 'light', locale: 'ja' }] as const) {
  test(`media bookmarks reuse blobs and retain source at ${scenario.width} ${scenario.theme}`, async ({ page, context }, testInfo) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await page.setViewportSize({ width: scenario.width, height: 844 });
    await page.addInitScript(({ theme, locale }) => {
      localStorage.setItem('kukuri.desktop.locale', locale);
      localStorage.setItem('kukuri.desktop.theme', theme);
      const revoked: string[] = [];
      (window as unknown as { mediaBookmarkRevoked: string[] }).mediaBookmarkRevoked = revoked;
      const release = URL.revokeObjectURL.bind(URL);
      URL.revokeObjectURL = (url) => { revoked.push(url); release(url); };
    }, scenario);
    await page.goto('/#/timeline');
    await page.addScriptTag({ path: axePath });
    await page.evaluate((bytes) => {
      const api = window.__KUKURI_DESKTOP__!;
      const original = api.getBlobMediaPayload.bind(api);
      api.getBlobMediaPayload = async (hash, mime, source) => mime === 'image/svg+xml' ? { mime, bytes_base64: bytes } : original(hash, mime, source);
    }, image.toString('base64'));
    const active = page.locator('[data-column-id][aria-current="true"]');
    await active.locator('.shell-column-primary-action').click();
    const composer = active.locator('.shell-column-composer');
    await composer.locator('textarea').fill('Original media');
    await composer.locator('input[type=file]').setInputFiles({ name: 'source.svg', mimeType: 'image/svg+xml', buffer: image });
    await expect(composer.locator('.draft-attachment-item')).toHaveCount(1);
    await composer.locator('button[type=submit]').click();
    const original = active.locator('.post-card').filter({ hasText: 'Original media' }).first();
    const bookmark = original.locator('.media-bookmark-button');
    await expect(bookmark).toBeVisible();
    await bookmark.click();
    await expect(bookmark).toHaveAttribute('aria-pressed', 'true');
    await original.locator('.media-preview').click({ button: 'right' });
    const metadata = page.getByRole('dialog');
    await expect(metadata.locator('.media-metadata-list')).toBeVisible();
    await checkDialog(page);
    await metadata.locator('.ui-dialog-body > button').click();
    const sourceMetadata = await page.evaluate(async () => JSON.parse(await navigator.clipboard.readText()) as { hash: string; origin: { object_id: string; author_pubkey: string } });
    await testInfo.attach('metadata', { body: await metadata.screenshot({ path: testInfo.outputPath('metadata.png') }), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await active.locator('.shell-column-primary-action').click();
    await composer.locator('textarea').fill('Reused media');
    await composer.getByRole('button', { name: scenario.locale === 'en' ? 'Choose from bookmarks' : 'ブックマークから選択' }).click();
    const picker = page.getByRole('dialog');
    await expect(picker.locator('.media-bookmark-choice')).toHaveCount(1);
    await expect(picker.locator('.media-bookmark-choice img')).toBeVisible();
    await checkDialog(page);
    await testInfo.attach('picker', { body: await picker.screenshot({ path: testInfo.outputPath('picker.png') }), contentType: 'image/png' });
    expect(await picker.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
    await picker.locator('.media-bookmark-choice').click();
    await expect(composer.locator('.draft-attachment-item')).toHaveCount(1);
    await expect(composer.locator('textarea')).toHaveValue('Reused media');
    const draftPreview = await composer.locator('.draft-preview-image').getAttribute('src');
    await composer.locator('button[type=submit]').click();
    await expect.poll(() => page.evaluate((url) => (window as unknown as { mediaBookmarkRevoked: string[] }).mediaBookmarkRevoked.includes(url!), draftPreview)).toBe(true);
    const reused = active.locator('.post-card').filter({ hasText: 'Reused media' }).first();
    await expect(reused.locator('.media-preview')).toBeVisible();
    await reused.locator('.media-asset-actions').focus();
    await page.keyboard.press('Shift+F10');
    await page.getByRole('dialog').locator('.ui-dialog-body > button').click();
    const reusedMetadata = await page.evaluate(async () => JSON.parse(await navigator.clipboard.readText()) as { hash: string; origin: unknown });
    expect(reusedMetadata.hash).toBe(sourceMetadata.hash);
    expect(reusedMetadata.origin).toEqual(sourceMetadata.origin);
    await page.keyboard.press('Escape');
    await reused.locator('.media-image-trigger').click();
    const viewer = page.getByRole('dialog');
    await expect(viewer.locator('.media-bookmark-button')).toHaveAttribute('aria-pressed', 'true');
    await testInfo.attach('viewer', { body: await viewer.screenshot({ path: testInfo.outputPath('viewer.png') }), contentType: 'image/png' });
    await viewer.locator('.media-bookmark-button').click();
    await expect(viewer.locator('.media-bookmark-button')).toHaveAttribute('aria-pressed', 'false');
    await page.keyboard.press('Escape');
    await expect(original.locator('.media-bookmark-button')).toHaveAttribute('aria-pressed', 'false');
  });
}

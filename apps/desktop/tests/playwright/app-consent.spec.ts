import { expect, test } from '@playwright/test';
import { appConsentCalls, seedAppConsent } from './app-consent-fixture';

const copy = {
  en: { title: 'Before you continue', accept: 'Accept and continue', decline: 'Decline',
    reason: 'To continue, check the box to confirm that you are 18 or older.',
    documents: 'Terms of Service and Privacy Policy', error: 'Failed to save your consent. Please try again.' },
  ja: { title: 'ご利用の前に', accept: '同意して続行', decline: '同意しない',
    reason: '続行するには、18歳以上であることをチェックしてください。',
    documents: '利用規約とプライバシーポリシー', error: '同意の保存に失敗しました。もう一度お試しください。' },
  'zh-CN': { title: '继续之前', accept: '同意并继续', decline: '不同意',
    reason: '如需继续，请勾选复选框，确认您已年满18周岁。',
    documents: '服务条款和隐私政策', error: '保存同意失败，请重试。' },
};

test('consent actions are reachable on first paint without scrolling the terms', async ({ page }, testInfo) => {
  await seedAppConsent(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Before you continue' })).toBeVisible();
  const accept = page.getByRole('button', { name: 'Age confirmation required' });
  await page.screenshot({ path: testInfo.outputPath('first-paint.png') });
  await testInfo.attach('consent-first-paint', { path: testInfo.outputPath('first-paint.png'), contentType: 'image/png' });
  const rect = await accept.boundingBox();
  expect(rect).not.toBeNull();
  expect(rect!.y).toBeGreaterThanOrEqual(0);
  expect(rect!.y + rect!.height).toBeLessThanOrEqual(800);
  await expect(accept).toBeDisabled();
  await expect(page.getByText('To continue, check the box to confirm that you are 18 or older.')).toBeVisible();
  expect(await appConsentCalls(page)).toEqual([]);
});

for (const locale of ['en', 'ja', 'zh-CN'] as const) {
  for (const theme of ['dark', 'light']) {
    for (const viewport of [{ width: 1280, height: 800 }, { width: 390, height: 844 }, { width: 760, height: 480 }]) {
      test(`consent layout ${locale} ${theme} ${viewport.width}x${viewport.height}`, async ({ page }) => {
        await seedAppConsent(page, { locale, theme });
        await page.setViewportSize(viewport);
        await page.goto('/');
        const text = copy[locale];
        await expect(page.getByRole('heading', { level: 1 })).toHaveText(text.title);
        const accept = page.locator('.app-consent-actions').getByRole('button').first();
        const checkbox = page.getByRole('checkbox');
        await expect(accept).toBeDisabled();
        await expect(checkbox).toHaveAccessibleDescription(text.reason);
        await expect(accept).toHaveAccessibleDescription(text.reason);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        if (viewport.height > 540) {
          for (const control of [accept, checkbox, page.getByRole('button', { name: text.decline, exact: true })]) {
            const box = await control.boundingBox();
            expect(box!.y).toBeGreaterThanOrEqual(0);
            expect(box!.y + box!.height).toBeLessThanOrEqual(viewport.height);
          }
        }
        const documents = page.getByRole('region', { name: text.documents });
        await documents.focus();
        await page.keyboard.press('Control+End');
        await expect.poll(() => documents.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
        await checkbox.focus();
        await page.keyboard.press('Space');
        await expect(accept).toBeEnabled();
        await expect(page.getByText(text.reason)).toHaveCount(0);
        await page.keyboard.press('Space');
        await expect(accept).toBeDisabled();
        await page.getByRole('button', { name: text.decline, exact: true }).click();
        expect(await appConsentCalls(page)).toEqual([]);
        await page.reload();
        await expect(page.getByRole('checkbox')).not.toBeChecked();
        await expect(accept).toBeDisabled();
      });
    }
  }

  test(`consent pending error and keyboard retry ${locale}`, async ({ page }) => {
    const text = copy[locale];
    await seedAppConsent(page, { locale, failOnce: true });
    await page.goto('/');
    const checkbox = page.getByRole('checkbox');
    await checkbox.check();
    const accept = page.getByRole('button', { name: text.accept, exact: true });
    await accept.focus();
    await page.keyboard.press('Enter');
    await expect(checkbox).toBeDisabled();
    await page.keyboard.press('Enter');
    await expect(page.getByText(text.error)).toBeVisible();
    await expect(checkbox).toBeChecked();
    expect(await appConsentCalls(page)).toHaveLength(1);
    await accept.focus();
    await page.keyboard.press('Space');
    await expect(page.getByTestId('control-center-trigger')).toBeVisible();
    expect(await appConsentCalls(page)).toHaveLength(2);
    expect((await appConsentCalls(page))[1].args).toEqual({
      documents: [{ slug: 'terms', version: 9 }, { slug: 'privacy', version: 9 }],
      language: locale, ageAttested: true,
    });
  });
}

test('renewed documents do not require another current age attestation', async ({ page }) => {
  await seedAppConsent(page, { attestedVersion: 1 });
  await page.goto('/');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(copy.en.title);
  await expect(page.getByRole('checkbox')).toHaveCount(0);
  await expect(page.getByText(copy.en.reason)).toHaveCount(0);
  await page.getByRole('button', { name: copy.en.accept }).click();
  await expect(page.getByTestId('control-center-trigger')).toBeVisible();
  expect((await appConsentCalls(page))[0].args?.ageAttested).toBe(false);
});

test('short windows preserve readable terms after declining and a save error', async ({ page }) => {
  await seedAppConsent(page, { locale: 'ja', failOnce: true });
  await page.setViewportSize({ width: 390, height: 541 });
  await page.goto('/');
  await page.getByRole('button', { name: copy.ja.decline, exact: true }).click();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: copy.ja.accept, exact: true }).click();
  await expect(page.getByText(copy.ja.error)).toBeVisible();
  await page.getByRole('checkbox').uncheck();
  const documents = page.getByRole('region', { name: copy.ja.documents });
  expect(await documents.evaluate((element) => element.clientHeight)).toBeGreaterThanOrEqual(128);
  await documents.focus();
  await page.keyboard.press('Control+End');
  await expect.poll(() => documents.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight)).toBeLessThanOrEqual(2);
  await page.getByRole('button', { name: copy.ja.decline, exact: true }).focus();
  const box = await page.getByRole('button', { name: copy.ja.decline, exact: true }).boundingBox();
  expect(box!.y).toBeGreaterThanOrEqual(0);
  expect(box!.y + box!.height).toBeLessThanOrEqual(541);
});

test('consent reflows at a 200-percent-equivalent viewport with forced colors', async ({ page }) => {
  await seedAppConsent(page, { locale: 'ja' });
  await page.setViewportSize({ width: 640, height: 400 });
  await page.emulateMedia({ forcedColors: 'active', reducedMotion: 'reduce' });
  await page.goto('/');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(copy.ja.title);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('checkbox').focus();
  await page.keyboard.press('Space');
  const accept = page.getByRole('button', { name: copy.ja.accept });
  await page.keyboard.press('Tab');
  await expect(accept).toBeFocused();
  const box = await accept.boundingBox();
  expect(box!.y).toBeGreaterThanOrEqual(0);
  expect(box!.y + box!.height).toBeLessThanOrEqual(400);
});

test.describe('consent touch input', () => {
  test.use({ hasTouch: true, viewport: { width: 390, height: 844 } });
  test('the label and actions are reachable by touch', async ({ page }) => {
    await seedAppConsent(page, { locale: 'ja' });
    await page.goto('/');
    const checkbox = page.getByRole('checkbox');
    await page.getByText('私は18歳以上です。', { exact: true }).tap();
    await expect(checkbox).toBeChecked();
    expect(await appConsentCalls(page)).toEqual([]);
    await page.getByRole('button', { name: copy.ja.accept, exact: true }).tap();
    await expect(page.getByTestId('control-center-trigger')).toBeVisible();
    expect(await appConsentCalls(page)).toHaveLength(1);
  });
});

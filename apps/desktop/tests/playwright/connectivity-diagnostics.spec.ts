import { expect, test } from '@playwright/test';
import { seedConnectivityDiagnostics } from './connectivity-diagnostics-fixture';

const copy = {
  ja: { summary: 'リアルタイム未接続・配送回復中', refresh: '診断を更新', pending: '診断を更新中…', details: '技術的な診断詳細', node: 'コミュニティノード設定を確認', paused: 'リアルタイム受信を停止中', unused: '現在未購読' },
  en: { summary: 'No live connection · recovering delivery', refresh: 'Refresh diagnostics', pending: 'Refreshing diagnostics…', details: 'Technical diagnostic details', node: 'Check community node settings', paused: 'Live reception paused', unused: 'Not currently subscribed' },
  'zh-CN': { summary: '无实时连接 · 正在恢复传递', refresh: '刷新诊断', pending: '正在刷新诊断…', details: '技术诊断详情', node: '检查社区节点设置', paused: '已暂停实时接收', unused: '当前未订阅' },
};
for (const locale of ['ja', 'en', 'zh-CN'] as const) for (const theme of ['dark', 'light']) {
  test(`${locale} ${theme}: diagnostics explain candidates and preserve settings navigation`, async ({ page }) => {
    await seedConnectivityDiagnostics(page, locale, theme);
    await page.goto('/#/timeline?settings=connectivity');
    const drawer = page.getByRole('dialog');
    await expect(drawer.getByText(copy[locale].summary, { exact: true }).first()).toBeVisible();
    await expect(drawer.getByText(copy[locale].paused, { exact: true })).toBeVisible();
    await expect(drawer.getByText(copy[locale].unused, { exact: true })).toBeVisible();
    const raw = drawer.getByText(/topic join pending/).first();
    await expect(raw).toBeHidden();
    await drawer.getByText(copy[locale].details, { exact: true }).first().click();
    // 設定済み・不足の peer の一覧は、topic の詳細を開いたときにページで読む(#1221 R2-D)。
    await drawer.getByText(copy[locale].details, { exact: true }).nth(1).click();
    await expect(drawer.getByText('candidate-peer', { exact: true }).first()).toBeVisible();
    await expect(raw).toBeVisible();
    await drawer.locator('input').first().fill('preserved-ticket');
    await drawer.getByRole('button', { name: copy[locale].node, exact: true }).click();
    await expect(drawer.getByTestId('settings-section-community-node')).toBeFocused();
    await drawer.getByTestId('settings-section-connectivity').click();
    await expect(drawer.locator('input').first()).toHaveValue('preserved-ticket');
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 844 });
      expect(await drawer.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      const overflow = await drawer.locator('.shell-settings-content').evaluate(el => ({
        fits: el.scrollWidth <= el.clientWidth + 1,
        elements: [...el.querySelectorAll('*')].filter(node => node.getBoundingClientRect().right > el.getBoundingClientRect().right + 1)
          .map(node => ({ tag: node.tagName, text: node.textContent?.slice(0, 90), width: node.getBoundingClientRect().width })),
      }));
      expect(overflow.fits, JSON.stringify(overflow.elements)).toBe(true);
    }
    expect(await page.evaluate(() => (window as unknown as { diagnosticsTest: { mutations: string[] } }).diagnosticsTest.mutations)).toEqual([]);
  });
}

test('diagnostic refresh has pending, failure, retry and recovery without reconnecting', async ({ page }) => {
  await seedConnectivityDiagnostics(page, 'en');
  await page.goto('/#/timeline?settings=connectivity');
  const drawer = page.getByRole('dialog');
  await expect(drawer.getByRole('button', { name: 'Refresh diagnostics', exact: true })).toBeEnabled();
  await page.evaluate(() => Object.assign((window as unknown as { diagnosticsTest: object }).diagnosticsTest, { fail: true, delay: 600 }));
  const refresh = drawer.getByRole('button', { name: 'Refresh diagnostics', exact: true });
  await refresh.focus();
  await refresh.press('Enter');
  await expect(drawer.getByRole('button', { name: 'Refreshing diagnostics…', exact: true })).toBeDisabled();
  await expect(drawer.getByText(/Showing the previous snapshot/).first()).toBeVisible();
  await page.evaluate(() => Object.assign((window as unknown as { diagnosticsTest: object }).diagnosticsTest, { fail: false, live: true }));
  await refresh.click();
  await expect(drawer.getByText('Connected · Direct P2P', { exact: true }).first()).toBeVisible();
  await expect(drawer.getByText(/Showing the previous snapshot/).first()).toBeHidden();
  await drawer.locator('.shell-settings-close').click();
  await expect(page.getByTestId('control-center-trigger')).toBeFocused();
  await page.getByTestId('control-center-trigger').click();
  await expect(page.getByRole('button', { name: /Connected · Direct P2P/ })).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { diagnosticsTest: { mutations: string[] } }).diagnosticsTest.mutations)).toEqual([]);
});

test('ordinary mode offers recovery without exposing raw diagnostic details', async ({ page }) => {
  await seedConnectivityDiagnostics(page, 'ja', 'dark', false);
  await page.goto('/#/timeline?settings=connectivity');
  const drawer = page.getByRole('dialog');
  await expect(drawer.getByRole('button', { name: copy.ja.refresh, exact: true })).toBeVisible();
  await expect(drawer.getByText(copy.ja.details, { exact: true })).toHaveCount(0);
  await expect(drawer.getByText(/topic join pending/)).toHaveCount(0);
  await drawer.evaluate(el => { (el as HTMLElement).style.zoom = '2'; });
  expect(await drawer.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
});

test('Control Center topic summaries distinguish missing diagnostics and paused reception', async ({ page }) => {
  await seedConnectivityDiagnostics(page, 'en');
  await page.goto('/#/timeline');
  await page.getByTestId('control-center-trigger').click();
  const center = page.getByRole('complementary');
  const dev = center.getByRole('button', { name: 'dev', exact: true }).locator('..');
  const paused = center.getByRole('button', { name: 'test', exact: true }).locator('..');
  await expect(dev.locator('.topic-diagnostic')).toHaveText('Not currently subscribed');
  await expect(paused.locator('.topic-diagnostic')).toHaveText('Live reception paused');
  await expect(dev.locator('.topic-diagnostic')).not.toContainText('peers: 0');
});

test('an initial read failure remains unknown in settings and Control Center until a successful refresh', async ({ page }) => {
  await seedConnectivityDiagnostics(page, 'en', 'dark', true, true);
  await page.goto('/#/timeline?settings=connectivity');
  const drawer = page.getByRole('dialog');
  await expect(drawer.getByText('Diagnostics could not be loaded. Try refreshing again.').first()).toBeVisible();
  await expect(drawer.getByText('Diagnostics not loaded').first()).toBeVisible();
  await expect(drawer.getByText('Peers', { exact: true })).toHaveCount(0);
  await drawer.locator('.shell-settings-close').click();
  await page.getByTestId('control-center-trigger').click();
  const center = page.getByRole('complementary');
  const dev = center.getByRole('button', { name: 'dev', exact: true }).locator('..');
  await expect(dev.locator('.topic-diagnostic')).toHaveText('Diagnostics not loaded');
  await center.getByRole('button', { name: /Diagnostics not loaded Connectivity/ }).click();
  await page.evaluate(() => Object.assign((window as unknown as { diagnosticsTest: object }).diagnosticsTest, { fail: false }));
  await drawer.getByRole('button', { name: 'Refresh diagnostics', exact: true }).click();
  await expect(drawer.getByText('No live connection · recovering delivery', { exact: true }).first()).toBeVisible();
});

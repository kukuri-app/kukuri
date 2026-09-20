import { expect, test, type Page } from '@playwright/test';

const copy = {
  en: { release: 'Release and updates', check: 'Check', checking: 'Checking', latest: 'Up to date',
    failed: 'Could not connect to the update server.', checkedAt: 'Checked at' },
  ja: { release: 'リリースと更新', check: '確認', checking: '確認中', latest: '最新です',
    failed: '更新サーバーに接続できませんでした。', checkedAt: 'に確認しました' },
  'zh-CN': { release: '版本与更新', check: '检查', checking: '正在检查', latest: '已是最新版本',
    failed: '无法连接到更新服务器。', checkedAt: '已于' },
};

// #956 Reopen: 結果が前回と同じでも、この確認の完了時刻（時:分:秒）で区別できる（AC-6）。
const CLOCK_START = Date.UTC(2026, 8, 12, 3, 4, 0);

async function advanceClock(page: Page, minutes: number): Promise<string> {
  const at = CLOCK_START + minutes * 60_000;
  await page.clock.setFixedTime(at);
  return page.evaluate((at) => new Intl.DateTimeFormat(
    document.documentElement.lang, { hour: 'numeric', minute: '2-digit', second: '2-digit' }
  ).format(at), at);
}

async function seedUpdateCheck(page: Page, locale: keyof typeof copy, theme: string) {
  await page.addInitScript(({ locale, theme }) => {
    localStorage.setItem('kukuri.desktop.locale', locale);
    localStorage.setItem('kukuri.desktop.theme', theme);
    localStorage.setItem('kukuri.desktop.developer-mode', 'false');
    const fixture = {
      armed: false, pending: false, checks: 0, forbidden: [] as string[],
      resolve: (() => {}) as (result: { id: number; version: string } | null) => void,
      reject: (() => {}) as (reason: string) => void,
    };
    Object.defineProperty(window, '__updateFeedback', { value: fixture });
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: {
        transformCallback: () => 1,
        unregisterCallback: () => {},
        invoke: async (command: string) => {
          if (command === 'plugin:app|version') return '0.2.1';
          if (command === 'check_app_update') {
            // Let the existing startup check complete before the manual action.
            if (!fixture.armed) return null;
            fixture.checks += 1;
            return new Promise((resolve, reject) => {
              fixture.resolve = resolve;
              fixture.reject = reject;
              fixture.pending = true;
            });
          }
          if (['download_app_update', 'install_app_update', 'restart_after_update'].includes(command)) {
            fixture.forbidden.push(command);
            throw new Error(`Unexpected update side effect: ${command}`);
          }
          if (command.startsWith('plugin:event|')) return 1;
          throw new Error(`Unexpected fixture command: ${command}`);
        },
      },
    });
    Object.defineProperty(window, '__TAURI_EVENT_PLUGIN_INTERNALS__', {
      value: { unregisterListener: () => {} },
    });
  }, { locale, theme });
}

type UpdateFixture = {
  armed: boolean; pending: boolean; checks: number; forbidden: string[];
  resolve: (result: { id: number; version: string } | null) => void;
  reject: (reason: string) => void;
};

async function settleCheck(page: Page, outcome: 'latest' | 'available' | 'failed') {
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __updateFeedback: UpdateFixture }).__updateFeedback.pending
  )).toBe(true);
  await page.evaluate((outcome) => {
    const fixture = (window as unknown as { __updateFeedback: UpdateFixture }).__updateFeedback;
    fixture.pending = false;
    if (outcome === 'failed') fixture.reject('network unavailable: fixture.internal');
    else fixture.resolve(outcome === 'latest' ? null : { id: 1, version: '0.2.2-preview.1' });
  }, outcome);
}

for (const locale of ['en', 'ja', 'zh-CN'] as const) {
  for (const theme of ['dark', 'light']) {
    for (const viewport of [{ width: 1280, height: 800 }, { width: 390, height: 844 }, { width: 640, height: 400 }]) {
      test(`update feedback ${locale} ${theme} ${viewport.width}`, async ({ page }, testInfo) => {
        await seedUpdateCheck(page, locale, theme);
        await page.setViewportSize(viewport);
        await page.clock.setFixedTime(CLOCK_START);
        await page.goto('/#/timeline');
        const text = copy[locale];
        await page.getByTestId('control-center-trigger').click();
        await page.locator('.shell-control-center').getByRole('button', { name: text.release, exact: true }).click();
        const dialog = page.getByRole('dialog');
        const status = dialog.getByRole('status');
        const startupTime = await advanceClock(page, 0);
        await expect(status).toContainText(text.latest);
        await expect(status).toContainText(text.checkedAt);
        await expect(status).toContainText(startupTime);
        // A populated live region can still be clipped by a zero-height drawer body.
        await expect(status).toBeInViewport();
        await page.evaluate(() => {
          (window as unknown as { __updateFeedback: UpdateFixture }).__updateFeedback.armed = true;
        });
        const check = dialog.getByRole('button', { name: text.check, exact: true });
        await check.focus();
        await page.keyboard.press('Enter');
        await expect(status).toHaveText(text.checking);
        await expect(dialog.getByRole('button', { name: text.checking, exact: true })).toBeDisabled();
        await expect(dialog.getByRole('button', { name: text.checking, exact: true })).toHaveAttribute('aria-busy', 'true');
        const failedTime = await advanceClock(page, 5);
        await settleCheck(page, 'failed');
        await expect(dialog.getByRole('alert')).toContainText(text.failed);
        await expect(dialog.getByRole('alert')).toContainText(failedTime);
        await expect(dialog).not.toContainText('fixture.internal');
        // The button stays acknowledged for one second after a fast completion (AC-7).
        await expect(dialog.getByRole('button', { name: text.check, exact: true })).toBeEnabled();
        await check.click();
        await expect(status).toHaveText(text.checking);
        await expect(dialog.getByRole('alert')).toHaveCount(0);
        const latestTime = await advanceClock(page, 10);
        await settleCheck(page, 'latest');
        await expect(status).toContainText(text.latest);
        await expect(status).toContainText(latestTime);
        await expect(status).not.toContainText(startupTime);
        await page.keyboard.press('Escape');
        await expect(dialog).toHaveCount(0);
        await page.getByTestId('control-center-trigger').click();
        await page.locator('.shell-control-center').getByRole('button', { name: text.release, exact: true }).click();
        await expect(status).toContainText(text.latest);
        await expect(status).toContainText(latestTime);
        await check.click();
        await expect(status).toHaveText(text.checking);
        const availableTime = await advanceClock(page, 15);
        await settleCheck(page, 'available');
        await expect(status).toContainText('0.2.2-preview.1');
        await expect(status).toContainText(availableTime);
        const bounds = await status.boundingBox();
        expect(bounds).not.toBeNull();
        expect(bounds!.x).toBeGreaterThanOrEqual(0);
        expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(viewport.width);
        expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        await testInfo.attach('update-result', { body: await dialog.screenshot(), contentType: 'image/png' });
        expect(await page.evaluate(() => {
          const fixture = (window as unknown as { __updateFeedback: UpdateFixture }).__updateFeedback;
          return { checks: fixture.checks, forbidden: fixture.forbidden };
        })).toEqual({ checks: 3, forbidden: [] });
      });
    }
  }
}

// #1189: 更新があることは Control Center を開かないと分からなかった。更新がある間だけ、
// 「フィードバックを送る」の右に強調表示の導線を出し、そこから設定の「リリースと更新」へ行く。
const updateTriggerLabel = {
  en: 'Update available',
  ja: 'アップデートがあります',
  'zh-CN': '有可用更新',
} satisfies Record<keyof typeof copy, string>;

async function reachUpdateAvailable(page: Page, locale: keyof typeof copy) {
  const text = copy[locale];
  await page.goto('/#/timeline');
  // 更新前は cluster に導線が無い（AC-3）。
  await expect(page.getByTestId('tester-feedback-trigger')).toBeVisible();
  await expect(page.getByTestId('update-available-trigger')).toHaveCount(0);

  await page.getByTestId('control-center-trigger').click();
  await page
    .locator('.shell-control-center')
    .getByRole('button', { name: text.release, exact: true })
    .click();
  const dialog = page.getByRole('dialog');
  // 起動時の確認が終わってから手動確認を armed にする。
  await expect(dialog.getByRole('status')).toContainText(text.latest);
  await page.evaluate(() => {
    (window as unknown as { __updateFeedback: UpdateFixture }).__updateFeedback.armed = true;
  });
  await dialog.getByRole('button', { name: text.check, exact: true }).click();
  await settleCheck(page, 'available');
  await expect(dialog.getByRole('status')).toContainText('0.2.2-preview.1');
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
}

for (const locale of ['en', 'ja', 'zh-CN'] as const) {
  test(`update trigger reaches release settings from the control cluster ${locale}`, async ({ page }) => {
    await seedUpdateCheck(page, locale, 'dark');
    await page.setViewportSize({ width: 390, height: 844 });
    await reachUpdateAvailable(page, locale);

    const update = page.getByTestId('update-available-trigger');
    await expect(update).toBeVisible();
    // 幅狭ではアイコンのみになるが、読み上げ名は保つ（AC-4）。
    await expect(update).toHaveAccessibleName(updateTriggerLabel[locale]);
    await expect(update.locator('span')).toBeHidden();

    // 投稿ボタン・他の cluster ボタンと高さと下辺が揃い、重ならない（AC-5）。
    const geometry = await page.locator('.shell-column-primary-action').first().evaluate((postButton) => {
      const rect = (selector: string) => document.querySelector(selector)!.getBoundingClientRect();
      const post = postButton.getBoundingClientRect();
      return {
        post: { left: post.left, bottom: post.bottom, height: post.height },
        cluster: rect('.shell-control-cluster'),
        update: rect('[data-testid="update-available-trigger"]'),
      };
    });
    expect(Math.abs(geometry.update.height - geometry.post.height)).toBeLessThanOrEqual(1);
    expect(Math.abs(geometry.update.bottom - geometry.post.bottom)).toBeLessThanOrEqual(1);
    expect(geometry.cluster.right).toBeLessThanOrEqual(geometry.post.left);
    expect(geometry.cluster.left).toBeGreaterThanOrEqual(0);

    // 押すと設定の「リリースと更新」が開く（AC-2）。更新の実行は発火しない（INVAR-2）。
    await update.click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByRole('status')).toContainText('0.2.2-preview.1');
    await expect(page.getByTestId('settings-section-release')).toHaveAttribute('aria-current', 'location');
    expect(await page.evaluate(() =>
      (window as unknown as { __updateFeedback: UpdateFixture }).__updateFeedback.forbidden
    )).toEqual([]);
  });
}

test('update trigger shows its label on desktop and hides with the cluster while composing', async ({ page }) => {
  await seedUpdateCheck(page, 'en', 'dark');
  await page.setViewportSize({ width: 1280, height: 800 });
  await reachUpdateAvailable(page, 'en');

  const update = page.getByTestId('update-available-trigger');
  await expect(update.locator('span')).toHaveText(updateTriggerLabel.en);
  // Control Center を開くと cluster ごと隠れる（INVAR-3）。
  await page.getByTestId('control-center-trigger').click();
  await expect(update).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(update).toBeVisible();

  // 幅狭で Composer に入力している間は、他の cluster ボタンと同じく隠れる（TR-6）。
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator('.shell-column-primary-action').first().click();
  await page.getByPlaceholder('Write a post').focus();
  await expect(update).toBeHidden();
  await expect(page.getByTestId('tester-feedback-trigger')).toBeHidden();
});

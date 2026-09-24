import { expect, test, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

// #962: 通知の受信設定と、開発者向けのログ所在の明示。
// 通知カラム／空状態 → 設定 > 通知、開発者 → 診断レポートの実 pointer / keyboard 到達を確認する。
const DESKTOP_LOCALE_STORAGE_KEY = 'kukuri.desktop.locale';
const DESKTOP_THEME_STORAGE_KEY = 'kukuri.desktop.theme';

const COPY = {
  ja: {
    settings: '設定',
    headerAction: '通知の受信設定',
    empty: '通知はまだありません。',
    enable: 'OS 通知を有効にする',
    quiet: '静音モード',
    report: '診断レポートを開く',
    logs: 'ログ',
    developerToggle: '開発者モードを有効にする',
    copyReport: 'レポートをコピー',
    columnTitle: '通知',
  },
  en: {
    settings: 'Settings',
    headerAction: 'Notification settings',
    empty: 'No notifications yet.',
    enable: 'Enable OS notifications',
    quiet: 'Quiet mode',
    report: 'Diagnostic report',
    logs: 'Logs',
    developerToggle: 'Enable developer mode',
    copyReport: 'Copy Report',
    columnTitle: 'Notifications',
  },
} as const;

async function seed(page: Page, locale: keyof typeof COPY, theme: 'dark' | 'light', emptyInbox: boolean) {
  await page.addInitScript(
    ({ locale, theme, emptyInbox, localeKey, themeKey, developerKey }) => {
      window.localStorage.setItem(localeKey, locale);
      window.localStorage.setItem(themeKey, theme);
      window.localStorage.setItem(developerKey, 'false');
      if (!emptyInbox) return;
      let api: typeof window.__KUKURI_DESKTOP__;
      Object.defineProperty(window, '__KUKURI_DESKTOP__', {
        configurable: true,
        get: () => api,
        set: (value: NonNullable<typeof window.__KUKURI_DESKTOP__>) => {
          value.listNotificationsPage = async () => ({ items: [], newer_cursor: null, older_cursor: null });
          api = value;
        },
      });
    },
    {
      locale,
      theme,
      emptyInbox,
      localeKey: DESKTOP_LOCALE_STORAGE_KEY,
      themeKey: DESKTOP_THEME_STORAGE_KEY,
      developerKey: DEVELOPER_MODE_STORAGE_KEY,
    }
  );
}

for (const { locale, theme, width, height } of [
  { locale: 'ja', theme: 'dark', width: 1280, height: 800 },
  { locale: 'en', theme: 'light', width: 390, height: 844 },
] as const) {
  const copy = COPY[locale];

  test(`${locale} ${theme} notifications inbox action opens the notification settings`, async ({ page }) => {
    await seed(page, locale, theme, false);
    await page.setViewportSize({ width, height });
    await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
    await expect(page.getByText('browser mock reply notification')).toBeVisible();
    const action = page.getByRole('button', { name: copy.headerAction, exact: true });
    await expect(action).toBeVisible();
    // 導線は本文側に置き、Column header の title / 要約 / 更新は変えない。
    await expect(page.locator('.shell-column-header').filter({ hasText: copy.columnTitle }).first()).toBeVisible();
    await action.focus();
    await page.keyboard.press('Enter');
    const settings = page.getByRole('dialog', { name: copy.settings, exact: true });
    await expect(settings).toBeVisible();
    await expect(settings.getByTestId('settings-section-notifications')).toHaveAttribute(
      'aria-current',
      'location'
    );
    await expect(page).toHaveURL(/settings=notifications/);
    const quiet = settings.getByRole('checkbox', { name: copy.quiet, exact: true });
    await expect(quiet).not.toBeChecked();
    await quiet.check();
    await expect(quiet).toBeChecked();
    expect(
      await page.evaluate(() => JSON.parse(localStorage.getItem('kukuri:os-notification-settings:v1') ?? '{}'))
    ).toMatchObject({ quietMode: true, enabled: false });
    for (const viewport of [1280, 700, 390]) {
      await page.setViewportSize({ width: viewport, height });
      const content = settings.locator('.shell-settings-content');
      expect(await content.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    }
    await page.setViewportSize({ width, height });
    await page.keyboard.press('Escape');
    await expect(settings).toBeHidden();
    await expect(page).not.toHaveURL(/settings=/);
    await expect(page.getByText('browser mock reply notification')).toBeVisible();
  });

  test(`${locale} ${theme} empty inbox offers the notification settings as its next action`, async ({ page }) => {
    await seed(page, locale, theme, true);
    await page.setViewportSize({ width, height });
    await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
    await expect(page.getByText(copy.empty, { exact: true })).toBeVisible();
    await page.getByRole('button', { name: copy.headerAction, exact: true }).click();
    const settings = page.getByRole('dialog', { name: copy.settings, exact: true });
    await expect(settings.getByRole('checkbox', { name: copy.enable, exact: true })).toBeVisible();
    await settings.locator('.shell-settings-close').click();
    await expect(settings).toBeHidden();
    await expect(page.getByText(copy.empty, { exact: true })).toBeVisible();
  });

  test(`${locale} ${theme} developer settings expose the diagnostic report and log guidance`, async ({ page }) => {
    await seed(page, locale, theme, false);
    await page.setViewportSize({ width, height });
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=developer');
    const settings = page.getByRole('dialog', { name: copy.settings, exact: true });
    await expect(settings.getByRole('heading', { name: copy.logs, exact: true })).toBeHidden();
    await settings.getByRole('checkbox', { name: copy.developerToggle, exact: true }).check();
    await expect(settings.getByRole('heading', { name: copy.logs, exact: true })).toBeVisible();
    await expect(settings.getByText(/RUST_LOG/)).toBeVisible();
    const content = settings.locator('.shell-settings-content');
    expect(await content.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await settings.getByRole('button', { name: copy.report, exact: true }).click();
    await expect(settings.getByTestId('settings-section-release')).toHaveAttribute('aria-current', 'location');
    await expect(page).toHaveURL(/settings=release/);
    await expect(settings.getByRole('button', { name: copy.copyReport, exact: true })).toBeVisible();
  });
}

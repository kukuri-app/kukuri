import { seedConnectivityDiagnostics } from './connectivity-diagnostics-fixture';
import { installReplyLayoutFixture } from './reply-layout-fixture';
import { seedProfileConnections } from './profile-connections-fixture';
import { expect, test, type Locator, type Page } from '@playwright/test';
import enProfile from '../../src/i18n/locales/en/profile.json' with { type: 'json' };
import jaProfile from '../../src/i18n/locales/ja/profile.json' with { type: 'json' };
import zhProfile from '../../src/i18n/locales/zh-CN/profile.json' with { type: 'json' };
import { seedUnconsentedCommunityNodes } from './community-node-fixture';
import { seedAppConsent } from './app-consent-fixture';
import { seedFeedback } from './tester-feedback-fixture';
import { expectIndexContentContained, seedIndexLayout } from './community-index-layout-fixture';
import {
  TIMELINE_ADVISORY_OBJECT_ID,
  TIMELINE_ADVISORY_URL,
  seedTimelineAdvisory,
} from './timeline-advisory-fixture';
import { runExploreSearch, seedExploreMedia } from './community-index-media-fixture';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

for (const [width, theme] of [[1440, 'dark'], [390, 'light']] as const) {
  test(`reply parent layout ${width} ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(({ theme }) => {
      localStorage.setItem('kukuri.desktop.locale', 'ja');
      localStorage.setItem('kukuri.desktop.theme', theme);
    }, { theme });
    await page.addInitScript(installReplyLayoutFixture);
    await page.goto('/');
    const group = page.locator('.post-reply-group').first();
    await expect(group).toContainText('CliPeerA');
    await expect(group).toHaveScreenshot(`reply-parent-${width}-${theme}.png`);
  });
}
import { DESKTOP_THEME_STORAGE_KEY, type DesktopTheme } from '../../src/lib/theme';

// src/i18n/index.ts の DESKTOP_LOCALE_STORAGE_KEY と一致（実読で確認）。
// i18n/index.ts を import すると全 locale JSON ロード + react-i18next.init の副作用が
// テスト評価時に走るため、副作用のない軽量な theme.ts だけを import し、locale キーは
// リテラルで持つ。
const DESKTOP_LOCALE_STORAGE_KEY = 'kukuri.desktop.locale';

// 視覚回帰スモーク（WP-S7）。到達操作は既存 E2E（shell.smoke / hash-routing /
// extended-flow）の deep link + testid パターンを再利用する。付録 B の安定化前処理を
// 適用し、決定的な名前で toHaveScreenshot を撮る。
//
// Windows ローカルでは playwright.config の ignoreSnapshots: !process.env.CI により
// 比較が skip され、本 spec は「到達操作の smoke」として流れる（baseline PNG は生成しない）。
// baseline は Linux CI（CI=1）でのみ --update-snapshots で生成する。

const WIDE = { width: 1400, height: 980 } as const;
const NARROW = { width: 700, height: 980 } as const;

const LOCALE_EN = 'en';

for (const { locale, theme, width, menuLabel, addLabel, createLabel } of [
  { locale: 'ja', theme: 'dark', width: 1280, menuLabel: 'アカウントメニュー', addLabel: 'アカウント追加', createLabel: '新しいアカウントを作成' },
  { locale: 'en', theme: 'light', width: 390, menuLabel: 'Account menu', addLabel: 'Add account', createLabel: 'Create a new account' },
] as const) {
  test(`account menu and creation ${locale} ${theme}`, async ({ page }) => {
    await page.addInitScript(({ locale, theme }) => {
      localStorage.setItem('kukuri.desktop.locale', locale);
      localStorage.setItem('kukuri.desktop.theme', theme);
    }, { locale, theme });
    await page.setViewportSize({ width, height: 844 });
    await page.goto('/');
    await page.getByTestId('account-menu-trigger').click();
    const menu = page.getByRole('menu', { name: menuLabel });
    await expect(menu.getByRole('menuitemradio')).toBeVisible();
    await expect(menu).toHaveScreenshot(`account-menu-${locale}-${theme}.png`);
    await menu.getByRole('menuitem', { name: addLabel }).click();
    const dialog = page.getByRole('dialog', { name: addLabel });
    await expect(dialog.getByRole('button', { name: createLabel })).toBeEnabled();
    await expect(dialog).toHaveScreenshot(`account-add-${locale}-${theme}.png`);
  });
}

for (const { width, locale, theme } of [
  { width: 1280, locale: 'ja', theme: 'dark' },
  { width: 390, locale: 'en', theme: 'light' },
  { width: 1024, locale: 'zh-CN', theme: 'dark' },
] as const) {
  test(`compact profile connections ${locale} ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 840 });
    await seedProfileConnections(page, locale, theme);
    await page.goto('/#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=connections&connectionsView=blocking');
    const profile = page.locator('.shell-column-surface').filter({ has: page.getByTestId('profile-connection-identifier-target') });
    await expect(profile.getByTestId('profile-connection-identifier-target')).toBeVisible();
    await settleForShot(page, theme);
    // Rows may be visible while the minimum loading notice is still displayed.
    const messages = { en: enProfile, ja: jaProfile, 'zh-CN': zhProfile };
    await expect(profile.getByText(messages[locale].connections.loading, { exact: true })).toHaveCount(0);
    await expect(profile).toHaveScreenshot(`profile-connections-${locale}-${theme}.png`);
  });
}

for (const { locale, theme, width } of [
  { locale: 'ja', theme: 'dark', width: 1280 },
  { locale: 'en', theme: 'light', width: 390 },
  { locale: 'zh-CN', theme: 'light', width: 1024 },
] as const) {
  test(`profile overview ${locale} ${theme}`, async ({ page }) => {
    await page.addInitScript(({ locale, theme }) => {
      localStorage.setItem('kukuri.desktop.locale', locale);
      localStorage.setItem('kukuri.desktop.theme', theme);
    }, { locale, theme });
    await page.setViewportSize({ width, height: 800 });
    await page.goto('/#/profile?topic=kukuri%3Atopic%3Ageneral');
    const profile = page.locator('.shell-column-surface').filter({ has: page.locator('.profile-overview-header') });
    const refresh = profile.locator('.shell-column-context-actions button[aria-busy]');
    await expect(refresh).toHaveAttribute('aria-busy', 'false');
    await page.evaluate(() => window.__KUKURI_DESKTOP__!.setMyProfile({
      display_name: '表示名テスト', name: 'kingyosun', about: 'プロフィールの更新と表示を確認します。',
    }));
    await refresh.click();
    await expect(profile.locator('.profile-overview-names h3')).toHaveText('表示名テスト');
    await expect(refresh).toHaveAttribute('aria-busy', 'false');
    await settleForShot(page, theme);
    await expect(profile).toHaveScreenshot(`profile-overview-${locale}-${theme}.png`);
  });
}

for (const { locale, theme, width, height, status, toggle } of [
  { locale: 'ja', theme: 'dark', width: 1280, height: 800,
    status: '開発者モードは有効です。', toggle: '開発者モードを有効にする' },
  { locale: 'en', theme: 'light', width: 390, height: 844,
    status: 'Developer mode is on.', toggle: 'Enable developer mode' },
] as const) {
  const openDeveloperSettings = async (page: Page) => {
    await page.addInitScript(({ locale, theme }) => {
      localStorage.setItem('kukuri.desktop.locale', locale);
      localStorage.setItem('kukuri.desktop.theme', theme);
      localStorage.setItem('kukuri.desktop.developer-mode', 'false');
    }, { locale, theme });
    await page.setViewportSize({ width, height });
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=developer');
    return page.getByRole('dialog');
  };
  test(`developer settings disabled ${locale} ${theme}`, async ({ page }) => {
    const drawer = await openDeveloperSettings(page);
    await expect(drawer.getByRole('status')).toBeVisible();
    await settleForShot(page, theme);
    await expect(drawer).toHaveScreenshot(`developer-disabled-${locale}-${theme}.png`, { maxDiffPixelRatio: 0.001 });
  });
  test(`developer settings enabled ${locale} ${theme}`, async ({ page }) => {
    const drawer = await openDeveloperSettings(page);
    await expect(drawer.getByRole('status')).toBeVisible();
    await drawer.getByRole('checkbox', { name: toggle }).check();
    await expect(drawer.getByRole('status')).toHaveText(status);
    await settleForShot(page, theme);
    await expect(drawer).toHaveScreenshot(`developer-enabled-${locale}-${theme}.png`, { maxDiffPixelRatio: 0.001 });
  });
}

for (const { locale, theme, width, height } of [
  { locale: 'en', theme: 'dark', width: 1280, height: 800 },
  { locale: 'ja', theme: 'light', width: 390, height: 844 },
] as const) {
  test(`feedback unavailable ${locale} ${theme}`, async ({ page }) => {
    await seedFeedback(page, locale, theme);
    await page.setViewportSize({ width, height });
    await page.goto('/#/timeline');
    await page.getByTestId('tester-feedback-trigger').click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByText('Search / 検索 / 搜索')).toBeVisible();
    await expect(dialog).toHaveScreenshot(`feedback-unavailable-${locale}-${theme}.png`);
  });
}

// #1108: 推定による代替表示(枠と短いラベル)と、枠から開く詳細 dialog。
for (const { locale, theme, width, title } of [
  { locale: 'ja', theme: 'dark', width: 1400, title: 'コミュニティノードによる推定' },
  { locale: 'en', theme: 'light', width: 390, title: 'Community Node estimate' },
] as const) {
  test(`advisory placeholder and details ${locale} ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: width < 600 ? 844 : 980 });
    await seedTimelineAdvisory(page, { locale, theme, lookup: 'advisory' });
    await page.goto(TIMELINE_ADVISORY_URL);
    const placeholder = page.getByTestId(`media-adult-gated-${TIMELINE_ADVISORY_OBJECT_ID}`);
    await expect(placeholder).toBeVisible();
    const card = page.locator(`[data-post-object-id="${TIMELINE_ADVISORY_OBJECT_ID}"]`).first();
    await expect(card).toHaveScreenshot(`advisory-placeholder-${locale}-${theme}.png`);
    await placeholder.click();
    const dialog = page.getByRole('dialog', { name: title });
    await expect(dialog.getByTestId(`post-advisory-issuer-${TIMELINE_ADVISORY_OBJECT_ID}`)).toContainText(
      'index.kukuri.example'
    );
    await expect(dialog).toHaveScreenshot(`advisory-details-${locale}-${theme}.png`);
  });
}

// #1171: portal上の画像viewerは共通Dialogの42rem幅ではなく、画像のaspect ratioと
// viewport上限で収まる。production bundleのwide / narrow両方で全体配置を固定する。
for (const { locale, theme, width, height, viewerImage, snapshot } of [
  {
    locale: 'ja',
    theme: 'dark',
    width: 1400,
    height: 980,
    viewerImage: 'landscape',
    snapshot: 'media-viewer-landscape-wide-dark.png',
  },
  {
    locale: 'en',
    theme: 'light',
    width: 390,
    height: 844,
    viewerImage: 'portrait',
    snapshot: 'media-viewer-portrait-narrow-light.png',
  },
] as const) {
  test(`media viewer contains ${viewerImage} image at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await seedExploreMedia(page, { locale, theme, viewerImage });
    const explore = await runExploreSearch(page);
    await explore.locator('.media-image-trigger').first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await settleForShot(page, theme);
    await expect(page).toHaveScreenshot(snapshot);
  });
}

test('app consent unchecked English dark', async ({ page }) => {
  await seedAppConsent(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/');
  const accept = page.locator('.app-consent-actions').getByRole('button').first();
  await expect(accept).toHaveAccessibleName('Age confirmation required');
  await expect(accept).toBeDisabled();
  // A button-only change can fit inside the global full-page 1% tolerance.
  await expect(page).toHaveScreenshot('app-consent-en-dark.png', { maxDiffPixelRatio: 0.001 });
  await expect(accept).toHaveScreenshot('app-consent-action-blocked-en-dark.png', { maxDiffPixelRatio: 0.001 });
  await page.getByRole('checkbox').check();
  await expect(accept).toHaveAccessibleName('Accept and continue');
  await expect(accept).toBeEnabled();
  await expect(accept).toHaveScreenshot('app-consent-action-ready-en-dark.png', { maxDiffPixelRatio: 0.001 });
});

test('app consent checked Japanese light narrow', async ({ page }) => {
  await seedAppConsent(page, { locale: 'ja', theme: 'light' });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await page.getByRole('checkbox').check();
  await page.getByRole('checkbox').blur();
  await expect(page.getByRole('button', { name: '同意して続行' })).toBeEnabled();
  await expect(page).toHaveScreenshot('app-consent-ja-light-narrow.png');
});

test('community node introduction wide English dark', async ({ page }) => {
  await seedUnconsentedCommunityNodes(page, { locale: 'en', theme: 'dark' });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/explore?topic=kukuri%3Atopic%3Ageneral');
  const dialog = page.getByRole('dialog', { name: 'What is a community node?' });
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveScreenshot('community-node-introduction-en-dark.png');
});

test('community node policies narrow English light', async ({ page }) => {
  await seedUnconsentedCommunityNodes(page, { locale: 'en', theme: 'light' });
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto('/#/explore?topic=kukuri%3Atopic%3Ageneral');
  await page.getByRole('dialog').getByRole('button', { name: 'Review terms' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Privacy Policy' })).toBeVisible();
  await expect(dialog).toHaveScreenshot('community-node-policies-en-light.png');
  await dialog.getByRole('button', { name: 'Terms of Service' }).click();
  await expect(dialog.getByRole('heading', { name: 'Scope' })).toBeVisible();
  await expect(dialog).toHaveScreenshot('community-node-policy-expanded-en-light.png');
});

test('Explore Japanese long policy labels stay inside the Column', async ({ page }) => {
  await seedIndexLayout(page, { pendingNodes: true });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/explore?topic=kukuri%3Atopic%3Ageneral');
  const workspace = page.getByTestId('community-index-explore');
  await expect(workspace.getByRole('textbox')).toBeVisible();
  await expectIndexContentContained(workspace);
  await settleForShot(page, 'dark');
  await expect(page).toHaveScreenshot('explore-long-policies-ja-dark.png');
});

test('Control Center Japanese content density', async ({ page }) => {
  await seedIndexLayout(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/explore?topic=kukuri%3Atopic%3Ageneral');
  await page.getByTestId('control-center-trigger').click();
  await expect(page.locator('.shell-control-center-grid')).toHaveCSS('font-size', '14px');
  await settleForShot(page, 'dark');
  await page.mouse.move(1275, 2);
  await expect(page).toHaveScreenshot('control-center-ja-dark.png');
});

/**
 * テーマ / ロケールを localStorage へ事前注入する（UI 操作でのテーマ切替は
 * トランジション残りのリスクがあるため addInitScript で注入する）。goto より前に呼ぶ。
 */
async function seedAppearance(page: Page, theme: DesktopTheme): Promise<void> {
  await page.addInitScript(
    ({ themeKey, themeValue, localeKey, localeValue, developerModeKey }) => {
      window.localStorage.setItem(themeKey, themeValue);
      window.localStorage.setItem(localeKey, localeValue);
      // 既存 baseline は Live/Game タブとステータスバッジを含むため developer mode を有効化する。
      window.localStorage.setItem(developerModeKey, 'true');
    },
    {
      themeKey: DESKTOP_THEME_STORAGE_KEY,
      themeValue: theme,
      localeKey: DESKTOP_LOCALE_STORAGE_KEY,
      localeValue: LOCALE_EN,
      developerModeKey: DEVELOPER_MODE_STORAGE_KEY,
    }
  );
}

/**
 * :focus-visible ring（base.css の 2px）や caret の写り込みを避けるため、撮影前に
 * アクティブ要素を blur し、body へフォーカスを移す。
 */
async function clearFocus(page: Page): Promise<void> {
  await page.evaluate(() => {
    const active = document.activeElement as HTMLElement | null;
    active?.blur?.();
    if (document.body) {
      document.body.focus();
    }
    window.scrollTo(0, 0);
  });
}

/**
 * data-theme が反映され、対象要素が visible になるまで待ってから撮影前処理を行う。
 */
async function settleForShot(page: Page, theme: DesktopTheme): Promise<void> {
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  await clearFocus(page);
}

async function expectLanguageBeforeTheme(settings: Locator) {
  await expect(settings.getByTestId('settings-section-appearance')).toHaveText('Language & theme');
  const language = await settings.getByRole('combobox', { name:'Language', exact:true }).boundingBox();
  const theme = await settings.getByRole('radiogroup', { name:'Theme mode' }).boundingBox();
  expect(language).not.toBeNull();
  expect(theme).not.toBeNull();
  expect(language!.y + language!.height).toBeLessThanOrEqual(theme!.y);
}

async function openComposerDialog(page: Page): Promise<void> {
  await activeColumn(page, 'Timeline').getByRole('button', { name: /^Post to / }).click();
  await expect(page.getByPlaceholder('Write a post')).toBeVisible();
}

function activeColumn(page: Page, title: string) {
  return page.getByRole('region', {
    name: new RegExp(`^${title} Column,.*Active,`),
  });
}

test.describe('visual regression smoke', () => {
  // 1: fresh default wide dark（5 Columnの初期構成とシェル全体の基準）
  test('fresh default wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/');
    await expect(page.getByText('browser mock peer post')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('timeline-wide-dark.png');
  });

  // 2: fresh default wide light（トークン退行）
  test('fresh default wide light', async ({ page }) => {
    await seedAppearance(page, 'light');
    await page.setViewportSize(WIDE);
    await page.goto('/');
    await expect(page.getByText('browser mock peer post')).toBeVisible();
    await settleForShot(page, 'light');
    await expect(page).toHaveScreenshot('timeline-wide-light.png');
  });

  // 3: fresh default narrow dark（760px 未満 breakpoint、nav オフキャンバス）
  test('fresh default narrow dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(NARROW);
    await page.goto('/');
    await expect(page.getByText('browser mock peer post')).toBeVisible();
    await expect(page.getByTestId('control-center-trigger')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('timeline-narrow-dark.png');
  });

  // 4: fresh default narrow light（narrow × light）
  test('fresh default narrow light', async ({ page }) => {
    await seedAppearance(page, 'light');
    await page.setViewportSize(NARROW);
    await page.goto('/');
    await expect(page.getByText('browser mock peer post')).toBeVisible();
    await expect(page.getByTestId('control-center-trigger')).toBeVisible();
    await settleForShot(page, 'light');
    await expect(page).toHaveScreenshot('timeline-narrow-light.png');
  });

  // 5: thread pane open（context pane 層）
  test('thread pane wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/');
    await page.getByText('browser mock peer post').click();
    await expect(activeColumn(page, 'Thread')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('thread-pane-wide-dark.png');
  });

  // 6: author pane open（detail pane スタック）
  test('author pane wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/');
    await page.getByText('browser mock peer post').click();
    const threadPane = activeColumn(page, 'Thread');
    await expect(threadPane).toBeVisible();
    // Thread 内のシード post の著者（browser peer）を開いて Author pane を積む。
    await threadPane
      .getByRole('button', { name: 'browser peer' })
      .first()
      .click();
    await expect(activeColumn(page, 'Profile')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('author-pane-wide-dark.png');
  });

  // 7: messages（DM workspace）
  test('messages wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/messages');
    await expect(activeColumn(page, 'Messages')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('messages-wide-dark.png');
  });

  // 8: notifications（deep link、notification-item 系 CSS）
  test('notifications wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
    await expect(activeColumn(page, 'Notifications')).toBeVisible();
    await expect(page.getByText('browser mock reply notification')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('notifications-wide-dark.png');
  });

  // 9: settings drawer / appearance（Radix Portal + drawer、H8 最重要）
  test('settings appearance wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance');
    const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
    await expect(settingsDialog).toBeVisible();
    await expect(settingsDialog.getByTestId('settings-section-appearance')).toHaveAttribute(
      'aria-current',
      'location'
    );
    await settleForShot(page, 'dark');
    await expectLanguageBeforeTheme(settingsDialog);
    await expect(page).toHaveScreenshot('settings-appearance-wide-dark.png');
  });

  // 10: settings drawer / appearance light（portal × light）
  test('settings appearance wide light', async ({ page }) => {
    await seedAppearance(page, 'light');
    await page.setViewportSize(WIDE);
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance');
    const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
    await expect(settingsDialog).toBeVisible();
    await expect(settingsDialog.getByTestId('settings-section-appearance')).toHaveAttribute(
      'aria-current',
      'location'
    );
    await settleForShot(page, 'light');
    await expectLanguageBeforeTheme(settingsDialog);
    await expect(page).toHaveScreenshot('settings-appearance-wide-light.png');
  });

  // #962: settings drawer / notifications（受信設定の正本。nav 追加の回帰も兼ねる）
  test('settings notifications wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=notifications');
    const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
    await expect(settingsDialog).toBeVisible();
    await expect(settingsDialog.getByTestId('settings-section-notifications')).toHaveAttribute(
      'aria-current',
      'location'
    );
    await expect(settingsDialog.getByRole('checkbox', { name: 'Enable OS notifications' })).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('settings-notifications-wide-dark.png');
  });

  // #967: settings drawer / backup & restore（作成・復元の入口。nav 密度と section 追加の回帰も兼ねる）
  test('settings backup wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=backup');
    const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
    await expect(settingsDialog).toBeVisible();
    await expect(settingsDialog.getByTestId('settings-section-backup')).toHaveAttribute(
      'aria-current',
      'location'
    );
    await expect(settingsDialog.getByRole('heading', { name: 'Create backup' })).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('settings-backup-wide-dark.png');
  });

  // 11: settings drawer / connectivity（スクロール drawer・フォーム群）
  test('settings connectivity wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=connectivity');
    const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
    await expect(settingsDialog).toBeVisible();
    await expect(settingsDialog.getByTestId('settings-section-connectivity')).toHaveAttribute(
      'aria-current',
      'location'
    );
    // スクロール top を固定してから撮る（付録 B: スクロール top 固定）。
    await settingsDialog
      .locator('.shell-settings-content')
      .evaluate((element) => element.scrollTo(0, 0));
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('settings-connectivity-wide-dark.png');
  });

  // 12: composer dialog open（Radix portal dialog）
  test('composer dialog wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/');
    await expect(page.getByText('browser mock peer post')).toBeVisible();
    await openComposerDialog(page);
    await expect(page.getByPlaceholder('Write a post')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('composer-dialog-wide-dark.png');
  });

  // 13: live workspace（シード空状態、extended 面）
  test('live wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/live');
    await expect(activeColumn(page, 'Live')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('live-wide-dark.png');
  });

  // 14: game workspace（Metaverse Rooms 一覧、WebGL 手前まで）
  test('game wide dark', async ({ page }) => {
    await seedAppearance(page, 'dark');
    await page.setViewportSize(WIDE);
    await page.goto('/#/game');
    await expect(activeColumn(page, 'Metaverse')).toBeVisible();
    await settleForShot(page, 'dark');
    await expect(page).toHaveScreenshot('game-wide-dark.png');
  });
});

for (const { locale, theme, width } of [
  { locale: 'ja', theme: 'dark', width: 1280 },
  { locale: 'en', theme: 'light', width: 390 },
]) {
  test(`connection recovery ${locale} ${theme}`, async ({ page }) => {
    await seedConnectivityDiagnostics(page, locale, theme);
    await page.setViewportSize({ width, height: 844 });
    await page.goto('/#/timeline?settings=connectivity');
    const drawer = page.getByRole('dialog');
    await expect(drawer.getByRole('button', { name: locale === 'ja' ? '診断を更新' : 'Refresh diagnostics', exact: true })).toBeEnabled();
    await settleForShot(page, theme as DesktopTheme);
    await expect(drawer).toHaveScreenshot(`connection-recovery-${locale}-${theme}.png`);
  });
}

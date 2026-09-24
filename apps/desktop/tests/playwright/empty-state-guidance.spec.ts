// #994: ブックマーク / フォロー中 / フォロワー / ミュート中 / ブロック中の 0 件で、実ボタンを示す chip と
// 次の行動が Column 幅に収まり、CTA が既存導線へ到達し、bookmark / follow / mute / block API を呼ばないことを
// 3 locale × 2 theme × 2 幅で確認する。初回 loading は取得が長い場合に完了まで続くことも確認する。
import { expect, test, type Locator, type Page } from '@playwright/test';

import commonEn from '../../src/i18n/locales/en/common.json' with { type: 'json' };
import profileEn from '../../src/i18n/locales/en/profile.json' with { type: 'json' };
import settingsEn from '../../src/i18n/locales/en/settings.json' with { type: 'json' };
import shellEn from '../../src/i18n/locales/en/shell.json' with { type: 'json' };
import commonJa from '../../src/i18n/locales/ja/common.json' with { type: 'json' };
import profileJa from '../../src/i18n/locales/ja/profile.json' with { type: 'json' };
import settingsJa from '../../src/i18n/locales/ja/settings.json' with { type: 'json' };
import shellJa from '../../src/i18n/locales/ja/shell.json' with { type: 'json' };
import commonZh from '../../src/i18n/locales/zh-CN/common.json' with { type: 'json' };
import profileZh from '../../src/i18n/locales/zh-CN/profile.json' with { type: 'json' };
import settingsZh from '../../src/i18n/locales/zh-CN/settings.json' with { type: 'json' };
import shellZh from '../../src/i18n/locales/zh-CN/shell.json' with { type: 'json' };

const COPY = {
  en: { common: commonEn, profile: profileEn, settings: settingsEn, shell: shellEn },
  ja: { common: commonJa, profile: profileJa, settings: settingsJa, shell: shellJa },
  'zh-CN': { common: commonZh, profile: profileZh, settings: settingsZh, shell: shellZh },
} as const;

type LocaleId = keyof typeof COPY;
const MUTATIONS = ['bookmarkPost', 'followAuthor', 'muteAuthor', 'blockAuthor'] as const;
const TOPIC_QUERY = 'topic=kukuri%3Atopic%3Ageneral';
const BOOKMARKS_ROUTE = `/#/timeline?${TOPIC_QUERY}&timelineView=bookmarks`;
const connectionsRoute = (view: string) =>
  `/#/profile?${TOPIC_QUERY}&profileMode=connections&connectionsView=${view}`;

async function seedEmptyLists(
  page: Page,
  { locale, theme, bookmarkDelayMs = 0 }: { locale: LocaleId; theme: 'dark' | 'light'; bookmarkDelayMs?: number }
) {
  await page.addInitScript(
    ({ locale, theme, bookmarkDelayMs, mutations }) => {
      localStorage.setItem('kukuri.desktop.locale', locale);
      localStorage.setItem('kukuri.desktop.theme', theme);
      const calls: string[] = [];
      Object.defineProperty(window, '__emptyStateCalls', { value: calls });
      let desktopApi = window.__KUKURI_DESKTOP__;
      Object.defineProperty(window, '__KUKURI_DESKTOP__', {
        configurable: true,
        get: () => desktopApi,
        set: (api: typeof desktopApi) => {
          desktopApi = api;
          if (!api) return;
          // browser seed はフォロー中 / フォロワーに 2 名を持つため、4 view とも 0 件にする。
          api.listSocialConnections = async () => {
            calls.push('listSocialConnections');
            return [];
          };
          const originalBookmarks = api.listBookmarkedPostsPage.bind(api);
          api.listBookmarkedPostsPage = async (cursor, before) => {
            calls.push('listBookmarkedPostsPage');
            if (bookmarkDelayMs > 0) {
              await new Promise<void>((resolve) => setTimeout(resolve, bookmarkDelayMs));
            }
            return originalBookmarks(cursor, before);
          };
          const mutable = api as unknown as Record<string, (...args: unknown[]) => Promise<unknown>>;
          for (const method of mutations) {
            const original = mutable[method];
            mutable[method] = async (...args: unknown[]) => {
              calls.push(method);
              return original.apply(api, args);
            };
          }
        },
      });
    },
    { locale, theme, bookmarkDelayMs, mutations: MUTATIONS }
  );
}

async function recordedCalls(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __emptyStateCalls: string[] }).__emptyStateCalls);
}

async function expectContained(page: Page, guidance: Locator) {
  const column = page.locator('[data-column-id]').filter({ has: guidance });
  const [columnBox, guidanceBox] = await Promise.all([column.boundingBox(), guidance.boundingBox()]);
  expect(columnBox).not.toBeNull();
  expect(guidanceBox).not.toBeNull();
  expect(guidanceBox!.x).toBeGreaterThanOrEqual(columnBox!.x - 1);
  expect(guidanceBox!.x + guidanceBox!.width).toBeLessThanOrEqual(columnBox!.x + columnBox!.width + 1);
  expect(await guidance.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
}

async function chipLabels(guidance: Locator): Promise<string[]> {
  return guidance.locator('[data-testid="action-ref"]').allInnerTexts();
}

const COMBOS = [
  ['en', 'dark', 1280],
  ['en', 'light', 390],
  ['ja', 'dark', 1280],
  ['ja', 'light', 390],
  ['zh-CN', 'dark', 1280],
  ['zh-CN', 'light', 390],
] as const;

for (const [locale, theme, width] of COMBOS) {
  const copy = COPY[locale];

  test(`bookmarks empty guidance ${locale} ${theme} ${width}`, async ({ page }) => {
    await seedEmptyLists(page, { locale, theme });
    await page.setViewportSize({ width, height: 900 });
    await page.goto(BOOKMARKS_ROUTE);

    const guidance = page.getByTestId('bookmarks-empty-state');
    await expect(guidance).toBeVisible();
    await expect(guidance).toHaveAttribute('role', 'status');
    await expect(guidance.getByText(copy.shell.workspace.noBookmarks, { exact: true })).toBeVisible();
    await expect(guidance.getByText(copy.shell.workspace.bookmarksEmpty.note, { exact: true })).toBeVisible();
    // 投稿カードの icon ボタンと同じ icon + 操作名の chip。button ではない。
    expect(await chipLabels(guidance)).toEqual([copy.common.actions.bookmark]);
    await expect(guidance.locator('[data-testid="action-ref"] svg')).toHaveAttribute('aria-hidden', 'true');
    await expect(guidance.getByRole('button', { name: copy.common.actions.bookmark, exact: true })).toHaveCount(0);
    await expect(page.getByRole('status', { name: copy.shell.workspace.bookmarksLoading })).toHaveCount(0);
    await expectContained(page, guidance);

    // CTA は同じ Column をフィードへ戻し、route から timelineView が消える。
    await guidance.getByRole('button', { name: copy.shell.workspace.bookmarksEmpty.showTimeline, exact: true }).click();
    await expect(page).not.toHaveURL(/timelineView=bookmarks/);
    await expect(page.getByRole('tab', { name: copy.shell.workspace.feed, exact: true }).first()).toHaveAttribute(
      'aria-selected',
      'true'
    );
    await expect(guidance).toHaveCount(0);
    const calls = await recordedCalls(page);
    expect(calls.filter((call) => (MUTATIONS as readonly string[]).includes(call))).toEqual([]);
  });

  test(`social connections empty guidance ${locale} ${theme} ${width}`, async ({ page }) => {
    await seedEmptyLists(page, { locale, theme });
    await page.setViewportSize({ width, height: 900 });
    const expectations = [
      {
        view: 'following',
        chips: [copy.common.actions.follow],
        texts: [copy.profile.connections.emptyGuidance.openProfile],
        actions: [copy.profile.connections.emptyActions.openExplore, copy.profile.connections.emptyActions.openTimeline],
      },
      {
        view: 'followed',
        chips: [],
        texts: [copy.profile.connections.emptyGuidance.followedInfo, copy.profile.connections.emptyGuidance.shareOwnId],
        actions: [copy.common.actions.copyAuthorId, copy.profile.connections.emptyActions.openTimeline],
      },
      {
        view: 'muted',
        chips: [copy.common.actions.mute, copy.shell.report.actionLabel],
        texts: [copy.profile.connections.emptyGuidance.openProfile, copy.settings.safety.social.mute],
        actions: [copy.profile.connections.emptyActions.openTimeline],
      },
      {
        view: 'blocking',
        chips: [copy.common.actions.block],
        texts: [copy.profile.connections.emptyGuidance.openProfile, copy.settings.safety.social.block],
        actions: [copy.profile.connections.emptyActions.openTimeline],
      },
    ] as const;

    for (const expectation of expectations) {
      await page.goto(connectionsRoute(expectation.view));
      const guidance = page.getByTestId('profile-connections-empty-state');
      await expect(guidance).toBeVisible();
      await expect(guidance).toHaveAttribute('role', 'status');
      await expect(
        guidance.getByText(copy.profile.connections.empty[expectation.view], { exact: true })
      ).toBeVisible();
      for (const text of expectation.texts) {
        await expect(guidance.getByText(text, { exact: true })).toBeVisible();
      }
      expect(await chipLabels(guidance)).toEqual([...expectation.chips]);
      for (const chip of expectation.chips) {
        await expect(guidance.getByRole('button', { name: chip, exact: true })).toHaveCount(0);
      }
      for (const action of expectation.actions) {
        await expect(guidance.getByRole('button', { name: action, exact: true })).toBeVisible();
      }
      await expect(page.getByText(copy.profile.connections.loading, { exact: true })).toHaveCount(0);
      await expectContained(page, guidance);
    }

    // 最後の view からタイムラインへ移る。follow / mute / block の送信は 0 回。
    await page
      .getByTestId('profile-connections-empty-state')
      .getByRole('button', { name: copy.profile.connections.emptyActions.openTimeline, exact: true })
      .click();
    await expect(page).toHaveURL(/#\/timeline/);
    const calls = await recordedCalls(page);
    expect(calls.filter((call) => (MUTATIONS as readonly string[]).includes(call))).toEqual([]);
  });
}

test('explore action from the following empty state moves to Explore', async ({ page }) => {
  await seedEmptyLists(page, { locale: 'en', theme: 'dark' });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto(connectionsRoute('following'));
  const guidance = page.getByTestId('profile-connections-empty-state');
  await guidance.getByRole('button', { name: COPY.en.profile.connections.emptyActions.openExplore, exact: true }).click();
  await expect(page).toHaveURL(/#\/explore/);
});

test('a slow bookmark fetch keeps loading until it completes, then shows the guidance', async ({ page }) => {
  await seedEmptyLists(page, { locale: 'en', theme: 'dark', bookmarkDelayMs: 1500 });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto(BOOKMARKS_ROUTE);
  const loading = page.getByRole('status', { name: COPY.en.shell.workspace.bookmarksLoading });
  await expect(loading).toBeVisible();
  await expect(page.getByTestId('bookmarks-empty-state')).toHaveCount(0);
  await expect(page.getByText(COPY.en.shell.workspace.noBookmarks, { exact: true })).toHaveCount(0);
  await page.waitForTimeout(800);
  // 0.5 秒を過ぎても取得が続く間は loading のまま(完了まで続ける)。
  await expect(loading).toBeVisible();
  await expect(page.getByTestId('bookmarks-empty-state')).toHaveCount(0);
  await expect(page.getByTestId('bookmarks-empty-state')).toBeVisible({ timeout: 5000 });
  await expect(loading).toHaveCount(0);
});

test('the settings entry opens the muted list with the same guidance', async ({ page }) => {
  await seedEmptyLists(page, { locale: 'en', theme: 'dark' });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto(`/#/timeline?${TOPIC_QUERY}&settings=safety`);
  const drawer = page.getByRole('dialog');
  await drawer.getByRole('button', { name: COPY.en.settings.safety.social.openMuted, exact: true }).click();
  const guidance = page.getByTestId('profile-connections-empty-state');
  await expect(guidance).toBeVisible();
  await expect(guidance.getByText(COPY.en.profile.connections.empty.muted, { exact: true })).toBeVisible();
  expect(await chipLabels(guidance)).toEqual([COPY.en.common.actions.mute, COPY.en.shell.report.actionLabel]);
});

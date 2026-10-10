import { expect, test, type Locator, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';
import { DESKTOP_THEME_STORAGE_KEY, type DesktopTheme } from '../../src/lib/theme';

const DESKTOP_LOCALE_STORAGE_KEY = 'kukuri.desktop.locale';

const LOCALES = [
  {
    locale: 'en',
    timeline: 'Timeline',
    controlCenter: 'Control Center',
    settings: 'Settings',
    appearance: 'Language & theme',
  },
  {
    locale: 'ja',
    timeline: 'タイムライン',
    controlCenter: 'コントロールセンター',
    settings: '設定',
    appearance: '表示と言語',
  },
  {
    locale: 'zh-CN',
    timeline: '时间线',
    controlCenter: '控制中心',
    settings: '设置',
    appearance: '外观与语言',
  },
] as const;

const MAJOR_ROUTES = [
  '/#/timeline?topic=kukuri%3Atopic%3Ageneral',
  '/#/explore?topic=kukuri%3Atopic%3Ageneral',
  '/#/messages?topic=kukuri%3Atopic%3Ageneral',
  '/#/notifications?topic=kukuri%3Atopic%3Ageneral',
  '/#/profile?topic=kukuri%3Atopic%3Ageneral',
  '/#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=connections&connectionsView=muted',
  '/#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=connections&connectionsView=blocking',
  '/#/live?topic=kukuri%3Atopic%3Ageneral',
  '/#/game?topic=kukuri%3Atopic%3Ageneral',
] as const;

const SETTINGS_SECTIONS = [
  'about',
  'appearance',
  'notifications',
  'backup',
  'account',
  'connectivity',
  'discovery',
  'community-node',
  'reactions',
  'release',
  'developer',
] as const;

const FORBIDDEN_ENGLISH_UI_COPY = [
  'Primary workspace',
  'Timeline views',
  'Messages',
  'No direct messages yet.',
  'No messages yet.',
  'Write a message',
] as const;

const FORBIDDEN_JAPANESE_UI_COPY = [
  'Community Node',
  'Timeline',
  'Explore',
  'Loading...',
  'Load more',
] as const;

async function seedLocale(page: Page, locale: (typeof LOCALES)[number]['locale']) {
  await page.addInitScript(
    ({ developerModeKey, localeKey, localeValue }) => {
      window.localStorage.setItem(developerModeKey, 'true');
      window.localStorage.setItem(localeKey, localeValue);
    },
    {
      developerModeKey: DEVELOPER_MODE_STORAGE_KEY,
      localeKey: DESKTOP_LOCALE_STORAGE_KEY,
      localeValue: locale,
    }
  );
}

async function wrappedButtonLabels(root: Page | Locator): Promise<string[]> {
  return root.locator('button:visible').evaluateAll((buttons) => {
    const wrapped: string[] = [];

    for (const button of buttons) {
      const buttonRect = button.getBoundingClientRect();
      const intersectsViewport =
        buttonRect.right > 0 &&
        buttonRect.bottom > 0 &&
        buttonRect.left < window.innerWidth &&
        buttonRect.top < window.innerHeight;
      if (!intersectsViewport) continue;

      const walker = document.createTreeWalker(button, NodeFilter.SHOW_TEXT);
      let textNode = walker.nextNode();
      while (textNode) {
        if (textNode.textContent?.trim()) {
          const range = document.createRange();
          range.selectNodeContents(textNode);
          const lineTops = new Set(
            Array.from(range.getClientRects())
              .filter((rect) => rect.width > 1 && rect.height > 1)
              .map((rect) => Math.round(rect.top))
          );
          if (lineTops.size > 1) {
            wrapped.push((button as HTMLElement).innerText.trim().replace(/\s+/g, ' '));
            break;
          }
        }
        textNode = walker.nextNode();
      }
    }

    return [...new Set(wrapped)];
  });
}

async function overflowingButtonLabels(root: Locator): Promise<string[]> {
  return root.locator('button').evaluateAll((buttons) =>
    buttons.flatMap((button) => {
      const bounds = button.getBoundingClientRect();
      if (!bounds.width || !bounds.height) return [];
      const range = document.createRange();
      range.selectNodeContents(button);
      const text = range.getBoundingClientRect();
      return text.left < bounds.left - 1 || text.right > bounds.right + 1
        ? [button.textContent?.trim() ?? '']
        : [];
    })
  );
}

for (const copy of LOCALES) {
  test(`${copy.locale} keeps major routes contained without wrapped controls`, async ({ page }) => {
    await seedLocale(page, copy.locale);
    await page.setViewportSize({ width: 900, height: 760 });

    for (const route of MAJOR_ROUTES) {
      await page.goto(route);
      await expect(page.locator('.shell-column-surface').first()).toBeVisible();
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        `${copy.locale} ${route} should not overflow horizontally`
      ).toBe(true);
      expect(
        await wrappedButtonLabels(page.locator('[data-active="true"]')),
        `${copy.locale} ${route} active Column button labels`
      ).toEqual([]);

      if (copy.locale !== 'en') {
        const renderedCopy = await page.locator('body').innerText();
        const accessibleCopy = await page
          .locator('[aria-label]')
          .evaluateAll((elements) =>
            elements.map((element) => element.getAttribute('aria-label') ?? '').join('\n')
          );
        for (const forbidden of FORBIDDEN_ENGLISH_UI_COPY) {
          expect(
            `${renderedCopy}\n${accessibleCopy}`,
            `${copy.locale} ${route} must localize ${forbidden}`
          ).not.toContain(forbidden);
        }
        if (copy.locale === 'ja') {
          for (const forbidden of FORBIDDEN_JAPANESE_UI_COPY) {
            expect(
              `${renderedCopy}\n${accessibleCopy}`,
              `ja ${route} must localize ${forbidden}`
            ).not.toContain(forbidden);
          }
        }
      }
    }
  });

  test(`${copy.locale} keeps primary labels visible and Control Center content reachable`, async ({
    page,
  }) => {
    await seedLocale(page, copy.locale);
    await page.setViewportSize({ width: 900, height: 760 });
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

    const title = page.getByRole('heading', { name: copy.timeline, exact: true });
    await expect(title).toBeVisible();
    expect(await title.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(
      true
    );

    await page.getByTestId('control-center-trigger').click();
    const controlCenter = page.getByRole('complementary', {
      name: copy.controlCenter,
      exact: true,
    });
    await expect(controlCenter).toBeVisible();

    const inaccessibleSections = await controlCenter
      .locator('.shell-control-center-section')
      .evaluateAll((sections) =>
        sections
          .filter((section) => {
            const style = getComputedStyle(section);
            return (
              section.scrollHeight > section.clientHeight + 1 &&
              style.overflowY !== 'auto' &&
              style.overflowY !== 'scroll'
            );
          })
          .map((section) => section.getAttribute('aria-labelledby') ?? section.textContent ?? '')
      );
    expect(inaccessibleSections).toEqual([]);

    const overlappingSections = await controlCenter
      .locator('.shell-control-center-section')
      .evaluateAll((sections) => {
        const rects = sections.map((section) => section.getBoundingClientRect());
        const overlaps: string[] = [];
        for (let left = 0; left < rects.length; left += 1) {
          for (let right = left + 1; right < rects.length; right += 1) {
            const horizontal =
              Math.min(rects[left].right, rects[right].right) -
              Math.max(rects[left].left, rects[right].left);
            const vertical =
              Math.min(rects[left].bottom, rects[right].bottom) -
              Math.max(rects[left].top, rects[right].top);
            if (horizontal > 1 && vertical > 1) overlaps.push(`${left}:${right}`);
          }
        }
        return overlaps;
      });
    expect(overlappingSections).toEqual([]);
    expect(await wrappedButtonLabels(controlCenter)).toEqual([]);

    const grid = controlCenter.locator('.shell-control-center-grid');
    const canReachGridEnd = await grid.evaluate((element) => {
      if (element.scrollHeight <= element.clientHeight + 1) return true;
      element.scrollTop = element.scrollHeight;
      return element.scrollTop > 0;
    });
    expect(canReachGridEnd).toBe(true);
  });

  test(`${copy.locale} keeps settings action labels inside their buttons at minimum width`, async ({
    page,
  }) => {
    await seedLocale(page, copy.locale);
    for (const [width, theme] of [[320, 'dark'], [320, 'light'], [900, 'dark']] as const) {
      await page.addInitScript(
        ({ key, value }) => window.localStorage.setItem(key, value),
        { key: DESKTOP_THEME_STORAGE_KEY, value: theme }
      );
      await page.setViewportSize({ width, height: 844 });
      await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=community-node');
      const settings = page.getByRole('dialog', { name: copy.settings, exact: true });
      await expect(settings).toBeVisible();
      const overflow = await overflowingButtonLabels(settings.locator('.shell-settings-content'));
      expect(overflow, `${copy.locale} ${width}px ${theme} settings labels`).toEqual([]);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width
      );
    }
  });

  test(`${copy.locale} keeps enlarged settings navigation from hiding the content`, async ({ page }) => {
    await seedLocale(page, copy.locale);
    await page.setViewportSize({ width: 411, height: 866 });
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=community-node');
    const settings = page.getByRole('dialog', { name: copy.settings, exact: true });
    await expect(settings).toBeVisible();
    // Match the 28px navigation text observed with Android font_scale=2.0.
    await page.addStyleTag({ content: '.shell-settings-nav-item { font-size: 1.75rem; }' });
    expect(await overflowingButtonLabels(settings.locator('.shell-settings-nav'))).toEqual([]);
    expect(
      await settings.locator('.shell-settings-nav').evaluate((nav) => nav.clientHeight)
    ).toBeLessThanOrEqual(866 * 0.45);
    expect(
      await settings.locator('.shell-settings-content').evaluate((content) => content.clientHeight)
    ).toBeGreaterThanOrEqual(280);
  });

  test(`${copy.locale} keeps narrow settings navigation compact and content scrollable`, async ({
    page,
  }) => {
    await seedLocale(page, copy.locale);
    await page.setViewportSize({ width: 390, height: 844 });

    for (const section of SETTINGS_SECTIONS) {
      await page.goto(`/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=${section}`);

      const settings = page.getByRole('dialog', { name: copy.settings, exact: true });
      await expect(settings).toBeVisible();
      await expect(settings.getByTestId(`settings-section-${section}`)).toHaveAttribute(
        'aria-current',
        'location'
      );
      if (section === 'appearance') {
        await expect(settings.getByTestId('settings-section-appearance')).toContainText(
          copy.appearance
        );
      }

      const layout = await settings.evaluate((drawer) => {
        const nav = drawer.querySelector<HTMLElement>('.shell-settings-nav');
        const content = drawer.querySelector<HTMLElement>('.shell-settings-content');
        if (!nav || !content) return null;
        return {
          drawerHeight: drawer.clientHeight,
          navHeight: nav.clientHeight,
          contentHeight: content.clientHeight,
          contentOverflowY: getComputedStyle(content).overflowY,
        };
      });

      expect(layout, `${copy.locale} ${section} settings layout`).not.toBeNull();
      expect(layout!.navHeight).toBeLessThanOrEqual(layout!.drawerHeight * 0.45);
      expect(layout!.contentHeight).toBeGreaterThanOrEqual(280);
      expect(layout!.contentOverflowY).toMatch(/auto|scroll/);

      const content = settings.locator('.shell-settings-content');
      const canReachContentEnd = await content.evaluate((element) => {
        if (element.scrollHeight <= element.clientHeight + 1) return true;
        element.scrollTop = element.scrollHeight;
        return element.scrollTop > 0;
      });
      expect(canReachContentEnd, `${copy.locale} ${section} content end`).toBe(true);
      expect(await wrappedButtonLabels(settings), `${copy.locale} ${section} button labels`).toEqual(
        []
      );
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        `${copy.locale} ${section} document width`
      ).toBe(true);
    }
  });
}

test('ja localizes Explore post actions and their tooltips', async ({ page }) => {
  await seedLocale(page, 'ja');
  await page.setViewportSize({ width: 900, height: 760 });
  await page.goto('/#/explore?topic=kukuri%3Atopic%3Ageneral');

  const explore = page.getByTestId('community-index-explore');
  await explore.getByRole('textbox', { name: '検索語' }).fill('dev topic');
  await explore.getByRole('button', { name: '結果を表示' }).click();

  const card = explore.locator('article.post-card').first();
  await expect(card).toBeVisible();
  for (const action of [
    'リアクション',
    'リポスト',
    '返信',
    'リンクをコピー',
    'ブックマーク',
    '通報',
  ]) {
    await expect(card.getByRole('button', { name: action })).toBeVisible();
  }

  const reply = card.getByRole('button', { name: '返信' });
  await reply.hover();
  await expect(page.getByRole('tooltip')).toHaveText('返信');

  const visibleAndAccessibleCopy = `${await page.locator('body').innerText()}\n${await page
    .locator('[aria-label]')
    .evaluateAll((elements) =>
      elements.map((element) => element.getAttribute('aria-label') ?? '').join('\n')
    )}`;
  for (const forbidden of FORBIDDEN_JAPANESE_UI_COPY) {
    expect(visibleAndAccessibleCopy).not.toContain(forbidden);
  }
  expect(visibleAndAccessibleCopy).not.toMatch(/\+\d+ media/u);
});

for (const copy of LOCALES) {
  for (const theme of ['dark', 'light'] satisfies DesktopTheme[]) {
    test(`${copy.locale} stays contained at a 200% effective viewport in ${theme}`, async ({
      page,
    }) => {
      await seedLocale(page, copy.locale);
      await page.addInitScript(
        ({ themeKey, themeValue }) => window.localStorage.setItem(themeKey, themeValue),
        { themeKey: DESKTOP_THEME_STORAGE_KEY, themeValue: theme }
      );
      // Browser zoom halves the CSS viewport. 450×380 exercises the same reflow pressure as
      // a 900×760 window at 200% without depending on browser-specific zoom shortcuts.
      await page.setViewportSize({ width: 450, height: 380 });
      await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

      await expect(page.locator('html')).toHaveAttribute('lang', copy.locale);
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await expect(page.getByRole('heading', { name: copy.timeline, exact: true })).toBeVisible();
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        `${copy.locale} ${theme} 200% document width`
      ).toBe(true);
      expect(
        await wrappedButtonLabels(page.locator('[data-active="true"]')),
        `${copy.locale} ${theme} 200% active Column button labels`
      ).toEqual([]);
    });
  }
}

import { expect, test, type Locator, type Page } from '@playwright/test';

import { DESKTOP_THEME_STORAGE_KEY } from '../../src/lib/theme';

// #964: 投稿作成の keyboard 操作(Esc / Tab / Ctrl+Enter)と、通常 GUI から到達できる
// ショートカット案内を実 keyboard 入力で固定する。browser 結果は Linux .deb 実機観測を
// 置き換えない(実機は docs/progress の #964 記録を参照)。

const COPY = {
  ja: {
    post: '投稿を書く',
    close: '閉じる',
    chooseFiles: 'ファイルを選択',
    chooseBookmarks: 'ブックマークから選択',
    adult: 'この投稿を成人向けとして申告する',
    publish: '投稿',
    hint: 'Esc で閉じる、Ctrl+Enter で送信。',
    help: 'キーボード操作',
    settings: '設定',
    keyboardSection: 'キーボード操作',
    emptyDraft: '本文を入力するか、ファイルを添付してから送信してください。',
  },
  en: {
    post: 'Write a post',
    close: 'Close',
    chooseFiles: 'Choose files',
    chooseBookmarks: 'Choose from bookmarks',
    adult: 'Label this post as adult material (self-declared)',
    publish: 'Post',
    hint: 'Esc closes, Ctrl+Enter sends.',
    help: 'Keyboard shortcuts',
    settings: 'Settings',
    keyboardSection: 'Keyboard',
    emptyDraft: 'Write something or attach a file before posting.',
  },
} as const;

async function seed(page: Page, locale: keyof typeof COPY, theme: 'dark' | 'light') {
  await page.addInitScript(
    ({ locale, theme, themeKey }) => {
      window.localStorage.setItem('kukuri.desktop.locale', locale);
      window.localStorage.setItem(themeKey, theme);
    },
    { locale, theme, themeKey: DESKTOP_THEME_STORAGE_KEY }
  );
}

function activeColumn(page: Page) {
  return page.locator('[data-column-id][aria-current="true"]');
}

async function openComposer(page: Page, placeholder: string) {
  const column = activeColumn(page);
  const action = column.locator('.shell-column-primary-action');
  await action.click();
  await expect(column.getByPlaceholder(placeholder)).toBeVisible();
  return column;
}

// focus-visible の outline / box-shadow が実際に描画されているか(none ではないか)を確認する。
async function expectVisibleFocusRing(target: Locator) {
  await expect(target).toBeFocused();
  const ring = await target.evaluate((element) => {
    const style = getComputedStyle(element);
    const outlineVisible =
      style.outlineStyle !== 'none' && parseFloat(style.outlineWidth) > 0;
    const shadowVisible = style.boxShadow !== 'none' && style.boxShadow !== '';
    return { outlineVisible, shadowVisible, matchesFocusVisible: element.matches(':focus-visible') };
  });
  expect(ring.matchesFocusVisible, 'keyboard focus should match :focus-visible').toBe(true);
  expect(ring.outlineVisible || ring.shadowVisible, 'focus ring must be painted').toBe(true);
}

for (const locale of ['ja', 'en'] as const) {
  for (const theme of ['dark', 'light'] as const) {
    const copy = COPY[locale];

    test(`${locale} ${theme}: Tab reaches every composer control with a visible focus ring, Esc closes and restores focus`, async ({ page }) => {
      await seed(page, locale, theme);
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto('/#/timeline');
      const column = await openComposer(page, copy.post);
      const textarea = column.getByPlaceholder(copy.post);
      await textarea.fill('keyboard draft');
      await expect(column.getByText(copy.hint, { exact: true })).toBeVisible();

      // 閉じる → 案内link → 本文 → ファイルを選択 → ブックマークから選択 → 成人向け申告 → 送信 の順に Tab で到達する。
      // :focus-visible は keyboard 由来の focus にだけ一致するため、script の focus() ではなく
      // 本文から Shift+Tab で先頭の control へ戻ってから巡回する。
      await textarea.click();
      await page.keyboard.press('Shift+Tab');
      await page.keyboard.press('Shift+Tab');
      const expectedOrder = [
        column.getByRole('button', { name: copy.close, exact: true }),
        column.getByRole('button', { name: copy.help, exact: true }),
        textarea,
        column.getByRole('button', { name: copy.chooseFiles, exact: true }),
        column.getByRole('button', { name: copy.chooseBookmarks, exact: true }),
        column.getByRole('checkbox', { name: copy.adult }),
        column.getByRole('button', { name: copy.publish, exact: true }),
      ];
      await expectVisibleFocusRing(expectedOrder[0]);
      for (const control of expectedOrder.slice(1)) {
        await page.keyboard.press('Tab');
        await expectVisibleFocusRing(control);
      }
      await page.keyboard.press('Shift+Tab');
      await expectVisibleFocusRing(expectedOrder[5]);

      // 送信ボタンではなく本文以外の control に focus がある状態の Esc でも閉じる。
      await page.keyboard.press('Escape');
      await expect(column.getByPlaceholder(copy.post)).toBeHidden();
      const action = column.locator('.shell-column-primary-action');
      await expectVisibleFocusRing(action);

      // 下書きは保持され、再度開くと復元される。送信は起きない。
      await page.keyboard.press('Enter');
      await expect(column.getByPlaceholder(copy.post)).toHaveValue('keyboard draft');
      await expect(column.locator('.post-card').filter({ hasText: 'keyboard draft' })).toHaveCount(0);
    });
  }
}

test('ja dark: Esc inside the text closes the composer, Ctrl+Enter sends once, empty send shows a reason', async ({ page }) => {
  const copy = COPY.ja;
  await seed(page, 'ja', 'dark');
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/timeline');
  const column = await openComposer(page, copy.post);
  const textarea = column.getByPlaceholder(copy.post);

  // 空のまま Ctrl+Enter: 送信されず、理由が表示される。
  await textarea.click();
  await page.keyboard.press('Control+Enter');
  await expect(column.getByText(copy.emptyDraft, { exact: true })).toBeVisible();
  await expect(column.getByPlaceholder(copy.post)).toBeVisible();

  // 本文に focus がある状態の Esc で閉じ、下書きは保持される。
  await textarea.fill('esc keeps me');
  await page.keyboard.press('Escape');
  await expect(column.getByPlaceholder(copy.post)).toBeHidden();
  await expect(column.locator('.shell-column-primary-action')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(column.getByPlaceholder(copy.post)).toHaveValue('esc keeps me');

  // Ctrl+Enter は送信可能な状態で 1 回だけ送信し、投稿が 1 件だけ現れる。
  await column.getByPlaceholder(copy.post).fill('ctrl enter once');
  await page.keyboard.press('Control+Enter');
  await expect(column.getByPlaceholder(copy.post)).toBeHidden();
  await expect(column.locator('.post-card').filter({ hasText: 'ctrl enter once' })).toHaveCount(1);
});

test('en dark: keyboard guidance is reachable from the Control Center and the composer hint by pointer and keyboard', async ({ page }) => {
  const copy = COPY.en;
  await seed(page, 'en', 'dark');
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/timeline');

  // pointer: Control Center > Keyboard
  await page.getByTestId('control-center-trigger').click();
  const controlCenter = page.getByRole('complementary', { name: 'Control Center' });
  await controlCenter.getByRole('button', { name: copy.keyboardSection, exact: true }).click();
  const settings = page.getByRole('dialog', { name: copy.settings, exact: true });
  await expect(settings).toBeVisible();
  await expect(settings.getByTestId('settings-section-keyboard')).toHaveAttribute('aria-current', 'location');
  await expect(settings.getByRole('heading', { name: copy.keyboardSection, exact: true })).toBeVisible();
  await expect(settings.getByRole('region', { name: 'Composer' }).locator('kbd', { hasText: 'Esc' }).first()).toBeVisible();
  await expect(settings.getByText(/Ctrl\+K, Ctrl\+N\) are not assigned yet/)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(settings).toBeHidden();

  // keyboard: composer hint link > Enter opens the guidance, Esc closes it, draft survives.
  const column = await openComposer(page, copy.post);
  await column.getByPlaceholder(copy.post).fill('help keeps draft');
  await column.getByRole('button', { name: copy.help, exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(settings).toBeVisible();
  await expect(settings.getByRole('heading', { name: copy.keyboardSection, exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(settings).toBeHidden();
  await expect(column.getByPlaceholder(copy.post)).toHaveValue('help keeps draft');
  await expect(page.getByTestId('control-center-trigger')).toBeFocused();
});

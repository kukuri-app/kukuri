import { expect, test, type Page } from '@playwright/test';

// #1840 AC-3: 公開スレッドの一覧の列・作成・詳細の列・リンクの card を browser mock 上の実操作で確かめる。

function activeColumn(page: Page, title: string) {
  return page.getByRole('region', { name: new RegExp(`^${title} Column,.*Active,`) });
}

async function openControlCenter(page: Page) {
  const controlCenter = page.getByRole('complementary', { name: 'Control Center' });
  if (!(await controlCenter.isVisible().catch(() => false))) {
    await page.getByTestId('control-center-trigger').click();
  }
  await expect(controlCenter).toBeVisible();
  return controlCenter;
}

async function openGeneralThreads(page: Page) {
  const controlCenter = await openControlCenter(page);
  await controlCenter
    .getByRole('button', { name: 'general', exact: true })
    .locator('xpath=ancestor::li[1]')
    .getByRole('button', { name: /^Public thread/ })
    .click();
  const list = activeColumn(page, 'Public threads');
  await expect(list).toBeVisible();
  await expect(page).toHaveURL(/#\/timeline\?topic=kukuri%3Atopic%3Ageneral&context=publicThreads$/);
  return list;
}

async function createThread(page: Page, title: string, firstPost: string) {
  const list = activeColumn(page, 'Public threads');
  await list.locator('.shell-column-footer').getByRole('button', { name: 'Create public thread' }).click();
  const dialog = page.getByRole('dialog', { name: 'Create a public thread' });
  await dialog.getByRole('textbox', { name: 'Title' }).fill(title);
  await dialog.getByRole('textbox', { name: 'First post (optional)' }).fill(firstPost);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(dialog).toBeHidden();
  const detail = activeColumn(page, 'Public thread');
  await expect(detail.locator('.shell-column-header')).toContainText(`${title} · general`);
  return detail;
}

test('a public thread is created from its topic, written to in its detail and found again in the list', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  const list = await openGeneralThreads(page);
  await expect(list.getByText('No public threads in this topic yet.')).toBeVisible();

  const detail = await createThread(page, 'Release notes', 'What changed in 0.5');
  await expect(detail.getByText('What changed in 0.5')).toBeVisible();
  await detail.getByRole('button', { name: 'Post to Release notes · general' }).click();
  await detail.getByPlaceholder('Write a post').fill('second post');
  await detail.locator('.shell-column-composer').getByRole('button', { name: 'Post', exact: true }).click();
  await expect(detail.getByText('second post')).toBeVisible();

  // 戻る操作で一覧の列へ戻り、作ったスレッドの行から詳細を開き直す。
  await page.goBack();
  const back = activeColumn(page, 'Public threads');
  await back.getByRole('button', { name: /^Release notes/ }).click();
  const reopened = activeColumn(page, 'Public thread');
  await expect(reopened.getByText('second post')).toBeVisible();

  // 詳細の列の「リンクをコピー」で写したリンクを topic のタイムラインに貼ると、本文の下に card が出る。
  await reopened.locator('.shell-column-header').getByRole('button', { name: 'Copy link' }).click();
  const link = await page.evaluate(() => navigator.clipboard.readText());
  expect(link).toMatch(/^#\/timeline\?topic=kukuri%3Atopic%3Ageneral&channel=pc-\d+-[0-9a-f]{16}$/);
  const timeline = page.getByRole('region', { name: /^Timeline Column,/ });
  await timeline.getByRole('button', { name: 'Post to Public · general' }).click();
  await timeline.getByPlaceholder('Write a post').fill(`Notes are here ${link}`);
  await timeline.locator('.shell-column-composer').getByRole('button', { name: 'Post', exact: true }).click();
  const shared = timeline.locator('article', { hasText: 'Notes are here' });
  await expect(shared.locator('.smart-reference-chip')).toHaveText('Release notes');
  const card = shared.locator('.public-thread-card');
  await expect(card).toContainText('Release notes');
  await expect(card).toContainText('What changed in 0.5');
  // 公開スレッドの投稿は topic のタイムラインに並ばない(本文の card の中だけに出る)。
  await expect(timeline.locator('.post-title', { hasText: 'second post' })).toHaveCount(0);

  // card から開いた詳細の列を閉じると、親の一覧の列へ戻る。
  await card.click();
  await activeColumn(page, 'Public thread').getByRole('button', { name: 'Close Public thread' }).click();
  await expect(back.getByRole('button', { name: /^Release notes/ })).toBeVisible();

  // 並び順は keyboard でも選べる。
  await back.getByRole('button', { name: 'Sort order: Latest post first' }).focus();
  await page.keyboard.press('Enter');
  const menu = page.getByRole('menu', { name: 'Sort order' });
  await expect(menu.getByRole('menuitemradio', { name: 'Latest post first' })).toHaveAttribute('aria-checked', 'true');
  await expect(menu.getByRole('menuitemradio', { name: 'Latest post first' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(back.getByRole('button', { name: 'Sort order: Newest first' })).toBeVisible();
});

test('on a narrow screen the thread detail replaces the list and closing it returns to the list', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await openGeneralThreads(page);
  const detail = await createThread(page, 'Weekly sync', '');
  await expect(detail).toBeInViewport();
  await expect(detail.getByRole('button', { name: 'Post to Weekly sync · general' })).toBeVisible();
  // 狭い幅では、列を閉じる操作は列の menu の中にある。
  await detail.getByRole('button', { name: 'Open Public thread menu' }).click();
  await page.getByRole('menuitem', { name: 'Close Public thread' }).click();
  const list = activeColumn(page, 'Public threads');
  await expect(list).toBeInViewport();
  await expect(list.getByRole('button', { name: /^Weekly sync/ })).toBeVisible();
});

import { expect, test, type Page } from '@playwright/test';

// Issue #1053: Timeline Column を選択した直後に別 Column のボタンを押すと、Timeline Column が
// 新しく開いて押した操作が実行されない不具合の browser 裏付け。jsdom は layout を持たないため、
// Column 追加に伴う scroll で click が失われるかどうかは実 browser でのみ確認できる。

const GENERAL_TIMELINE_URL = /#\/timeline\?topic=kukuri%3Atopic%3Ageneral$/;
const DEV_TIMELINE_URL = /#\/timeline\?topic=kukuri%3Atopic%3Adev$/;

function column(page: Page, title: string) {
  return page.getByRole('region', { name: new RegExp(`^${title} Column,`) });
}

// Column の title 行(非 interactive)を押して Column を選択する。
async function selectColumn(page: Page, title: string, url: RegExp) {
  const target = column(page, title);
  await target.locator('.shell-column-title-row').click();
  await expect(target).toHaveAttribute('aria-current', 'true');
  await expect(page).toHaveURL(url);
}

async function openWorkspaceWithSwitchedTimeline(page: Page) {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await expect(column(page, 'Timeline')).toBeVisible();
  await column(page, 'Timeline')
    .getByRole('combobox', { name: 'Column topic' })
    .selectOption('kukuri:topic:dev');
  await expect(page).toHaveURL(DEV_TIMELINE_URL);
  await selectColumn(page, 'Profile', /#\/profile\?topic=kukuri%3Atopic%3Ageneral$/);
  await selectColumn(page, 'Timeline', DEV_TIMELINE_URL);
  await expect(page.locator('[data-column-id]')).toHaveCount(5);
}

test('a body button runs and focuses its Column right after the switched Timeline was selected', async ({ page }) => {
  await openWorkspaceWithSwitchedTimeline(page);

  const explore = column(page, 'Explore');
  const discover = explore.getByRole('tab', { name: 'Discover' });
  await discover.click();

  await expect(page.getByRole('region', { name: /^Timeline Column,/ })).toHaveCount(1);
  await expect(page.locator('[data-column-id]')).toHaveCount(5);
  await expect(discover).toHaveAttribute('aria-selected', 'true');
  await expect(explore).toHaveAttribute('aria-current', 'true');
  await expect(page).toHaveURL(/#\/explore\?topic=kukuri%3Atopic%3Ageneral$/);
});

test('a header button focuses its Column right after the Timeline was selected', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await expect(column(page, 'Notifications')).toBeVisible();
  await selectColumn(page, 'Profile', /#\/profile\?topic=kukuri%3Atopic%3Ageneral$/);
  await selectColumn(page, 'Timeline', GENERAL_TIMELINE_URL);

  const notifications = column(page, 'Notifications');
  await notifications
    .locator('.shell-column-header')
    .getByRole('button', { name: 'Refresh' })
    .click();

  await expect(notifications).toHaveAttribute('aria-current', 'true');
  await expect(page).toHaveURL(/#\/notifications\?topic=kukuri%3Atopic%3Ageneral$/);
  await expect(page.locator('[data-column-id]')).toHaveCount(5);
});

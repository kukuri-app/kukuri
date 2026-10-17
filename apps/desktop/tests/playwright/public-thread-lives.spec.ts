import { expect, test, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

// #1840 AC-4: 公開スレッドの詳細の列の上部の「ライブ」から Live を始め、投稿が流れて開始 post が本文の外へ出ても、上部の行から
// その Live を「ライブ」の列で開ける(Live の画面は開発者モードで出す)。

test.beforeEach(async ({ page }) => {
  await page.addInitScript((key) => {
    window.localStorage.setItem(key, 'true');
  }, DEVELOPER_MODE_STORAGE_KEY);
});

function activeColumn(page: Page, title: string) {
  return page.getByRole('region', { name: new RegExp(`^${title} Column,.*Active,`) });
}

test('a live started in a public thread stays reachable from the detail header after its start post scrolls away', async ({ page }) => {
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&context=publicThreads');
  const list = activeColumn(page, 'Public threads');
  await list.locator('.shell-column-footer').getByRole('button', { name: 'Create public thread' }).click();
  const create = page.getByRole('dialog', { name: 'Create a public thread' });
  await create.getByRole('textbox', { name: 'Title' }).fill('Watch party');
  await create.getByRole('button', { name: 'Create' }).click();
  const detail = activeColumn(page, 'Public thread');
  await expect(detail.locator('.shell-column-header')).toContainText('Watch party · general');

  // 上部の「ライブ」から、スレッドの「ライブ」の列を開き、下部の「ライブを開始」で始める。
  await detail.locator('.shell-column-header').getByRole('button', { name: 'Live', exact: true }).click();
  const live = activeColumn(page, 'Live');
  await expect(live.locator('.shell-column-header')).toContainText('Watch party · general');
  await live.locator('.shell-column-footer').getByRole('button', { name: 'Start Live' }).click();
  const start = page.getByRole('dialog', { name: 'Start Live' });
  await start.getByPlaceholder('Friday stream').fill('Night stream');
  await start.getByRole('button', { name: 'Start Live' }).click();
  await expect(start).toBeHidden();
  await expect(live.getByText('Night stream')).toBeVisible();
  await live.getByRole('button', { name: 'Close Live' }).click();

  const back = activeColumn(page, 'Public thread');
  const row = back.getByRole('list', { name: 'Live now' }).getByRole('button', { name: /^Live Night stream/ });
  await expect(row).toBeVisible();

  // 投稿を重ねて、開始 post を本文の外へ流す。上部の行は本文と一緒に流れない。
  for (let index = 1; index <= 8; index += 1) {
    const compose = back.getByRole('button', { name: 'Post to Watch party · general' });
    if (await compose.isVisible()) await compose.click();
    await back.getByPlaceholder('Write a post').fill(`message ${index}`);
    await back.locator('.shell-column-composer').getByRole('button', { name: 'Post', exact: true }).click();
    await expect(back.getByText(`message ${index}`, { exact: true })).toBeVisible();
  }
  await back.locator('.shell-column-body').evaluate((body) => body.scrollTo({ top: 0 }));
  await expect(back.locator('.post-live')).not.toBeInViewport();
  await expect(row).toBeInViewport();

  await row.click();
  const opened = activeColumn(page, 'Live');
  await expect(opened.locator('.post-card-targeted')).toContainText('Night stream');
});

import { expect, test, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

// 既存フローは Live/Game タブなど WIP 面の表示を前提とするため developer mode を有効化する。
test.beforeEach(async ({ page }) => {
  await page.addInitScript((key) => {
    window.localStorage.setItem(key, 'true');
  }, DEVELOPER_MODE_STORAGE_KEY);
});

async function openChannelManager(page: Page) {
  const dialog = page.getByRole('dialog', { name: 'Create / Join Private Channel' });
  if (await dialog.isVisible().catch(() => false)) {
    return dialog;
  }
  const controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name: 'Create or join a private channel' }).click();
  await expect(dialog).toBeVisible();
  return dialog;
}

async function openChannelSettings(page: Page, channelLabel: string) {
  const controlCenter = await openControlCenter(page);
  await controlCenter
    .getByRole('button', { name: `Open ${channelLabel} channel settings` })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Channel Settings' });
  await expect(dialog).toBeVisible();
  return dialog;
}

async function openControlCenter(page: Page) {
  const controlCenter = page.getByRole('complementary', { name: 'Control Center' });
  if (!(await controlCenter.isVisible().catch(() => false))) {
    await page.getByTestId('control-center-trigger').click();
  }
  await expect(controlCenter).toBeVisible();
  return controlCenter;
}

function activeColumn(page: Page, title: string) {
  return page.getByRole('region', {
    name: new RegExp(`^${title} Column,.*Active,`),
  });
}

async function openLiveCreateDialog(page: Page) {
  await activeColumn(page, 'Timeline').getByRole('button', { name: /^Post to / }).click();
  await activeColumn(page, 'Timeline').getByRole('button', { name: 'Start Live' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  return dialog;
}

test('browser mock shell can run profile, private channel, live, and game flows', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 980 });
  await page.goto('/');

  const channelDialog = await openChannelManager(page);
  await channelDialog.getByPlaceholder('Channel name').fill('Core Contributors');
  await channelDialog.getByRole('button', { name: 'Create Channel' }).click();
  await expect(page).toHaveURL(/#\/timeline\?topic=.*&channel=channel-1/);
  await page.keyboard.press('Escape');

  const channelSettingsDialog = await openChannelSettings(page, 'Core Contributors');
  await channelSettingsDialog.getByRole('button', { name: 'Create share link' }).click();
  await expect(channelSettingsDialog.getByText('Copy share link')).toBeVisible();
  await expect(channelSettingsDialog.getByRole('button', { name: 'Copy link' })).toBeVisible();
  await page.keyboard.press('Escape');

  const joinDialog = await openChannelManager(page);
  await joinDialog
    .getByPlaceholder('Paste a private channel invite, mutual grant, or mutuals+ share')
    .fill('invite-token');
  await joinDialog.getByRole('button', { name: 'Join' }).click();
  await expect(page).toHaveURL(
    /#\/timeline\?topic=kukuri%3Atopic%3Ageneral&channel=channel-imported/
  );
  await page.keyboard.press('Escape');

  await page.goto('/#/timeline');
  const liveDialog = await openLiveCreateDialog(page);
  const liveTitle = liveDialog.getByLabel('Live Title');
  const liveDescription = liveDialog.getByLabel('Live Description');
  await liveTitle.fill('Launch Party');
  await expect(liveTitle).toHaveValue('Launch Party');
  await liveDescription.fill('watch along');
  await expect(liveDescription).toHaveValue('watch along');
  await liveDialog.getByRole('button', { name: 'Start Live' }).click();
  // #1839: 開始 post の中にも同じ Live が出るので、Live の一覧のカードに絞る。
  const liveTile = activeColumn(page, 'Timeline')
    .locator('article[data-live-session-id]')
    .filter({ has: page.getByText('Launch Party') });
  await liveTile.getByRole('button').click();
  const liveCard = page.getByRole('dialog', { name: 'Launch Party' });
  await expect(liveCard).toBeVisible();
  // #1034・#1839 AC-3: 主催者の card に配信の開始がある(取り込みは押した後だけで、配信中の操作は shell の上部)。
  await expect(liveCard.getByRole('button', { name: 'Broadcast camera' })).toBeVisible();
  await expect(liveCard.getByRole('button', { name: 'Broadcast screen' })).toBeVisible();
  await liveCard.getByRole('button', { name: 'Join', exact: true }).click();
  await expect(liveCard.getByText('viewers: 1')).toBeVisible();
  await liveCard.getByRole('button', { name: 'End', exact: true }).click();
  await expect(liveCard.getByText('Ended', { exact: true })).toBeVisible();
  await liveCard.getByRole('button', { name: 'Close dialog' }).click();

  await page.goto('/#/game');
  await expect(page.getByRole('heading', { name: 'Metaverse Rooms' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Game Rooms' })).toHaveCount(0);
  await expect(page.getByText('No game rooms', { exact: true })).toHaveCount(0);
  await expect(
    activeColumn(page, 'Metaverse').getByRole('button', { name: 'Create Metaverse Room' })
  ).toBeVisible();

  await page.goto('/#/profile?profileMode=edit');
  await page.getByPlaceholder('Visible label').fill('Browser Author');
  await page.getByRole('button', { name: 'Save Profile' }).click();
  await expect(activeColumn(page, 'Profile').getByText('Browser Author')).toBeVisible();
});

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

async function openComposerDialog(page: Page) {
  await activeColumn(page, 'Timeline').getByRole('button', { name: /^Post to / }).click();
  await expect(page.getByPlaceholder('Write a post')).toBeVisible();
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

test('browser mock hash routes deep link profile, notifications, timeline normalization, and settings surfaces', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 980 });

  await page.goto('/#/profile');
  await expect(page.getByRole('button', { name: 'Edit Profile' })).toBeVisible();
  await expect(page).toHaveURL(/#\/profile\?topic=/);

  await page.goto('/#/channels');
  await expect(page.getByTestId('control-center-trigger')).toBeVisible();
  await expect(page).toHaveURL(/#\/timeline\?topic=/);

  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance');
  await expect(page).toHaveURL(/#\/timeline\?topic=kukuri%3Atopic%3Ageneral&settings=appearance/);
  const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(settingsDialog).toBeVisible({ timeout: 10000 });
  await expect(settingsDialog.getByTestId('settings-section-appearance')).toHaveAttribute(
    'aria-current',
    'location'
  );
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  await settingsDialog.getByTestId('settings-section-connectivity').click();
  await expect(page).toHaveURL(/settings=connectivity/);
  await page.keyboard.press('Escape');
  await expect(settingsDialog).not.toBeVisible();

  await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
  await expect(activeColumn(page, 'Notifications')).toBeVisible();
  await expect(page.getByText('browser mock reply notification')).toBeVisible();

  await page.getByText('browser mock reply notification').click();
  await expect(page).toHaveURL(
    /#\/timeline\?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=browser-seed-post/
  );
  await expect(activeColumn(page, 'Thread')).toBeVisible();
});

test('browser mock hash history keeps route state stable without narrow-width overflow', async ({
  page,
}) => {
  await page.setViewportSize({ width: 700, height: 980 });
  await page.goto('/');

  const controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name: 'Add Profile Column' }).click();
  await expect(page.getByRole('button', { name: 'Edit Profile' })).toBeVisible();
  await expect(page).toHaveURL(/#\/profile\?topic=/);

  await page.goBack();
  await expect(page).toHaveURL(/#\/timeline\?topic=/);
  await openComposerDialog(page);
  await expect(page.getByPlaceholder('Write a post')).toBeVisible();

  await page.getByPlaceholder('Write a post').fill('route history post');
  await page.getByRole('button', { name: 'Post', exact: true }).click();
  await expect(activeColumn(page, 'Timeline').getByText('route history post')).toBeVisible();

  await activeColumn(page, 'Timeline').getByText('route history post').click();
  await expect(page).toHaveURL(/context=thread/);
  await expect(activeColumn(page, 'Thread')).toBeVisible();

  await page.goBack();
  await expect(page).not.toHaveURL(/context=thread/);
  await openComposerDialog(page);
  await expect(page.getByPlaceholder('Write a post')).toBeVisible();
  await page.keyboard.press('Escape');

  await page.goForward();
  await expect(page).toHaveURL(/context=thread/);
  await expect(activeColumn(page, 'Thread')).toBeVisible();

  await page.goBack();
  await openControlCenter(page);
  const channelDialog = await openChannelManager(page);
  await channelDialog.getByPlaceholder('Channel name').fill('Route Room');
  await channelDialog.getByRole('button', { name: 'Create Channel' }).click();
  await expect(page).toHaveURL(/#\/timeline\?topic=.*&channel=channel-/);

  const noOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth <= window.innerWidth
  );
  expect(noOverflow).toBeTruthy();
});

test('production Columns retain a pinned causal chain and close back through its parents', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/');

  const timelineColumn = activeColumn(page, 'Timeline');
  await expect(timelineColumn).toBeVisible();
  await timelineColumn.getByText('browser mock peer post').click();

  const threadColumn = activeColumn(page, 'Thread');
  await expect(threadColumn).toBeVisible();
  await expect(page.getByRole('region', { name: /^Timeline Column,/ })).toBeVisible();
  await threadColumn.getByRole('button', { name: 'Pin Thread' }).click();
  await expect(threadColumn.getByRole('button', { name: 'Unpin Thread' })).toBeVisible();

  await threadColumn.getByRole('button', { name: 'browser peer' }).first().click();
  const profileColumn = activeColumn(page, 'Profile');
  await expect(profileColumn).toBeVisible();
  await expect(page.getByRole('region', { name: /^Thread Column,.*Pinned$/ })).toBeVisible();
  await expect
    .poll(() =>
      profileColumn
        .locator('.shell-column-body')
        .evaluate((element) => element.scrollWidth - element.clientWidth)
    )
    .toBeLessThanOrEqual(0);
  await expect
    .poll(async () => {
      const bodyBox = await profileColumn.locator('.shell-column-body').boundingBox();
      const actionsBox = await profileColumn.locator('.author-detail-action-buttons').boundingBox();
      return actionsBox && bodyBox ? actionsBox.x + actionsBox.width - (bodyBox.x + bodyBox.width) : 1;
    })
    .toBeLessThanOrEqual(0);
  await expect
    .poll(() =>
      profileColumn.locator('.shell-column-body').evaluate((body) => {
        const bodyRight = body.getBoundingClientRect().right;
        return Math.max(
          0,
          ...Array.from(body.querySelectorAll<HTMLElement>('*'), (element) =>
            element.getBoundingClientRect().right - bodyRight
          )
        );
      })
    )
    .toBeLessThanOrEqual(0.5);
  await expect
    .poll(async () => {
      const canvasBox = await page.locator('.shell-column-canvas').boundingBox();
      const profileBox = await profileColumn.boundingBox();
      return canvasBox && profileBox
        ? profileBox.x + profileBox.width - (canvasBox.x + canvasBox.width)
        : 1;
    })
    .toBeLessThanOrEqual(0);

  await profileColumn.getByRole('button', { name: 'Close Profile' }).click();
  await expect(activeColumn(page, 'Thread')).toBeVisible();
  await expect(page).toHaveURL(/context=thread/);

  await activeColumn(page, 'Thread').getByRole('button', { name: 'Close Thread' }).click();
  await expect(activeColumn(page, 'Timeline')).toBeVisible();
  await expect(page).not.toHaveURL(/context=thread/);
});

test('developer mode off keeps the live deep link', async ({ page }) => {
  // beforeEach の seed より後に登録した init script が勝つため、既定 OFF を再現できる。
  await page.addInitScript((key) => {
    window.localStorage.setItem(key, 'false');
  }, DEVELOPER_MODE_STORAGE_KEY);
  await page.setViewportSize({ width: 1400, height: 980 });

  await page.goto('/#/live');
  await expect(page).toHaveURL(/#\/live\?topic=/);
  const controlCenter = await openControlCenter(page);
  await expect(controlCenter.getByRole('button', { name: 'Add Live Sessions Column' })).toHaveCount(0);
  await expect(activeColumn(page, 'Live')).toBeVisible();
});

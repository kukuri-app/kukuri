import { expect, test, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

test.beforeEach(async ({ page }) => {
  await page.addInitScript((key) => {
    window.localStorage.setItem(key, 'true');
  }, DEVELOPER_MODE_STORAGE_KEY);
});

function activeColumn(page: Page, title: string) {
  return page.getByRole('region', {
    name: new RegExp(`^${title} Column,.*Active,`),
  });
}

test('icon-only controls expose the same localized action on hover and focus', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 760 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  const timeline = activeColumn(page, 'Timeline');
  const feed = timeline.getByRole('tab', { name: 'Feed' });
  await feed.hover();
  await expect(page.getByRole('tooltip')).toHaveText('Feed');

  await page.keyboard.press('Escape');
  await expect(page.getByRole('tooltip')).toHaveCount(0);

  await feed.focus();
  await expect(feed).toBeFocused();
  await expect(page.getByRole('tooltip')).toHaveText('Feed');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('tooltip')).toHaveCount(0);

  await page.goto('/#/notifications?topic=kukuri%3Atopic%3Ageneral');
  const notifications = activeColumn(page, 'Notifications');
  const header = notifications.locator('.shell-column-header');
  await expect(header).toContainText(/notification.*unread/);
  await expect(header.getByRole('button', { name: 'Refresh' })).toBeVisible();
  await expect(notifications.locator('.shell-column-body .shell-workspace-header')).toHaveCount(0);
});

// #1210: Control Center の topic 追加ボタンで tooltip が見えなかった。tooltip は portal で
// body 直下へ出るため、overlay surface より手前に積まれていないと panel の背後へ隠れる。
// DOM に存在するだけでは再現を捉えられないので、hit test で最前面かどうかを確認する。
test('Control Center icon tooltips render in front of the panel', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  await page.getByTestId('control-center-trigger').click();
  const controlCenter = page.getByRole('complementary', { name: 'Control Center' });
  await expect(controlCenter).toBeVisible();

  const addTopic = controlCenter.getByRole('button', { name: 'Add Topic', exact: true });
  await addTopic.hover();
  const tooltip = page.getByRole('tooltip');
  await expect(tooltip).toHaveText('Add Topic');

  await expect
    .poll(() =>
      tooltip.evaluate((node) => {
        const rect = node.getBoundingClientRect();
        const hit = document.elementFromPoint(
          rect.left + rect.width / 2,
          rect.top + rect.height / 2
        );
        return hit === node || node.contains(hit);
      })
    )
    .toBe(true);
});

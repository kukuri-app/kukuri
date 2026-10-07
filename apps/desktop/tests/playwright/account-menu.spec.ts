import { expect, test } from '@playwright/test';

test('account menu opens own profile and add dialog using pointer and keyboard', async ({ page }) => {
  await page.goto('/');
  const trigger = page.getByTestId('account-menu-trigger');
  await trigger.click();
  const menu = page.getByRole('menu', { name: 'Account menu' });
  await expect(menu.getByRole('menuitem').first()).toHaveText('View profile');
  await expect(menu.getByRole('menuitemradio').first()).toBeVisible();
  await menu.getByRole('menuitem', { name: 'View profile' }).click();
  await expect(page.locator('[data-column-id]:focus')).toHaveAttribute('aria-label', /^Profile/);
  const count = await page.locator('[data-column-id]').count();
  await trigger.focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await expect(page.locator('[data-column-id]')).toHaveCount(count);
  await trigger.click();
  await menu.getByRole('menuitem', { name: 'Add account' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add account' });
  await expect(dialog.getByRole('button', { name: 'Create a new account' })).toBeEnabled();
  await expect(dialog.getByTestId('import-input')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(trigger).toBeFocused();
});

test('logout cancellation retains the account in the menu', async ({ page }) => {
  await page.goto('/');
  const trigger = page.getByTestId('account-menu-trigger');
  await trigger.click();
  const menu = page.getByRole('menu', { name: 'Account menu' });
  await expect(menu.getByRole('menuitemradio')).toHaveCount(1);
  await menu.getByRole('menuitem', { name: 'Log out' }).click();
  const dialog = page.getByRole('dialog', { name: 'Log out of this account?' });
  await expect(dialog.getByText(/Local data stays here/)).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await trigger.click();
  await expect(menu.getByRole('menuitemradio')).toHaveCount(1);
});

// #1650: 使用中の行の同期のボタンは、tooltip と keyboard（行からの下の矢印）でも届き、同期の dialog を開く。mock の相手の
// 端末とつながって投稿まで受けて完了し、閉じたら menu の入口へ focus を戻す（keyboard で開いた dialog の最初の Escape は、
// 閉じるボタンの tooltip を閉じる。既存の dialog と同じ）。
test('the current account row opens the profile sync across devices', async ({ page }) => {
  await page.goto('/');
  const trigger = page.getByTestId('account-menu-trigger');
  await trigger.click();
  const menu = page.getByRole('menu', { name: 'Account menu' });
  const sync = menu.getByRole('menuitem', { name: 'Sync profile across devices' });
  await sync.hover();
  await expect(page.getByRole('tooltip', { name: 'Sync profile across devices' })).toBeVisible();
  await menu.getByRole('menuitemradio').focus();
  await page.keyboard.press('ArrowDown');
  await expect(sync).toBeFocused();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'Sync profile across devices' });
  await expect(dialog.getByText(/On the other device, also choose/)).toBeVisible();
  await expect(dialog.getByTestId('account-transfer-history-result')).toBeVisible({ timeout: 10_000 });
  await dialog.getByRole('button', { name: 'Close dialog' }).click();
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
});

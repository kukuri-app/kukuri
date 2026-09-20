import { expect, test } from '@playwright/test';

const image = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a/A8AAAAASUVORK5CYII=', 'base64');

test('settings language localizes Explore and the file control across reloads', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/timeline?settings=appearance');
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByLabel('Language').selectOption('ja');
  await page.keyboard.press('Escape');
  await page.goto('/#/explore');
  // #1192: カラム見出しと重複するカード見出しを外したため、機能タブの accessible name で確認する。
  await expect(page.getByRole('tablist', { name: 'コミュニティインデックスの機能' })).toBeVisible();
  await expect(page.getByRole('tab', { name: '検索', exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('tablist', { name: 'コミュニティインデックスの機能' })).toBeVisible();
  await page.goto('/#/timeline');
  await page.locator('[data-column-id][aria-current="true"] .shell-column-primary-action').click();
  await expect(page.getByRole('button', { name: 'ファイルを選択', exact: true })).toBeVisible();
  await expect(page.getByText('ファイル未選択', { exact: true })).toBeVisible();
});

for (const locale of [
  { code: 'ja', choose: 'ファイルを選択', empty: 'ファイル未選択', selected: '添付 1 件', remove: '削除' },
  { code: 'en', choose: 'Choose files', empty: 'No files selected', selected: 'Attached files: 1', remove: 'Remove' },
  { code: 'zh-CN', choose: '选择文件', empty: '未选择文件', selected: '已附加 1 个文件', remove: '移除' },
]) {
  for (const theme of ['dark', 'light']) {
    test(`${locale.code} ${theme} file picker supports pointer, keyboard, reset and narrow layouts`, async ({ page }, testInfo) => {
      await page.addInitScript(({ code, theme }) => {
        localStorage.setItem('kukuri.desktop.locale', code);
        localStorage.setItem('kukuri.desktop.theme', theme);
      }, { code: locale.code, theme });
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto('/#/timeline');
      await page.locator('[data-column-id][aria-current="true"] .shell-column-primary-action').click();
      const composer = page.locator('.composer');
      const button = composer.getByRole('button', { name: locale.choose, exact: true });
      await expect(composer.getByText(locale.empty, { exact: true })).toBeVisible();
      await expect(composer.locator('input[type=file]')).toBeHidden();
      await composer.locator('textarea').fill('preserved draft');
      await testInfo.attach('empty', { body: await composer.screenshot(), contentType: 'image/png' });
      const file = { name: '長いファイル名'.repeat(12) + '.png', mimeType: 'image/png', buffer: image };
      for (const action of ['pointer', 'Enter', 'Space']) {
        const chooserPromise = page.waitForEvent('filechooser');
        if (action === 'pointer') await button.click();
        else { await button.focus(); await button.press(action); }
        const chooser = await chooserPromise;
        expect(chooser.isMultiple()).toBe(true);
        await chooser.setFiles(file);
        await expect(composer.getByText(locale.selected, { exact: true })).toBeVisible();
        await expect(composer.locator('input[type=file]')).toHaveValue('');
        await expect(composer.locator('textarea')).toHaveValue('preserved draft');
        await expect(button).toBeFocused();
        for (const width of [1280, 900, 390]) {
          await page.setViewportSize({ width, height: 800 });
          await expect(button).toBeVisible();
          expect(await composer.evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
        }
        await testInfo.attach(`selected-${action}`, { body: await composer.screenshot(), contentType: 'image/png' });
        await composer.getByRole('button', { name: locale.remove, exact: true }).click();
        await expect(composer.getByText(locale.empty, { exact: true })).toBeVisible();
      }
    });
  }
}

// #965: 対応形式は選ぶ前に見え、非対応ファイルを選ぶと理由が composer 内に出て下書きは残る。
for (const locale of [
  {
    code: 'ja',
    choose: 'ファイルを選択',
    empty: 'ファイル未選択',
    selected: '添付 1 件',
    guidance: '画像と動画のみ添付できます。テキストや PDF などのファイルは添付できません。',
    rejected: '「notes.txt」ほか 1 件は添付できません。画像と動画のみ添付できます。',
    rejectedOne: '「notes.txt」は添付できません。画像と動画のみ添付できます。',
  },
  {
    code: 'en',
    choose: 'Choose files',
    empty: 'No files selected',
    selected: 'Attached files: 1',
    guidance: 'Only images and videos can be attached. Text, PDF, and other files are not supported.',
    rejected: 'The file “notes.txt” and 1 more cannot be attached. Only images and videos can be attached.',
    rejectedOne: 'The file “notes.txt” cannot be attached. Only images and videos can be attached.',
  },
]) {
  test(`${locale.code} non-media selection shows a reason with supported formats and keeps the draft`, async ({ page }, testInfo) => {
    await page.addInitScript((code) => {
      localStorage.setItem('kukuri.desktop.locale', code);
    }, locale.code);
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto('/#/timeline');
    await page.locator('[data-column-id][aria-current="true"] .shell-column-primary-action').click();
    const composer = page.locator('.composer');
    const button = composer.getByRole('button', { name: locale.choose, exact: true });
    await expect(composer.getByText(locale.guidance, { exact: true })).toBeVisible();
    await expect(button).toHaveAccessibleDescription(`${locale.empty} ${locale.guidance}`);
    await expect(composer.locator('input[type=file]')).toHaveAttribute('accept', 'image/*,video/*');
    await composer.locator('textarea').fill('preserved draft');

    const chooserPromise = page.waitForEvent('filechooser');
    await button.click();
    const chooser = await chooserPromise;
    await chooser.setFiles([
      { name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('plain text') },
      { name: 'photo.png', mimeType: 'image/png', buffer: image },
      { name: 'report.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF-1.4') },
    ]);
    const alert = composer.getByRole('alert');
    await expect(alert).toHaveText(locale.rejected);
    await expect(composer.getByText(locale.selected, { exact: true })).toBeVisible();
    await expect(composer.getByText('photo.png', { exact: true })).toBeVisible();
    await expect(composer.getByText(locale.guidance, { exact: true })).toBeVisible();
    await expect(composer.locator('textarea')).toHaveValue('preserved draft');
    await expect(composer.locator('input[type=file]')).toHaveValue('');
    await expect(button).toBeFocused();
    await testInfo.attach(`rejected-${locale.code}`, { body: await composer.screenshot(), contentType: 'image/png' });

    const secondChooser = page.waitForEvent('filechooser');
    await button.click();
    await (await secondChooser).setFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('plain text') });
    await expect(alert).toHaveText(locale.rejectedOne);
    await expect(composer.getByText(locale.selected, { exact: true })).toBeVisible();

    await composer.locator('textarea').press('End');
    await composer.locator('textarea').type(' edited');
    await expect(composer.getByRole('alert')).toHaveCount(0);
    await expect(composer.locator('textarea')).toHaveValue('preserved draft edited');
    await expect(composer.getByText(locale.selected, { exact: true })).toBeVisible();
  });
}

test('clipboard image paste creates a local draft before explicit publish', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('kukuri.desktop.locale', 'en');
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/#/timeline');
  await page.locator('[data-column-id][aria-current="true"] .shell-column-primary-action').click();
  const composer = page.locator('.composer');
  const textarea = composer.locator('textarea');
  await textarea.fill('clipboard image draft');

  const prevented = await textarea.evaluate((element, bytes) => {
    const clipboard = new DataTransfer();
    clipboard.setData('text/plain', 'must not be inserted with the image');
    clipboard.items.add(
      new File([Uint8Array.from(bytes)], '', {
        type: 'image/png',
        lastModified: 42,
      })
    );
    return !element.dispatchEvent(
      new ClipboardEvent('paste', {
        bubbles: true,
        cancelable: true,
        clipboardData: clipboard,
      })
    );
  }, Array.from(image));

  expect(prevented).toBe(true);
  await expect(composer.getByText('Attached files: 1', { exact: true })).toBeVisible();
  await expect(composer.getByText('clipboard-image.png', { exact: true })).toBeVisible();
  await expect(textarea).toHaveValue('clipboard image draft');
  await expect(
    page.locator('.post-card').filter({ hasText: 'clipboard image draft' })
  ).toHaveCount(0);

  await composer.getByRole('button', { name: 'Post', exact: true }).click();
  await expect(
    page
      .locator('[data-column-id][aria-current="true"] .post-card')
      .filter({ hasText: 'clipboard image draft' })
  ).toHaveCount(1);
});

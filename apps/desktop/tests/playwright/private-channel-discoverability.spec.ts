import { expect, test, type Locator, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';
import { DESKTOP_THEME_STORAGE_KEY } from '../../src/lib/theme';

// Issue #966: 開発者モード OFF・未参加の通常画面から、プライベートチャンネルの存在と
// 作成・招待による参加・招待共有の入口へ実 pointer / keyboard で到達できることを固定する。
// browser(Chromium)の結果は Linux .deb 実機観測を置き換えない(docs/progress の #966 記録を参照)。

const COPY = {
  ja: {
    entry: 'チャンネル作成・参加',
    entryText: 'プライベートチャンネル',
    dialog: 'プライベートチャンネル作成 / 参加',
    intro: 'プライベートチャンネルは、このトピックの中で参加者だけが投稿を読み書きできる範囲です。',
    joinHint: '招待・共有リンク（またはトークン）は、チャンネルの参加者から受け取ります。',
    channelName: 'チャンネル名',
    create: 'チャンネルを作成',
    closeDialog: 'ダイアログを閉じる',
    settingsEntry: 'core のチャンネル設定と共有を開く',
    settingsDialog: 'チャンネル設定',
    shareLink: '共有リンク作成',
    copyShareLink: '共有リンクをコピーする',
    shareHint: '共有リンクは招待したい相手にだけ送ってください。',
    controlCenter: 'コントロールセンター',
    ccCreateJoin: 'チャンネル作成・参加',
    ccEmpty: '参加済みのプライベートチャンネルはありません。',
    ccEmptyAction: '作成または参加',
    ccShare: '選択中のチャンネルを共有',
    ccShareHint: '参加中のチャンネルを選ぶと、招待・共有リンクを作成できます。',
  },
  en: {
    entry: 'Create or join a private channel',
    entryText: 'Private channel',
    dialog: 'Create / Join Private Channel',
    intro: 'A private channel is a space inside this topic where only participants can read and write posts.',
    joinHint: 'Invite and share links (or tokens) come from a channel participant.',
    channelName: 'Channel name',
    create: 'Create Channel',
    closeDialog: 'Close dialog',
    settingsEntry: 'Open core channel settings and sharing',
    settingsDialog: 'Channel Settings',
    shareLink: 'Create share link',
    copyShareLink: 'Copy share link',
    shareHint: 'Send the share link only to people you want to invite.',
    controlCenter: 'Control Center',
    ccCreateJoin: 'Create or join a private channel',
    ccEmpty: 'No joined private channels.',
    ccEmptyAction: 'Create or join',
    ccShare: 'Share active channel',
    ccShareHint: 'Select a joined channel to create invite or share links.',
  },
} as const;

const TIMELINE_URL = '/#/timeline?topic=kukuri%3Atopic%3Ageneral';

async function seed(page: Page, locale: keyof typeof COPY, theme: 'dark' | 'light') {
  await page.addInitScript(
    ({ locale, theme, themeKey, developerKey }) => {
      window.localStorage.setItem('kukuri.desktop.locale', locale);
      window.localStorage.setItem(themeKey, theme);
      window.localStorage.setItem(developerKey, 'false');
    },
    { locale, theme, themeKey: DESKTOP_THEME_STORAGE_KEY, developerKey: DEVELOPER_MODE_STORAGE_KEY }
  );
}

function activeColumn(page: Page) {
  return page.locator('[data-column-id][aria-current="true"]');
}

async function expectNoDocumentOverflow(page: Page) {
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= document.documentElement.clientWidth
    )
  ).toBe(true);
}

async function expectWithinViewport(target: Locator) {
  const inside = await target.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.left >= 0 && rect.right <= window.innerWidth + 0.5;
  });
  expect(inside).toBe(true);
}

for (const locale of ['ja', 'en'] as const) {
  const copy = COPY[locale];
  for (const theme of ['dark', 'light'] as const) {
    for (const width of [1280, 390] as const) {
      test(`private channel entry, guidance, and sharing reachable by pointer (${locale} ${theme} ${width})`, async ({
        page,
      }) => {
        await seed(page, locale, theme);
        await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
        await page.goto(TIMELINE_URL);
        await expect(page.locator('html')).toHaveAttribute('lang', locale);

        // 1. 未参加の通常画面に機能の存在が読める(AC-2)。
        const column = activeColumn(page);
        const entry = column.getByRole('button', { name: copy.entry });
        await expect(entry).toBeVisible();
        await expect(entry).toHaveText(copy.entryText);
        await expect(entry).toBeInViewport();
        await expectNoDocumentOverflow(page);

        // 2. 入口から作成・参加 Dialog と説明へ到達し、閉じても scope は不変(INVAR-3)。
        await entry.click();
        const dialog = page.getByRole('dialog', { name: copy.dialog });
        await expect(dialog).toBeVisible();
        await expect(dialog.getByText(copy.intro)).toBeVisible();
        await expect(dialog.getByText(copy.joinHint)).toBeVisible();
        await expectWithinViewport(dialog);
        await expectNoDocumentOverflow(page);
        await page.keyboard.press('Escape');
        await expect(dialog).toBeHidden();
        await expect(page).toHaveURL(/#\/timeline\?topic=kukuri%3Atopic%3Ageneral$/);
        await expect(entry).toBeVisible();

        // 3. Control Center の「場所」でも空状態と開始方法が読める(AC-2)。
        await page.getByTestId('control-center-trigger').click();
        const controlCenter = page.getByRole('complementary', { name: copy.controlCenter });
        await expect(controlCenter).toBeVisible();
        await expect(controlCenter.getByRole('button', { name: copy.ccCreateJoin })).toBeVisible();
        const shareButton = controlCenter.getByRole('button', { name: copy.ccShare });
        await expect(shareButton).toBeDisabled();
        await expect(controlCenter.getByText(copy.ccShareHint)).toBeVisible();
        await expect(controlCenter.getByText(copy.ccEmpty).first()).toBeVisible();
        await controlCenter.getByRole('button', { name: copy.ccEmptyAction }).first().click();
        await expect(dialog).toBeVisible();

        // 4. 作成後は参加済み Column から設定・共有へ到達し、共有リンクを作成できる(AC-3)。
        await dialog.getByPlaceholder(copy.channelName).fill('core');
        await dialog.getByRole('button', { name: copy.create }).click();
        await expect(page).toHaveURL(/channel=channel-1/);
        await dialog.getByRole('button', { name: copy.closeDialog }).click();
        await expect(dialog).toBeHidden();
        const settingsEntry = activeColumn(page).getByRole('button', { name: copy.settingsEntry });
        await expect(settingsEntry).toBeVisible();
        await expect(settingsEntry).toBeInViewport();
        await settingsEntry.click();
        const settings = page.getByRole('dialog', { name: copy.settingsDialog });
        await expect(settings).toBeVisible();
        await expect(settings.getByText(copy.shareHint)).toBeVisible();
        await settings.getByRole('button', { name: copy.shareLink }).click();
        await expect(settings.getByText(copy.copyShareLink)).toBeVisible();
        await expectWithinViewport(settings);
        await expectNoDocumentOverflow(page);
      });
    }
  }
}

// Issue #1517: 公開の列の見出しから開いた Dialog で channel を作る・参加済みの一覧から開くと、閉じた後も
// channel の列が active で画面に入っている。入口へ focus を戻すと入口の列が active に戻り、右端の
// channel の列が画面外に残っていた(画面外の列は読み直さないため新しい投稿が出ない、#765)。
// focus も channel の列へ移す(入口に残すと、次の Tab で入口の列が active に戻る)。
test('a channel created or opened from the Timeline Column header stays active and in view (en 1280)', async ({
  page,
}) => {
  const copy = COPY.en;
  await seed(page, 'en', 'dark');
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto(TIMELINE_URL);
  const timelineColumn = (scope: string) =>
    page
      .getByRole('region', { name: /^Timeline Column,/ })
      .filter({ has: page.locator('.shell-column-header', { hasText: scope }) });
  const publicColumn = timelineColumn('Public · general');
  const channelColumn = timelineColumn('core · general');
  const dialog = page.getByRole('dialog', { name: copy.dialog });
  const expectChannelColumnActiveAfterClose = async () => {
    await expect(dialog).toBeHidden();
    // focus は閉じた後の task で戻る。戻った後の frame まで待ってから確かめる。
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)))
    );
    await expect(channelColumn).toHaveAttribute('aria-current', 'true');
    await expect(channelColumn).toBeInViewport();
    await expect(channelColumn).toBeFocused();
  };

  await publicColumn.getByRole('button', { name: copy.entry }).click();
  await dialog.getByPlaceholder(copy.channelName).fill('core');
  await dialog.getByRole('button', { name: copy.create }).click();
  await expect(page).toHaveURL(/channel=channel-1/);
  await dialog.getByRole('button', { name: copy.closeDialog }).click();
  await expectChannelColumnActiveAfterClose();

  await publicColumn.locator('.shell-column-title-row').click();
  await expect(channelColumn).not.toBeInViewport();
  await publicColumn.getByRole('button', { name: copy.entry }).click();
  await dialog.getByRole('button', { name: 'Open core' }).click();
  await expectChannelColumnActiveAfterClose();
});

// Issue #1533: 参加中の channel の行は、名前の長さによらず接続の切替・設定・退出が Control Center の
// 「場所」の枠内に見え、その位置を実 pointer で押せる。名前の横に収まらない行だけ操作が名前の下の段へ
// 移り、行の全幅にも収まらない名前は省略せずに折り返す。修正前は 1280 幅(4 列)で枠が狭く、
// 17 文字の名前で退出の button が枠の外へ切れていた。
for (const width of [1280, 390] as const) {
  test(`joined channel rows keep their actions inside the Places frame (en dark ${width})`, async ({
    page,
  }) => {
    const copy = COPY.en;
    // 17 文字の名前の段は幅と font で変わるので問わない。退出はこの行で最後に押す。
    const rows = [
      { name: 'core', actionsBelowName: false },
      { name: 'leave-me-musm876p-quarterly-planning-and-review', actionsBelowName: true },
      { name: 'leave-me-musm876p' },
    ];
    await seed(page, 'en', 'dark');
    await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
    await page.goto(TIMELINE_URL);
    await activeColumn(page).getByRole('button', { name: copy.entry }).click();
    const dialog = page.getByRole('dialog', { name: copy.dialog });
    for (const [index, { name }] of rows.entries()) {
      await dialog.getByPlaceholder(copy.channelName).fill(name);
      await dialog.getByRole('button', { name: copy.create }).click();
      await expect(page).toHaveURL(new RegExp(`channel=channel-${index + 1}`));
    }
    await dialog.getByRole('button', { name: copy.closeDialog }).click();
    await expect(dialog).toBeHidden();

    await page.getByTestId('control-center-trigger').click();
    const places = page.locator('.shell-control-center-place-list');
    for (const { name, actionsBelowName } of rows) {
      const row = places.locator('.topic-subitem-row').filter({
        has: page.getByRole('button', { name: `Leave ${name} channel`, exact: true }),
      });
      const layout = await row.evaluate((element) => {
        element.scrollIntoView({ block: 'center', inline: 'nearest' });
        const frame = element.closest('.shell-control-center-place-list')!.getBoundingClientRect();
        const inFrame = (rect: DOMRect) => rect.left >= frame.left && rect.right <= frame.right;
        const label = element.querySelector('.shell-topic-link-label')!;
        const nameBottom = element.querySelector('.topic-subitem')!.getBoundingClientRect().bottom;
        const actions = [...element.querySelectorAll('button:not(.topic-subitem)')].map((button) => {
          const rect = button.getBoundingClientRect();
          const x = rect.left + rect.width / 2;
          const y = rect.top + rect.height / 2;
          const reachable = inFrame(rect) && button.contains(document.elementFromPoint(x, y));
          return { reachable, x, y };
        });
        return {
          nameShown: inFrame(label.getBoundingClientRect()) && label.scrollWidth <= label.clientWidth,
          actionsReachable: actions.map((action) => action.reachable),
          actionsBelowName: actions.every((action) => action.y > nameBottom),
          leave: actions[actions.length - 1],
        };
      });
      expect.soft({
        name,
        nameShown: layout.nameShown,
        actionsReachable: layout.actionsReachable,
        actionsBelowName: layout.actionsBelowName,
      }).toEqual({
        name,
        nameShown: true,
        actionsReachable: [true, true, true],
        actionsBelowName: actionsBelowName ?? layout.actionsBelowName,
      });
      if (name === 'leave-me-musm876p') {
        await page.mouse.click(layout.leave.x, layout.leave.y);
      }
    }
    await expect(page.getByRole('dialog', { name: 'Leave channel' })).toBeVisible();
  });
}

test('private channel entry is reachable by keyboard and returns focus on Escape (ja dark 1280)', async ({
  page,
}) => {
  const copy = COPY.ja;
  await seed(page, 'ja', 'dark');
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(TIMELINE_URL);
  const entry = activeColumn(page).getByRole('button', { name: copy.entry });
  await expect(entry).toBeVisible();

  // skip link から Tab で進み、実 keyboard で入口へ到達する(AC-4)。
  let reached = false;
  for (let index = 0; index < 40; index += 1) {
    await page.keyboard.press('Tab');
    const label = await page.evaluate(
      () => document.activeElement?.getAttribute('aria-label') ?? ''
    );
    if (label === copy.entry) {
      reached = true;
      break;
    }
  }
  expect(reached).toBe(true);
  await expect(entry).toBeFocused();

  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: copy.dialog });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(copy.intro)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(entry).toBeFocused();
  await expect(page).toHaveURL(/#\/timeline\?topic=kukuri%3Atopic%3Ageneral$/);
});

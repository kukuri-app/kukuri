import { expect, test, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

// 既存フローは Live/Game タブなど WIP 面の表示を前提とするため developer mode を有効化する。
test.beforeEach(async ({ page }) => {
  await page.addInitScript((key) => {
    window.localStorage.setItem(key, 'true');
  }, DEVELOPER_MODE_STORAGE_KEY);
});

async function openComposerDialog(page: Page) {
  const composer = page.getByPlaceholder(/Write a post|投稿を書く|写一条帖子/);
  if (!(await composer.isVisible().catch(() => false))) {
    const englishTimeline = activeColumn(page, 'Timeline');
    if (await englishTimeline.isVisible().catch(() => false)) {
      await englishTimeline.getByRole('button', { name: /^Post to / }).click();
    } else {
      await page
        .locator('[data-column-id][aria-current="true"] .shell-column-primary-action')
        .click();
    }
  }
  await expect(composer).toBeVisible();
}

async function openControlCenter(page: Page) {
  const controlCenter = page.getByRole('complementary', { name: 'Control Center' });
  if (!(await controlCenter.isVisible().catch(() => false))) {
    await page.getByTestId('control-center-trigger').click();
  }
  await expect(controlCenter).toBeVisible();
  return controlCenter;
}

async function openSettings(page: Page) {
  const controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name: 'Settings', exact: true }).click();
  const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(settingsDialog).toBeVisible();
  return settingsDialog;
}

const TOPIC_ID_PREFIX = 'kukuri:topic:';
const LONG_PEER_ID = `12D3KooW${'a'.repeat(240)}`;
const LONG_PEER_TICKET = `${LONG_PEER_ID}@192.0.2.10:7777,2001:db8::10:7777`;
const LONG_ENDPOINT_DETAIL = `endpoint://${'b'.repeat(320)}@iroh-relay.kukuri.app:7842`;

function topicDisplayName(topicId: string): string {
  return topicId.startsWith(TOPIC_ID_PREFIX) ? topicId.slice(TOPIC_ID_PREFIX.length) : topicId;
}

function activeColumn(page: Page, title: string) {
  return page.getByRole('region', {
    name: new RegExp(`^${title} Column,.*Active,`),
  });
}

async function expectActiveTopic(page: Page, topic: string) {
  const controlCenter = await openControlCenter(page);
  const topicItem = controlCenter
    .getByRole('button', { name: topicDisplayName(topic), exact: true })
    .locator('xpath=ancestor::li[1]');
  await expect(topicItem).toHaveClass(/topic-item-active/);
  await controlCenter.getByRole('button', { name: 'Close Control Center' }).click();
}

test('browser mock wide shell keeps global navigation in the fixed Control Center', async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/');

  await expect(page.getByRole('complementary', { name: 'Primary navigation' })).toHaveCount(0);
  const trigger = page.getByTestId('control-center-trigger');
  await expect(trigger).toBeVisible();
  // #802: トリガーはテスターフィードバックボタンと共有する固定クラスタ内に置かれる。
  await expect(page.locator('.shell-control-cluster')).toHaveCSS('position', 'fixed');
  await expect(page.getByTestId('tester-feedback-trigger')).toBeVisible();
  await trigger.click();
  await expect(page.getByRole('complementary', { name: 'Control Center' })).toBeVisible();
});

test('technical identifiers stay hidden across developer modes and remain copyable by context actions', async ({
  context,
  page,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  const objectId = 'browser-seed-post';
  const envelopeId = 'browser-seed-envelope';
  const authorId = 'b'.repeat(64);
  const post = page.locator(`[data-post-object-id="${objectId}"]`);
  const identifierTarget = post.getByTestId('post-identifier-target');

  async function expectIdentifiersHidden() {
    await expect(post).toBeVisible();
    await expect(page.getByText(objectId, { exact: true })).toHaveCount(0);
    await expect(page.getByText(envelopeId, { exact: true })).toHaveCount(0);
    await expect(page.getByText(authorId, { exact: true })).toHaveCount(0);
  }

  await expectIdentifiersHidden();

  await page.evaluate((key) => window.localStorage.setItem(key, 'false'), DEVELOPER_MODE_STORAGE_KEY);
  await page.reload();
  await expectIdentifiersHidden();

  await post.locator('.post-body').click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Copy post ID' }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(objectId);

  await page.evaluate((key) => window.localStorage.setItem(key, 'true'), DEVELOPER_MODE_STORAGE_KEY);
  await page.reload();
  await expectIdentifiersHidden();

  await identifierTarget.focus();
  await page.keyboard.press('Shift+F10');
  await page.getByRole('menuitem', { name: 'Copy envelope ID' }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(envelopeId);

  await page.keyboard.press('Shift+F10');
  await page.keyboard.press('Escape');
  await expect(identifierTarget).toBeFocused();
});

test('browser mock starts with the accessible product overview Columns without legacy workspace chrome', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  const layout = page.locator('.shell-phase1');
  const timelineColumn = page.getByRole('region', { name: /Timeline Column/ });
  const profileColumn = page.getByRole('region', { name: /Profile Column/ });
  const exploreColumn = page.getByRole('region', { name: /Explore Column/ });
  const notificationsColumn = page.getByRole('region', { name: /Notifications Column/ });
  const messagesColumn = page.getByRole('region', { name: /Messages Column/ });

  await expect(layout).toHaveAttribute('data-workspace-layout', 'column');
  await expect(timelineColumn).toHaveCount(1);
  await expect(profileColumn).toHaveCount(1);
  await expect(exploreColumn).toHaveCount(1);
  await expect(notificationsColumn).toHaveCount(1);
  await expect(messagesColumn).toHaveCount(1);
  await expect(timelineColumn).toHaveAttribute('aria-current', 'true');
  await expect(timelineColumn).toHaveAttribute('data-span', '1');
  await expect(timelineColumn).toHaveAccessibleName(/Column 1 of 5/);
  await expect(profileColumn).toHaveAccessibleName(/Column 2 of 5/);
  await expect(exploreColumn).toHaveAccessibleName(/Column 3 of 5/);
  await expect(notificationsColumn).toHaveAccessibleName(/Column 4 of 5/);
  await expect(messagesColumn).toHaveAccessibleName(/Column 5 of 5/);
  await expect(timelineColumn).toHaveAccessibleName(/Pinned/);
  await expect(page.locator('.shell-column-title-row h2')).toHaveText([
    'Timeline',
    'Profile',
    'Explore',
    'Notifications',
    'Messages',
  ]);
  const overflowingExploreTabs = await exploreColumn
    .getByRole('tablist', { name: 'Community Index surfaces' })
    .getByRole('tab')
    .evaluateAll((tabs) =>
      tabs
        .filter(
          (tab) => tab.scrollWidth > tab.clientWidth + 1 || tab.scrollHeight > tab.clientHeight + 1
        )
        .map((tab) => (tab as HTMLElement).innerText)
    );
  expect(overflowingExploreTabs).toEqual([]);
  await expect(page.locator('main .shell-workspace-header-card')).toHaveCount(0);
  await expect(page.getByTestId('community-index-topic')).toHaveCount(0);
  await expect(page.getByRole('tablist', { name: 'Workspaces' })).toHaveCount(0);

  const columnHeader = timelineColumn.locator('.shell-column-header');
  const timelineViews = columnHeader.getByRole('tablist', { name: 'Timeline views' });
  const feedTab = timelineViews.getByRole('tab', { name: 'Feed' });
  const bookmarksTab = timelineViews.getByRole('tab', { name: 'Bookmarks' });
  const unpin = timelineColumn.getByRole('button', { name: 'Unpin Timeline' });
  await expect(feedTab.locator('svg')).toHaveCount(1);
  await expect(feedTab).toHaveText('');
  await expect(bookmarksTab.locator('svg')).toHaveCount(1);
  await expect(bookmarksTab).toHaveText('');
  await expect(timelineColumn.locator('.shell-column-body .shell-workspace-tabs')).toHaveCount(0);
  const [feedBox, bookmarksBox, unpinBox] = await Promise.all([
    feedTab.boundingBox(),
    bookmarksTab.boundingBox(),
    unpin.boundingBox(),
  ]);
  expect(feedBox).not.toBeNull();
  expect(bookmarksBox).not.toBeNull();
  expect(unpinBox).not.toBeNull();
  expect(feedBox!.width).toBe(unpinBox!.width);
  expect(feedBox!.height).toBe(unpinBox!.height);
  expect(bookmarksBox!.width).toBe(unpinBox!.width);
  expect(bookmarksBox!.height).toBe(unpinBox!.height);

  await unpin.click();
  await expect(timelineColumn).toHaveAttribute('data-transient', 'true');
  await expect(timelineColumn).toHaveAccessibleName(/Temporary/);

  const overflow = await page.evaluate(() => ({
    canvasOwnsHorizontalOverflow:
      getComputedStyle(document.querySelector('.shell-column-canvas')!).overflowX === 'auto',
    documentOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
  }));
  expect(overflow.canvasOwnsHorizontalOverflow).toBe(true);
  expect(overflow.documentOverflow).toBeLessThanOrEqual(0);
});

test('Column header switches replace Timeline scope in place and focus an inactive Column', async ({
  page,
}) => {
  await page.setViewportSize({ width: 900, height: 760 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  const timeline = page.getByRole('region', { name: /^Timeline Column,/ });
  const topicSelect = timeline.getByRole('combobox', { name: 'Timeline topic' });
  const initialColumnId = await timeline.getAttribute('data-column-id');
  await expect(topicSelect).toHaveValue('kukuri:topic:general');

  await timeline.getByRole('button', { name: /^Post to / }).click();
  await timeline.getByPlaceholder('Write a post').fill('general browser draft');
  await topicSelect.selectOption('kukuri:topic:dev');

  await expect(page).toHaveURL(/#\/timeline\?topic=kukuri%3Atopic%3Adev$/);
  await expect(page.getByRole('region', { name: /^Timeline Column,/ })).toHaveCount(1);
  await expect(timeline).toHaveAttribute('data-column-id', initialColumnId!);
  await timeline.getByRole('button', { name: /^Post to Public · dev$/ }).click();
  await expect(timeline.getByPlaceholder('Write a post')).toHaveValue('');
  await timeline.getByPlaceholder('Write a post').fill('dev browser draft');

  await topicSelect.selectOption('kukuri:topic:general');
  await expect(timeline.getByPlaceholder('Write a post')).toHaveValue('general browser draft');

  await page
    .getByRole('region', { name: /^Profile Column,/ })
    .getByRole('heading', { level: 2, name: 'Profile' })
    .click();
  const profile = activeColumn(page, 'Profile');
  await expect(profile).toBeVisible();
  await expect(page).toHaveURL(/#\/profile\?topic=kukuri%3Atopic%3Ageneral$/);
  await topicSelect.selectOption('kukuri:topic:test');
  await expect(topicSelect).toHaveValue('kukuri:topic:test');
  await expect(activeColumn(page, 'Timeline')).toBeVisible();
  await expect(page).toHaveURL(/#\/timeline\?topic=kukuri%3Atopic%3Atest$/);

  const overflow = await page.evaluate(() => ({
    documentOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    selectOverflow: Array.from(
      document.querySelectorAll<HTMLElement>('.shell-column-context-select')
    ).some((select) => {
      const selectBox = select.getBoundingClientRect();
      const columnBox = select.closest<HTMLElement>('[data-column-id]')?.getBoundingClientRect();
      return Boolean(
        columnBox &&
          (selectBox.left < columnBox.left - 1 || selectBox.right > columnBox.right + 1)
      );
    }),
  }));
  expect(overflow.documentOverflow).toBeLessThanOrEqual(0);
  expect(overflow.selectOverflow).toBe(false);
});

// ADR 0031 §2: 保存 layout が無いdesktop初期表示は既定5 ColumnをCanvas内へ並べ、
// 左下 Control Center trigger は先頭Timelineのfooter / primary action / Composerと重ねない。
for (const viewport of [
  { width: 1400, height: 980 },
  { width: 900, height: 760 },
]) {
  test(`browser mock contains the default layout overflow and keeps the Control Center trigger clear at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

    const timelineColumn = page.getByRole('region', { name: /Timeline Column/ });
    const footer = timelineColumn.locator('.shell-column-footer');
    const primaryAction = timelineColumn.getByRole('button', { name: /^Post to / });
    const trigger = page.getByTestId('control-center-trigger');
    await expect(timelineColumn).toHaveCount(1);
    await expect(primaryAction).toBeVisible();
    await expect(trigger).toBeVisible();

    const [columnBox, footerBox, actionBox, triggerBox] = await Promise.all([
      timelineColumn.boundingBox(),
      footer.boundingBox(),
      primaryAction.boundingBox(),
      trigger.boundingBox(),
    ]);
    expect(columnBox).not.toBeNull();
    expect(footerBox).not.toBeNull();
    expect(actionBox).not.toBeNull();
    expect(triggerBox).not.toBeNull();

    const canvasOverflow = await page.locator('.shell-column-canvas').evaluate((canvas) => ({
      clientWidth: canvas.clientWidth,
      overflowX: getComputedStyle(canvas).overflowX,
      scrollWidth: canvas.scrollWidth,
    }));
    expect(canvasOverflow.overflowX).toBe('auto');
    expect(canvasOverflow.scrollWidth).toBeGreaterThan(canvasOverflow.clientWidth);
    expect(intersects(footerBox!, triggerBox!)).toBe(false);
    expect(intersects(actionBox!, triggerBox!)).toBe(false);

    await primaryAction.click();
    const submit = timelineColumn.getByRole('button', { name: /^Post$/ });
    await expect(submit).toBeVisible();
    const [submitBox, expandedFooterBox] = await Promise.all([submit.boundingBox(), footer.boundingBox()]);
    expect(submitBox).not.toBeNull();
    expect(expandedFooterBox).not.toBeNull();
    expect(intersects(submitBox!, triggerBox!)).toBe(false);
    expect(intersects(expandedFooterBox!, triggerBox!)).toBe(false);
  });
}

function intersects(
  a: { x: number; y: number; width: number; height: number },
  b: { x: number; y: number; width: number; height: number }
) {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

test('browser mock shell can switch topics, publish, open thread, open author, and update discovery from settings', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/');

  await expectActiveTopic(page, 'kukuri:topic:general');

  const controlCenter = await openControlCenter(page);
  await controlCenter.getByPlaceholder('general').fill('kukuri:topic:browser');
  await controlCenter.getByRole('button', { name: 'Add Topic', exact: true }).click();
  await controlCenter.getByRole('button', { name: /^browser$/ }).click();
  await expectActiveTopic(page, 'kukuri:topic:browser');

  await openComposerDialog(page);
  await page.getByPlaceholder('Write a post').fill('hello browser mock');
  await page.getByRole('button', { name: 'Post', exact: true }).click();

  await expect(activeColumn(page, 'Timeline').getByText('hello browser mock')).toBeVisible();

  await activeColumn(page, 'Timeline').getByText('hello browser mock').click();
  const threadPane = activeColumn(page, 'Thread');
  await expect(threadPane).toBeVisible();
  await threadPane.getByRole('button', { name: 'Unknown user' }).first().click();
  await expect(activeColumn(page, 'Profile')).toBeVisible();

  const settingsDialog = await openSettings(page);
  await settingsDialog.getByTestId('settings-section-discovery').click();
  await settingsDialog.getByPlaceholder('node_id or node_id@host:port').fill('seed-peer-1');
  await settingsDialog.getByRole('button', { name: 'Save Seeds' }).click();
  await expect(settingsDialog.getByRole('textbox', { name: 'Seed Peers' })).toHaveValue('seed-peer-1');

  await settingsDialog.getByRole('textbox', { name: 'Seed Peers' }).fill('seed-peer-1\nseed-peer-2');
  await settingsDialog.getByRole('button', { name: 'Reset' }).click();
  await expect(settingsDialog.getByRole('textbox', { name: 'Seed Peers' })).toHaveValue('seed-peer-1');

  await settingsDialog.getByTestId('settings-section-community-node').click();
  await expect(
    settingsDialog.getByRole('checkbox', { name: 'Auto-approve consent for this node' })
  ).toHaveCount(0);
  await expect(settingsDialog.getByText('active on current session', { exact: true })).toBeVisible();
  await expect(settingsDialog.getByText('connectivity urls active on current session')).toBeVisible();

  await settingsDialog.locator('button').filter({ hasText: /^Add Node$/ }).click();
  await settingsDialog
    .getByPlaceholder('https://community.example.com')
    .last()
    .fill('https://community.example.com');
  await settingsDialog.getByRole('button', { name: 'Save Nodes', exact: true }).click();
  await expect(
    settingsDialog.getByRole('heading', { name: 'https://community.example.com' })
  ).toBeVisible();

  await settingsDialog.getByRole('button', { name: 'Refresh' }).first().click();
  await expect(settingsDialog.getByRole('heading', { name: 'https://api.kukuri.app' })).toBeVisible();
});

test('browser mock shell can open an author from messages without leaving the dm workspace', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/');

  await page.getByRole('button', { name: 'browser peer' }).first().click();
  const authorPane = activeColumn(page, 'Profile');
  await expect(authorPane).toBeVisible();

  await authorPane.getByRole('button', { name: 'Message' }).click();
  const conversation = activeColumn(page, 'Conversation');
  await expect(conversation).toBeVisible();
  await expect(page).toHaveURL(/#\/messages\?topic=.*peerPubkey=/);
  const conversationHeader = conversation.locator('.shell-column-header');
  await expect(conversationHeader).toContainText(/peer/);
  await expect(conversationHeader.getByRole('button', { name: 'Refresh' })).toBeVisible();
  await expect(conversationHeader.getByRole('button', { name: 'Clear' })).toBeDisabled();
  await expect(conversation.locator('.shell-column-body .shell-workspace-header')).toHaveCount(0);

  // Conversation Column 内の peer ボタンから author を開く(Timeline の author chip ではなく
  // dm workspace 起点の導線。開いた Profile の親は Conversation になる)。
  await activeColumn(page, 'Conversation')
    .getByRole('button', { name: 'browser peer' })
    .first()
    .click();
  await expect(activeColumn(page, 'Profile')).toBeVisible();
  await expect(page).toHaveURL(/authorPubkey=/);

  await activeColumn(page, 'Profile').getByRole('button', { name: 'Close Profile' }).click();
  await expect(activeColumn(page, 'Conversation')).toBeVisible();
  await expect(activeColumn(page, 'Conversation')).toBeVisible();
  await expect(page).toHaveURL(/#\/messages\?topic=.*peerPubkey=/);
});

test('browser mock shell persists appearance theme changes across reloads', async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/');

  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  const settingsDialog = await openSettings(page);
  await settingsDialog.getByTestId('settings-section-appearance').click();
  await settingsDialog.getByRole('radio', { name: /Light/i }).click();

  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');

  await page.reload();

  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
});

test('desktop Columns use kind spans, keyboard reorder, drag reorder, and persisted layout', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  let controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name: 'Add Live Column' }).click();
  const liveColumn = page.getByRole('region', { name: /^Live Column/ });
  await expect(liveColumn).toHaveAttribute('data-span', '2');
  expect(Math.round((await liveColumn.boundingBox())!.width)).toBe(896);
  const wideStreamTracks = await liveColumn.locator('.shell-stream-layout').evaluate(
    (element) => getComputedStyle(element).gridTemplateColumns
  );
  expect(wideStreamTracks.split(' ').length).toBeGreaterThan(1);

  controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name: 'Add Metaverse Column' }).click();
  let metaverseColumn = page.getByRole('region', { name: /^Metaverse Column/ });
  await expect(metaverseColumn).toHaveAttribute('data-span', '3');
  expect(Math.round((await metaverseColumn.boundingBox())!.width)).toBe(1352);

  await metaverseColumn.getByRole('button', { name: 'Open Metaverse menu' }).click();
  const menu = page.getByRole('menu', { name: 'Metaverse actions' });
  await expect(menu).toBeVisible();
  await menu.getByRole('menuitemradio', { name: '4 spans' }).click();
  await expect(metaverseColumn).toHaveAttribute('data-span', '4');
  expect(Math.round((await metaverseColumn.boundingBox())!.width)).toBe(1808);

  // Issue #768 T4: reorder は click を併用せず、trigger への実 key 入力だけで実行する
  // (WAI-ARIA menu button pattern / PR #769: Enter で開くと先頭 menuitem に focus、
  //  ArrowDown / ArrowUp で有効項目を循環、Enter で実行)。span 変更側の click 操作は上に残す。
  const metaverseMenuTrigger = metaverseColumn.getByRole('button', {
    name: 'Open Metaverse menu',
  });
  await metaverseMenuTrigger.focus();
  await page.keyboard.press('Enter');
  const moveLeftItem = page.getByRole('menuitem', { name: 'Move Metaverse left' });
  await expect(moveLeftItem).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(moveLeftItem).not.toBeFocused();
  await page.keyboard.press('ArrowUp');
  await expect(moveLeftItem).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(metaverseColumn).toHaveAccessibleName(/Column 6 of 7/);

  const canvas = page.locator('.shell-column-canvas');
  await canvas.evaluate((element) => { element.scrollLeft = 0; });
  const profileColumn = page.getByRole('region', { name: /^Profile Column/ });
  const orderBeforeDrag = await page.locator('[data-column-id]').evaluateAll((columns) =>
    columns.map((column) => (column as HTMLElement).dataset.columnId)
  );
  const profileGrip = profileColumn.getByRole('button', {
    name: 'Move Profile Column',
  });
  const profileGripBox = await profileGrip.boundingBox();
  const canvasBox = await canvas.boundingBox();
  expect(profileGripBox).not.toBeNull();
  expect(canvasBox).not.toBeNull();
  await page.mouse.move(
    profileGripBox!.x + profileGripBox!.width / 2,
    profileGripBox!.y + profileGripBox!.height / 2
  );
  await page.mouse.down();
  await page.mouse.move(
    canvasBox!.x + Math.min(canvasBox!.width - 24, 1800),
    profileGripBox!.y + profileGripBox!.height / 2,
    { steps: 12 }
  );
  await expect(page.getByRole('separator', { name: /Drop Column at position/ })).toBeVisible();
  await page.mouse.up();
  await expect.poll(async () =>
    page.locator('[data-column-id]').evaluateAll((columns) =>
      columns.map((column) => (column as HTMLElement).dataset.columnId)
    )
  ).not.toEqual(orderBeforeDrag);
  const orderAfterDrag = await page.locator('[data-column-id]').evaluateAll((columns) =>
    columns.map((column) => (column as HTMLElement).dataset.columnId)
  );
  const profileColumnId = await profileColumn.getAttribute('data-column-id');
  expect(profileColumnId).not.toBeNull();
  expect(orderAfterDrag.indexOf(profileColumnId!)).toBeGreaterThan(1);

  await page.reload();
  metaverseColumn = page.getByRole('region', { name: /^Metaverse Column/ });
  await expect(metaverseColumn).toHaveAttribute('data-span', '4');
  await expect.poll(async () =>
    page.locator('[data-column-id]').evaluateAll((columns) =>
      columns.map((column) => (column as HTMLElement).dataset.columnId)
    )
  ).toEqual(orderAfterDrag);
  await expect(page).not.toHaveURL(/span|layout|column/i);
});

test('named layouts save, restore, rename, survive reload, and delete', async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  let controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('textbox', { name: 'Layout name' }).fill('Research');
  await controlCenter.getByRole('button', { name: 'Save new layout' }).click();
  await expect(controlCenter.getByRole('button', { name: 'Research', exact: true })).toBeVisible();

  await controlCenter.getByRole('button', { name: 'Add Live Column' }).click();
  await expect(page.getByRole('region', { name: /^Live Column/ })).toBeVisible();
  controlCenter = await openControlCenter(page);
  await expect(controlCenter.getByText('Unsaved changes')).toBeVisible();
  await controlCenter.getByRole('button', { name: 'Research', exact: true }).click();
  const replaceDialog = page.getByRole('dialog', { name: 'Replace unsaved workspace?' });
  await replaceDialog.getByRole('button', { name: 'Open saved layout' }).click();
  await expect(page.getByRole('complementary', { name: 'Control Center' })).not.toBeVisible();
  await expect(page.getByRole('region', { name: /^Live Column/ })).toHaveCount(0);

  controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name: 'Rename Research' }).click();
  const rename = controlCenter.getByRole('textbox', { name: 'Rename layout' });
  await rename.fill('Reading');
  await controlCenter.getByRole('button', { name: 'Save layout name' }).click();
  await page.reload();
  controlCenter = await openControlCenter(page);
  await expect(controlCenter.getByRole('button', { name: 'Reading', exact: true })).toBeVisible();
  await controlCenter.getByRole('button', { name: 'Delete Reading' }).click();
  await page.getByRole('dialog', { name: 'Delete saved layout?' })
    .getByRole('button', { name: 'Delete layout' })
    .click();
  await expect(controlCenter.getByRole('button', { name: 'Reading', exact: true })).toHaveCount(0);
});

// Issue #768 T4: 代表 mobile test を 375 / 390 / 430px の viewport パラメタで実行する
// (test 名の変更は viewport 寸法 suffix の付与のみ)。restart 復元系など他の mobile test は
// 実行時間を抑えるためパラメタ化しない。
for (const mobileViewport of [
  { width: 375, height: 812 },
  { width: 390, height: 844 },
  { width: 430, height: 932 },
]) {
  test(`mobile Columns page one viewport at a time with snap, direct jump, and history back at ${mobileViewport.width}x${mobileViewport.height}`, async ({
    page,
  }) => {
  await page.setViewportSize(mobileViewport);
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  await openComposerDialog(page);
  await page.getByPlaceholder('Write a post').fill('mobile paging thread');
  await page.getByRole('button', { name: 'Post', exact: true }).click();
    await activeColumn(page, 'Timeline').getByText('mobile paging thread').click();
  const thread = activeColumn(page, 'Thread');
  await expect(thread).toBeVisible();
  await thread.getByRole('button', { name: 'Unknown user' }).first().click();
  await expect(activeColumn(page, 'Profile')).toBeVisible();

  const canvas = page.locator('.shell-column-canvas');
  const geometry = await page.evaluate(() => {
    const root = document.querySelector<HTMLElement>('.shell-column-canvas')!;
    return {
      canvasWidth: root.clientWidth,
      snapType: getComputedStyle(root).scrollSnapType,
      overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      widths: Array.from(root.querySelectorAll<HTMLElement>('[data-column-id]')).map(
        (column) => Math.round(column.getBoundingClientRect().width)
      ),
    };
  });
  expect(geometry.snapType).toContain('mandatory');
  expect(geometry.overflow).toBeLessThanOrEqual(0);
  expect(geometry.widths.every((width) => Math.abs(width - geometry.canvasWidth) <= 1)).toBe(true);
  const headerTargets = await activeColumn(page, 'Profile')
    .locator('.shell-column-header-actions button:visible')
    .evaluateAll((buttons) => buttons.map((button) => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height };
    }));
  expect(headerTargets.every((box) => box.width >= 44 && box.height >= 44)).toBe(true);
  // page indicator の dot も 44px のタップ目標として到達可能であること(Issue #768 T4)。
  const indicatorTargets = await page
    .locator('.shell-column-page-dots button:visible')
    .evaluateAll((buttons) => buttons.map((button) => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height };
    }));
  expect(indicatorTargets.length).toBeGreaterThan(0);
  expect(indicatorTargets.every((box) => box.width >= 44 && box.height >= 44)).toBe(true);

  await page.goBack();
  await expect(activeColumn(page, 'Thread')).toBeVisible();
  await page.goBack();
  await expect(activeColumn(page, 'Timeline')).toBeVisible();

  await page.getByRole('button', { name: 'Go to Column 3 of 7' }).click();
  await expect(activeColumn(page, 'Profile')).toBeVisible();
  await page.getByRole('button', { name: 'Go to Column 1 of 7' }).click();
  await expect(activeColumn(page, 'Timeline')).toBeVisible();
  await expect(page.getByText('1 / 7')).toBeVisible();

  const indicator = page.getByRole('navigation', { name: 'Column pages' });
  const indicatorBox = (await indicator.boundingBox())!;
  await indicator.dispatchEvent('pointerdown', {
    pointerId: 91,
    clientX: indicatorBox.x + indicatorBox.width / 2,
    clientY: indicatorBox.y + indicatorBox.height / 2,
  });
  await indicator.dispatchEvent('pointerup', {
    pointerId: 91,
    clientX: indicatorBox.x + indicatorBox.width / 2 - 80,
    clientY: indicatorBox.y + indicatorBox.height / 2 + 4,
  });
  await expect(activeColumn(page, 'Thread')).toBeVisible();
  await expect(page.getByText('2 / 7')).toBeVisible();
  // 次の逆方向gestureを始める前に、最初のsmooth scrollが対象pageへ到達するまで待つ。
  // active stateだけを待つと、低速CIでは2本のsmooth scrollが重なって前のpageへ戻り得る。
  await expect
    .poll(() => canvas.evaluate(
      (element, pageWidth) => Math.abs(element.scrollLeft - pageWidth),
      geometry.canvasWidth
    ))
    .toBeLessThanOrEqual(1);
  await indicator.dispatchEvent('pointerdown', {
    pointerId: 92,
    clientX: indicatorBox.x + indicatorBox.width / 2,
    clientY: indicatorBox.y + indicatorBox.height / 2,
  });
  await indicator.dispatchEvent('pointerup', {
    pointerId: 92,
    clientX: indicatorBox.x + indicatorBox.width / 2 + 80,
    clientY: indicatorBox.y + indicatorBox.height / 2 + 4,
  });
  await expect(activeColumn(page, 'Timeline')).toBeVisible();
  await expect(page.getByText('1 / 7')).toBeVisible();

  // indicator 切替の smooth scroll / settle が完了してから独立した edge gesture を開始する。
  // 固定 wait では並列実行時の animation frame 遅延を吸収できないため、scroll の無通信期間を待つ。
  await canvas.evaluate((element) => new Promise<void>((resolve) => {
    let settleTimer = window.setTimeout(finish, 180);
    function finish() {
      element.removeEventListener('scroll', scheduleFinish);
      resolve();
    }
    function scheduleFinish() {
      window.clearTimeout(settleTimer);
      settleTimer = window.setTimeout(finish, 180);
    }
    element.addEventListener('scroll', scheduleFinish);
  }));
  await expect
    .poll(() => canvas.evaluate((element) => Math.round(element.scrollLeft)))
    .toBeLessThanOrEqual(1);
  const canvasBox = (await canvas.boundingBox())!;
  await canvas.dispatchEvent('pointerdown', {
    pointerId: 93,
    clientX: canvasBox.x + canvasBox.width - 4,
    clientY: canvasBox.y + canvasBox.height / 2,
  });
  await canvas.dispatchEvent('pointerup', {
    pointerId: 93,
    clientX: canvasBox.x + canvasBox.width - 84,
    clientY: canvasBox.y + canvasBox.height / 2 + 3,
  });
  await expect(activeColumn(page, 'Thread')).toBeVisible();
  await page.getByRole('button', { name: 'Go to Column 1 of 7' }).click();
  await expect(activeColumn(page, 'Timeline')).toBeVisible();

  const primaryTargets = await page.locator('.shell-column-primary-action:visible').evaluateAll(
    (buttons) => buttons.map((button) => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height };
    })
  );
  expect(primaryTargets.every((box) => box.width >= 44 && box.height >= 44)).toBe(true);
  await activeColumn(page, 'Timeline').locator('.shell-column-primary-action').click();
  const composerClose = activeColumn(page, 'Timeline').getByRole('button', { name: 'Close' });
  const composerCloseBox = (await composerClose.boundingBox())!;
  expect(composerCloseBox.width).toBeGreaterThanOrEqual(44);
  expect(composerCloseBox.height).toBeGreaterThanOrEqual(44);
  await composerClose.click();

  const mobileControlCenter = await openControlCenter(page);
  const columnListTargets = await mobileControlCenter
    .locator('.shell-control-center-column-row button:not(.shell-control-center-column-focus)')
    .evaluateAll((buttons) => buttons.map((button) => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height };
    }));
  expect(columnListTargets.length).toBeGreaterThan(0);
  expect(
    columnListTargets.filter((box) => box.width + 0.01 < 44 || box.height + 0.01 < 44)
  ).toEqual([]);
  await mobileControlCenter.getByRole('button', { name: 'Close Control Center' }).click();

  await canvas.evaluate((element) => {
    element.scrollLeft = element.clientWidth;
    element.dispatchEvent(new Event('scroll'));
  });
  await expect(activeColumn(page, 'Thread')).toBeVisible();
  await page.getByRole('button', { name: 'Go to Column 1 of 7' }).click();
  await expect(activeColumn(page, 'Timeline')).toBeVisible();
  await expect(canvas).toBeVisible();
  });
}

test('mobile restart restores text Draft and active Column focus without unsafe fields', async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await openComposerDialog(page);
  const composer = page.getByPlaceholder('Write a post');
  await composer.fill('restart-safe mobile Draft');
  const composerLayout = await page.evaluate(() => ({
    controlCenterDisplay: getComputedStyle(
      document.querySelector<HTMLElement>('.shell-control-center-trigger')!
    ).display,
    footerRight: document.querySelector<HTMLElement>('.shell-column-footer')!
      .getBoundingClientRect().right,
    viewportWidth: document.documentElement.clientWidth,
  }));
  expect(composerLayout.controlCenterDisplay).toBe('none');
  expect(composerLayout.footerRight).toBeLessThanOrEqual(composerLayout.viewportWidth);
  await page.waitForTimeout(300);

  const payload = await page.evaluate(() => localStorage.getItem('kukuri:column-drafts:v1'));
  expect(payload).toContain('restart-safe mobile Draft');
  expect(payload).not.toContain('mediaItems');
  expect(payload).not.toContain('pending');
  expect(payload).not.toContain('error');

  await page.reload();
  await expect(page.getByPlaceholder('Write a post')).toHaveValue('restart-safe mobile Draft');
  await expect(activeColumn(page, 'Timeline')).toBeFocused();
});

// Issue #765 T4: hash を持たない cold start でも、保存 layout の active Column が復元される。
test('cold start without a hash restores the persisted active Thread Column', async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  // seed 済み投稿(reload 後も存在する)から Thread を開く。
  // 実行中に publish した投稿は browser mock のメモリにしか無く、reload 後は
  // 無効 target として安全側 normalize に倒れてしまうため使わない。
  await page.getByText('browser mock peer post').click();
  await expect(activeColumn(page, 'Thread')).toBeVisible();
  await page.waitForTimeout(300);

  // hash 無しで開き直す(cold start 相当。localStorage は同一 context で保持される)。
  await page.goto('/');
  await expect(activeColumn(page, 'Thread')).toBeVisible();
  await expect(page).toHaveURL(/context=thread&threadId=/);
});

test('browser mock shell persists language changes across reloads', async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/');

  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await openComposerDialog(page);
  await expect(page.getByPlaceholder('Write a post')).toBeVisible();
  await page.keyboard.press('Escape');

  const settingsDialog = await openSettings(page);
  await settingsDialog.getByTestId('settings-section-appearance').click();
  await settingsDialog.getByLabel('Language').selectOption('ja');

  await expect(page.locator('html')).toHaveAttribute('lang', 'ja');
  await page.keyboard.press('Escape');
  await expect(settingsDialog).toBeHidden();
  await openComposerDialog(page);
  await expect(page.getByPlaceholder('投稿を書く')).toBeVisible();
  await expect(page.getByRole('button', { name: '投稿' })).toBeVisible();
  await page.keyboard.press('Escape');

  await page.reload();

  await expect(page.locator('html')).toHaveAttribute('lang', 'ja');
  await openComposerDialog(page);
  await expect(page.getByPlaceholder('投稿を書く')).toBeVisible();
  await page.keyboard.press('Escape');

  await page.goto('/#/game');
  await expect(page.getByRole('heading', { name: 'メタバースルーム' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'メタバースルームを作成' })).toBeVisible();
});

test('browser mock settings drawer keeps the close button clear of content and captures wheel scrolling', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.goto('/');

  const settingsDialog = await openSettings(page);
  await settingsDialog.getByTestId('settings-section-connectivity').click();

  const closeButton = settingsDialog.getByRole('button', { name: 'Close settings' });
  const syncStatusCard = settingsDialog
    .getByRole('heading', { name: 'Sync Status' })
    .locator('xpath=ancestor::section[1]');
  const scrollContainer = settingsDialog.locator('.shell-settings-content');

  await expect.poll(() =>
    scrollContainer.evaluate((element) => element.scrollHeight > element.clientHeight)
  ).toBeTruthy();

  const closeBox = await closeButton.boundingBox();
  const syncStatusBox = await syncStatusCard.boundingBox();

  expect(closeBox).not.toBeNull();
  expect(syncStatusBox).not.toBeNull();
  expect(closeBox!.y + closeBox!.height).toBeLessThanOrEqual(syncStatusBox!.y);

  const scrollBox = await scrollContainer.boundingBox();
  expect(scrollBox).not.toBeNull();

  await page.mouse.move(
    scrollBox!.x + scrollBox!.width / 2,
    scrollBox!.y + Math.min(scrollBox!.height / 2, 220)
  );
  await page.mouse.wheel(0, 960);

  await expect.poll(() => scrollContainer.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
});

test('browser mock connectivity settings keep long identifiers within the content width', async ({
  page,
}) => {
  await page.addInitScript(
    ({ endpointDetail, peerId, peerTicket }) => {
      let desktopApi = window.__KUKURI_DESKTOP__;

      Object.defineProperty(window, '__KUKURI_DESKTOP__', {
        configurable: true,
        get: () => desktopApi,
        set: (api: typeof desktopApi) => {
          desktopApi = api;
          if (!api) {
            return;
          }

          const getSyncStatus = api.getSyncStatus.bind(api);
          api.getSyncStatus = async () => {
            const status = await getSyncStatus();
            return {
              ...status,
              configured_peers: [peerId],
              status_detail: endpointDetail,
              discovery: {
                ...status.discovery,
                connected_peer_ids: [peerId],
                local_endpoint_id: peerId,
              },
              topic_diagnostics: status.topic_diagnostics.map((diagnostic) => ({
                ...diagnostic,
                connected_peers: [peerId],
                configured_peer_ids: [peerId],
                docs_assist_peer_ids: [peerId],
                status_detail: endpointDetail,
              })),
            };
          };
          api.getLocalPeerTicket = async () => peerTicket;
        },
      });
    },
    {
      endpointDetail: LONG_ENDPOINT_DETAIL,
      peerId: LONG_PEER_ID,
      peerTicket: LONG_PEER_TICKET,
    }
  );

  await page.setViewportSize({ width: 1024, height: 870 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=connectivity');

  const settingsDialog = page.getByRole('dialog', { name: 'Settings' });
  const scrollContainer = settingsDialog.locator('.shell-settings-content');
  const localPeerTicket = settingsDialog.getByRole('textbox', { name: 'Your Ticket' });
  await expect(settingsDialog).toBeVisible();
  await expect(localPeerTicket).toHaveValue(LONG_PEER_TICKET);
  await settingsDialog.getByText('Technical diagnostic details', { exact: true }).first().click();
  await expect(settingsDialog.getByText(LONG_ENDPOINT_DETAIL).first()).toBeVisible();
  await expect(settingsDialog.getByText(LONG_PEER_ID).first()).toBeVisible();

  for (const width of [1024, 1280, 1440, 700]) {
    await page.setViewportSize({ width, height: 870 });

    const layout = await scrollContainer.evaluate((element) => {
      const contentRect = element.getBoundingClientRect();
      const contentRight = contentRect.left + element.clientWidth;
      const overflowingPanels = Array.from(element.querySelectorAll<HTMLElement>('.panel')).filter(
        (panel) => panel.getBoundingClientRect().right > contentRight + 0.5
      );

      return {
        contentFits: element.scrollWidth <= element.clientWidth,
        horizontalOverflowPolicy: getComputedStyle(element).overflowX,
        overflowingPanelCount: overflowingPanels.length,
      };
    });
    const ticketFits = await localPeerTicket.evaluate(
      (element) => element.scrollWidth <= element.clientWidth
    );

    expect(layout, `viewport width: ${width}`).toEqual({
      contentFits: true,
      horizontalOverflowPolicy: 'hidden',
      overflowingPanelCount: 0,
    });
    expect(ticketFits, `peer ticket at viewport width: ${width}`).toBeTruthy();
    await expect(settingsDialog.getByRole('button', { name: 'Import Peer' })).toBeVisible();
  }
});

test('browser mock narrow shell keeps nav, context, and settings flows reachable without overflow', async ({
  page,
}) => {
  await page.setViewportSize({ width: 700, height: 980 });
  await page.goto('/');

  let controlCenter = await openControlCenter(page);
  await controlCenter.getByPlaceholder('general').fill('kukuri:topic:narrow');
  await controlCenter.getByRole('button', { name: 'Add Topic', exact: true }).click();

  controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name: /^general$/ }).click();
  await expectActiveTopic(page, 'kukuri:topic:general');

  await openComposerDialog(page);
  await page.getByPlaceholder('Write a post').fill('narrow browser mock');
  await page.getByRole('button', { name: 'Post', exact: true }).click();
  await expect(activeColumn(page, 'Timeline').getByText('narrow browser mock')).toBeVisible();

  await activeColumn(page, 'Timeline').getByText('narrow browser mock').click();
  const threadColumn = activeColumn(page, 'Thread');
  await expect(threadColumn).toBeVisible();

  await threadColumn.getByRole('button', { name: 'Unknown user' }).first().click();
  await expect(activeColumn(page, 'Profile')).toBeVisible();

  await page.goto('/');
  const settingsDialog = await openSettings(page);
  await settingsDialog.getByTestId('settings-section-connectivity').click();
  await settingsDialog.getByPlaceholder('nodeid@127.0.0.1:7777').fill('peer-b@127.0.0.1:8888');
  await settingsDialog.getByRole('button', { name: 'Import Peer' }).click();
  await expect(settingsDialog.getByPlaceholder('nodeid@127.0.0.1:7777')).toHaveValue('');

  await settingsDialog.getByTestId('settings-section-community-node').click();
  await settingsDialog.locator('button').filter({ hasText: /^Add Node$/ }).click();
  await settingsDialog
    .getByPlaceholder('https://community.example.com')
    .last()
    .fill('https://community.example.com');
  await settingsDialog.getByRole('button', { name: 'Save Nodes', exact: true }).click();
  await expect(
    settingsDialog.getByRole('heading', { name: 'https://community.example.com' })
  ).toBeVisible();

  const settingsNoOverflow = await settingsDialog.evaluate(
    (element) => element.scrollWidth <= element.clientWidth
  );
  expect(settingsNoOverflow).toBeTruthy();

  await page.keyboard.press('Escape');
  await page.goto('/#/profile?topic=kukuri%3Atopic%3Ageneral');
  await expect(page.getByRole('button', { name: 'Edit Profile' })).toBeVisible();

  const noOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth <= window.innerWidth
  );
  expect(noOverflow).toBeTruthy();
});

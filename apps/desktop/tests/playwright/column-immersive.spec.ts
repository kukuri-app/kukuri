import { expect, test, type Page } from '@playwright/test';

import { openFixtureLive } from './live-fixture';
import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

// Issue #748 設計判断 12 / 13 の browser 固定:
// - Metaverse stage(gesture owner)内の pointer / touch は Column 切替を奪わない
// - 画面外の Stream / Metaverse Column は suspended へ縮退し、戻ると復帰する(session は維持)
// Live / Metaverse Column は developer mode でのみ Control Center から追加できる。
test.beforeEach(async ({ page }) => {
  await page.addInitScript((key) => {
    window.localStorage.setItem(key, 'true');
  }, DEVELOPER_MODE_STORAGE_KEY);
});

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

async function addColumn(page: Page, name: 'Add Live Column' | 'Add Metaverse Column') {
  if (name === 'Add Live Column') { await openFixtureLive(page); return; }
  const controlCenter = await openControlCenter(page);
  await controlCenter.getByRole('button', { name }).click();
}

test('Stream and Metaverse fullscreen return to the same Column workspace state', async ({
  page,
}) => {
  // Two fullscreen round trips plus a cold 3D admission exceed the default
  // 30s on the shared CI runner; assertions and actionability remain unchanged.
  test.slow();
  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await addColumn(page, 'Add Live Column');
  await addColumn(page, 'Add Metaverse Column');
  const { metaverse, stage } = await createMetaverseRoom(page);
  await metaverse.getByRole('button', { name: 'Menu (Tab)' }).click();
  await metaverse.getByRole('button', { name: 'Hosting', exact: true }).click();
  const hostingWidth = await metaverse.locator('.metaverse-category-content:not([hidden]) .panel-subsection').evaluate(
    (element) => element.getBoundingClientRect().width / Number.parseFloat(getComputedStyle(document.documentElement).fontSize)
  );
  expect(hostingWidth).toBeLessThanOrEqual(40);
  await metaverse.getByRole('button', { name: 'Open room chat' }).click();
  const draft = metaverse.locator('.metaverse-chat-form input');
  await draft.fill('Unsent fullscreen draft 1023');
  const originalCanvas = await stage.locator('canvas').elementHandle();
  const live = page.getByRole('region', { name: /^Live Column/ });
  const before = await page.locator('[data-column-id]').evaluateAll((columns) =>
    columns.map((column) => ({
      id: (column as HTMLElement).dataset.columnId,
      span: (column as HTMLElement).dataset.span,
    }))
  );

  await live.scrollIntoViewIfNeeded();
  await page.waitForTimeout(150);
  await live.getByRole('button', { name: 'Open Live menu' }).click();
  await page.getByRole('menuitem', { name: 'Enter Live fullscreen' }).click();
  await expect.poll(() => live.evaluate((element) => document.fullscreenElement === element)).toBe(true);
  // Headless Chromium は browser chrome が所有する Escape を合成keyから実行しないため、
  // 標準 Escape と同じ exitFullscreen -> fullscreenchange 経路を直接発火する。
  await page.evaluate(() => document.exitFullscreen());
  await expect.poll(() => page.evaluate(() => document.fullscreenElement === null)).toBe(true);
  await expect(live).toBeFocused();

  await metaverse.getByRole('button', { name: 'Open Metaverse menu' }).click();
  await page.getByRole('menuitem', { name: 'Enter Metaverse fullscreen' }).click();
  await expect.poll(() => metaverse.evaluate((element) => document.fullscreenElement === element)).toBe(true);
  await expect(metaverse).toHaveAttribute('data-runtime-visible', 'true');
  await expect(metaverse).not.toHaveAttribute('data-runtime-suspended', 'true');
  const viewport = metaverse.locator('.metaverse-viewport-shell');
  await expect(viewport).not.toHaveAttribute('data-render-suspended', 'true');
  const bodyBounds = await metaverse.locator('.shell-column-body').boundingBox();
  const sceneBounds = await viewport.boundingBox();
  expect(sceneBounds!.height).toBeGreaterThan(bodyBounds!.height - 100);
  expect(sceneBounds!.y + sceneBounds!.height).toBeLessThanOrEqual(bodyBounds!.y + bodyBounds!.height);
  await expect(draft).toBeInViewport();
  await expect(metaverse.getByRole('button', { name: 'Refresh dome connections' })).toBeHidden();
  await metaverse.getByRole('button', { name: 'Dome tools', exact: true }).click();
  await expect(metaverse.locator('.metaverse-aux-before')).toBeVisible();
  await expect(metaverse.getByRole('button', { name: 'Refresh dome connections' })).toBeHidden();
  await metaverse.locator('.metaverse-auxiliary').getByRole('button', { name: 'Close Dome tools', exact: true }).click();
  await expect(metaverse.getByRole('button', { name: 'Dome tools', exact: true })).toBeFocused();
  await expect(draft).toHaveValue('Unsent fullscreen draft 1023');
  await metaverse.getByRole('button', { name: 'Open Metaverse menu' }).click();
  await page.getByRole('menuitem', { name: 'Exit Metaverse fullscreen' }).click();
  await expect.poll(() => page.evaluate(() => document.fullscreenElement === null)).toBe(true);

  expect(
    await page.locator('[data-column-id]').evaluateAll((columns) =>
      columns.map((column) => ({
        id: (column as HTMLElement).dataset.columnId,
        span: (column as HTMLElement).dataset.span,
      }))
    )
  ).toEqual(before);
  await expect(stage).toHaveCount(1);
  expect(await originalCanvas!.evaluate((canvas) => canvas.isConnected)).toBe(true);
  await expect(draft).toHaveValue('Unsent fullscreen draft 1023');
  await expect(metaverse).toBeFocused();
});

// Metaverse Column 内で room を作成して stage(data-column-gesture-owner)を表示する。
test('fullscreen at a small effective viewport keeps chat and tools inside the screen', async ({ page }) => {
  await page.setViewportSize({ width: 800, height: 600 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await addColumn(page, 'Add Metaverse Column');
  const { metaverse } = await createMetaverseRoom(page);
  await metaverse.getByRole('button', { name: 'Open room chat' }).click();
  await metaverse.getByRole('button', { name: 'Open Metaverse menu' }).click();
  await page.getByRole('menuitem', { name: 'Enter Metaverse fullscreen' }).click();
  await expect(metaverse.getByRole('button', { name: 'Dome tools', exact: true })).toBeVisible();
  await expect(metaverse.locator('.metaverse-chat-form input')).toBeInViewport();
  const stage = await metaverse.locator('.metaverse-room-stage').boundingBox();
  expect(stage!.y + stage!.height).toBeLessThanOrEqual(600);
  await metaverse.getByRole('button', { name: 'Dome tools', exact: true }).click();
  await expect(metaverse.locator('.metaverse-aux-before').getByRole('button', { name: 'Close Dome tools' })).toBeInViewport();
});

async function createMetaverseRoom(page: Page) {
  const metaverse = page.getByRole('region', { name: /^Metaverse Column/ });
  const createTrigger = metaverse.getByRole('button', { name: 'Create metaverse room' }).first();
  if (await createTrigger.getAttribute('aria-expanded') === 'false') await createTrigger.click();
  await metaverse.getByPlaceholder('Atrium').fill('Gesture lab');
  await metaverse.getByRole('button', { name: 'Create metaverse room' }).last().click();
  await metaverse.getByRole('button', { name: 'Start hosting and enter' }).click();
  const stage = metaverse.locator('[data-column-gesture-owner="metaverse"]');
  await expect(stage).toBeVisible();
  return { metaverse, stage };
}

test('owner management separates stopping, cancellation and deletion, then permits recreation', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 840 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await addColumn(page, 'Add Metaverse Column');
  const { metaverse, stage } = await createMetaverseRoom(page);
  await expect(stage).toBeFocused();
  await metaverse.getByRole('button', { name: 'Manage Dome', exact: true }).click();
  const management = metaverse.locator('[aria-label="Manage Dome"]');
  await management.getByRole('button', { name: 'Close hosting' }).click();
  await expect(stage).toHaveCount(0);
  await expect(management.getByRole('button', { name: 'Start hosting and enter' })).toBeEnabled();
  await management.getByRole('button', { name: 'Delete Dome', exact: true }).click();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(management).toBeVisible();
  await management.getByRole('button', { name: 'Delete Dome', exact: true }).click();
  await page.getByRole('button', { name: 'Confirm deletion', exact: true }).click();
  await expect(metaverse.getByRole('button', { name: 'Manage Dome', exact: true })).toHaveCount(0);
  const recreated = await createMetaverseRoom(page);
  await expect(recreated.stage).toBeFocused();
});

test('desktop Metaverse stage keeps pointer ownership and does not steal Column activation', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await addColumn(page, 'Add Metaverse Column');
  const { metaverse, stage } = await createMetaverseRoom(page);
  await expect(metaverse).toHaveAttribute('aria-current', 'true');

  // Timeline header(非 interactive 領域)で Timeline を active にする。
  const timeline = page.getByRole('region', { name: /^Timeline Column/ });
  await timeline.locator('.shell-column-header h2').first().dispatchEvent('pointerdown');
  await expect(activeColumn(page, 'Timeline')).toBeVisible();

  // stage 内の pointerdown は scene 操作として扱われ、Column activation を起こさない。
  const box = (await stage.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 120, box.y + box.height / 2 + 40, { steps: 6 });
  await page.mouse.up();
  await expect(activeColumn(page, 'Timeline')).toBeVisible();
  await expect(metaverse).not.toHaveAttribute('aria-current', 'true');

  // Metaverse Column 自身の header では通常どおり activate できる。
  await metaverse.locator('.shell-column-header h2').first().dispatchEvent('pointerdown');
  await expect(activeColumn(page, 'Metaverse')).toBeVisible();
});

test.describe('mobile touch ownership', () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 844 } });

  test('mobile Metaverse stage owns horizontal touch while Column header swipe still pages', async ({
    page,
  }) => {
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
    await addColumn(page, 'Add Metaverse Column');
    const { metaverse, stage } = await createMetaverseRoom(page);
    await expect(activeColumn(page, 'Metaverse')).toBeVisible();
    await expect(page.getByText('6 / 6')).toBeVisible();
    await stage.scrollIntoViewIfNeeded();
    await expect(stage).toBeInViewport();

    const touchAction = await stage.evaluate((element) => getComputedStyle(element).touchAction);
    expect(touchAction).toBe('none');

    const canvas = page.locator('.shell-column-canvas');
    const startScrollLeft = await canvas.evaluate((element) => element.scrollLeft);
    expect(startScrollLeft).toBeGreaterThan(0);

    // stage 上の右スワイプ(前の Column へ戻る方向)は Column paging を起こさない。
    const client = await page.context().newCDPSession(page);
    const stageBox = (await stage.boundingBox())!;
    const bodyBox = (await metaverse.locator('.shell-column-body').boundingBox())!;
    // Responsive HUD/chat can make the stage taller than the visible body.
    // Start inside its visible intersection, not on the fixed page indicator below it.
    const touchY = (Math.max(stageBox.y, bodyBox.y) + Math.min(stageBox.y + stageBox.height, bodyBox.y + bodyBox.height - 80)) / 2;
    expect(await page.evaluate(({ x, y }) => Boolean(document.elementFromPoint(x, y)?.closest('[data-column-gesture-owner="metaverse"]')), { x: stageBox.x + 40, y: touchY })).toBe(true);
    await swipe(client, stageBox.x + 40, touchY, 300, 0);
    await page.waitForTimeout(400);
    expect(await canvas.evaluate((element) => element.scrollLeft)).toBe(startScrollLeft);
    await expect(activeColumn(page, 'Metaverse')).toBeVisible();

    // Column header 上の同じスワイプは Column paging を起こし、直前のMessagesがactiveになる。
    const headerBox = (await metaverse.locator('.shell-column-header').boundingBox())!;
    await swipe(client, headerBox.x + 40, headerBox.y + headerBox.height / 2, 300, 0);
    await expect(activeColumn(page, 'Messages')).toBeVisible();
    expect(await canvas.evaluate((element) => element.scrollLeft)).toBeLessThan(startScrollLeft);
  });
});

test('offscreen Metaverse and Live Columns suspend rendering and resume without losing the session', async ({
  page,
}) => {
  // fresh defaultの末尾にLive / Metaverseを追加し、各immersive Columnへ移動した際の
  // runtime suspensionとsession維持を確認する。
  // 900px幅ではCanvasがLiveの2 span幅より狭く、隣接するMetaverseと同時に
  // 1%を超えて交差しないためsuspension境界を決定的に観測できる。
  await page.setViewportSize({ width: 900, height: 900 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await addColumn(page, 'Add Live Column');
  await addColumn(page, 'Add Metaverse Column');
  const { metaverse, stage } = await createMetaverseRoom(page);
  const live = page.getByRole('region', { name: /^Live Column/ });
  await expect(live).toHaveAccessibleName(/Column 6 of 7/);
  await expect(metaverse).toHaveAccessibleName(/Column 7 of 7/);

  // 追加直後は Metaverse が active で画面内にあり、縮退していない。
  await expect(metaverse).not.toHaveAttribute('data-runtime-suspended', 'true');
  await expect(stage.locator('[data-render-suspended="true"]')).toHaveCount(0);
  // この時点で Live は画面外(Canvas 末尾までスクロール済み)のため縮退している。
  await expect(live).toHaveAttribute('data-runtime-suspended', 'true');

  // Liveをactiveにして画面内へ戻すとMetaverseは画面外になり縮退し、Liveは復帰する。
  await live.locator('.shell-column-header h2').first().dispatchEvent('pointerdown');
  await expect(activeColumn(page, 'Live')).toBeVisible();
  await expect(metaverse).toHaveAttribute('data-runtime-suspended', 'true');
  await expect(stage.locator('[data-render-suspended="true"]')).toHaveCount(1);
  await expect(live).not.toHaveAttribute('data-runtime-suspended', 'true');
  // suspended 中も room session(stage)は mount されたまま維持される。
  await expect(stage).toHaveCount(1);
  await expect(stage.locator('canvas')).toHaveCount(1);

  // 画面内へ戻すと縮退が解除され、代わりに画面外へ出た Live が縮退する。
  await metaverse.locator('.shell-column-header h2').first().dispatchEvent('pointerdown');
  await expect(activeColumn(page, 'Metaverse')).toBeVisible();
  await expect(metaverse).not.toHaveAttribute('data-runtime-suspended', 'true');
  await expect(stage.locator('[data-render-suspended="true"]')).toHaveCount(0);
  await expect(live).toHaveAttribute('data-runtime-suspended', 'true');
});

test('rapid header reselection keeps Metaverse active while the previous route is pending', async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 900, height: 900 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await addColumn(page, 'Add Live Column');
  await addColumn(page, 'Add Metaverse Column');
  const { metaverse, stage } = await createMetaverseRoom(page);
  const live = page.getByRole('region', { name: /^Live Column/ });
  await expect(activeColumn(page, 'Metaverse')).toBeVisible();
  await expect(page).toHaveURL(/#\/game\?/);

  const sequence = await page.evaluate(async ({ liveId, metaverseId }) => {
    const header = (id: string) => document.querySelector<HTMLElement>(
      `[data-column-id="${CSS.escape(id)}"] .shell-column-header h2`
    )!;
    const snapshot = () => ({
      hash: window.location.hash,
      active: document.querySelector<HTMLElement>('[data-column-id][data-active]')?.dataset.columnId,
      focus: document.activeElement?.closest<HTMLElement>('[data-column-id]')?.dataset.columnId,
    });
    const before = snapshot();
    header(liveId).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    // external store の更新を反映し、router の transition render より先に選び直す。
    await Promise.resolve();
    const afterLive = snapshot();
    header(metaverseId).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    await Promise.resolve();
    return { before, afterLive, afterMetaverse: snapshot() };
  }, {
    liveId: (await live.getAttribute('data-column-id'))!,
    metaverseId: (await metaverse.getAttribute('data-column-id'))!,
  });
  await testInfo.attach('activation-sequence', {
    body: JSON.stringify(sequence, null, 2),
    contentType: 'application/json',
  });
  expect(sequence.afterLive.active).toBe(await live.getAttribute('data-column-id'));
  expect(sequence.afterMetaverse.active).toBe(await metaverse.getAttribute('data-column-id'));
  expect(sequence.afterMetaverse.hash).toBe(sequence.before.hash);
  // 選択・URL・runtime・scene を同時点で観測し、一瞬の復帰だけを成功としない。
  const liveId = (await live.getAttribute('data-column-id'))!;
  await expect.poll(() => metaverse.evaluate((element, streamId) => ({
    active: element.getAttribute('data-active') === 'true',
    hash: window.location.hash,
    suspended: element.getAttribute('data-runtime-suspended') === 'true',
    suspendedScenes: element.querySelectorAll('[data-render-suspended="true"]').length,
    canvases: element.querySelectorAll('canvas').length,
    liveSuspended: document.querySelector(`[data-column-id="${CSS.escape(streamId)}"]`)
      ?.getAttribute('data-runtime-suspended') === 'true',
  }), liveId)).toEqual({
    active: true, hash: sequence.before.hash, suspended: false,
    suspendedScenes: 0, canvases: 1, liveSuspended: true,
  });
  await expect(stage).toHaveCount(1);
});

test('pointer selection and history preserve joined Live and hosted Metaverse sessions', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 900 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await addColumn(page, 'Add Live Column');
  const live = page.getByRole('region', { name: /^Live Column/ });
  // 参加の状態は表示寿命と独立して保持する。
  await page.evaluate(async () => {
    const api = window.__KUKURI_DESKTOP__!;
    const id = new URLSearchParams(location.hash.split('?')[1]).get('sessionId')!;
    await api.joinLiveSession('kukuri:topic:general', id);
  });
  const liveSession = live.locator('.live-stage');
  const sessionElement = await liveSession.elementHandle();
  await addColumn(page, 'Add Metaverse Column');
  const { metaverse, stage } = await createMetaverseRoom(page);
  const stageElement = await stage.elementHandle();
  const canvasElement = await stage.locator('canvas').elementHandle();

  // 実 pointer の default focus／scroll と、正当な後続の履歴操作も維持する。
  await live.locator('.shell-column-header h2').click();
  await expect(activeColumn(page, 'Live')).toBeVisible();
  await expect(metaverse).toHaveAttribute('data-runtime-suspended', 'true');
  await metaverse.locator('.shell-column-header h2').click();
  await expect(activeColumn(page, 'Metaverse')).toBeVisible();
  await expect(stage.locator('[data-render-suspended="true"]')).toHaveCount(0);
  await expect(live).toHaveAttribute('data-runtime-suspended', 'true');
  await page.goBack();
  await expect(activeColumn(page, 'Live')).toBeVisible();
  await expect(page).toHaveURL(/#\/live\?/);
  expect(await page.evaluate(async () => {
    const api = window.__KUKURI_DESKTOP__!;
    const id = new URLSearchParams(location.hash.split('?')[1]).get('sessionId')!;
    return (await api.getLiveSession('kukuri:topic:general', id))?.joined_by_me;
  })).toBe(true);
  await page.goForward();
  await expect(activeColumn(page, 'Metaverse')).toBeVisible();
  await expect(page).toHaveURL(/#\/game\?/);
  await expect(metaverse).not.toHaveAttribute('data-runtime-suspended', 'true');
  expect(await sessionElement!.evaluate((element) => element.isConnected)).toBe(true);
  expect(await stageElement!.evaluate((element) => element.isConnected)).toBe(true);
  expect(await canvasElement!.evaluate((element) => element.isConnected)).toBe(true);
});

// CDP で touch swipe を合成する(Playwright は touch drag API を持たない)。
async function swipe(
  client: Awaited<ReturnType<Page['context']>['newCDPSession']> extends (...args: never[]) => infer R
    ? Awaited<R>
    : never,
  x: number,
  y: number,
  dx: number,
  dy: number
) {
  await client.send('Input.dispatchTouchEvent', {
    type: 'touchStart',
    touchPoints: [{ x, y }],
  });
  const steps = 8;
  for (let step = 1; step <= steps; step += 1) {
    await client.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [{ x: x + (dx * step) / steps, y: y + (dy * step) / steps }],
    });
  }
  await client.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
}

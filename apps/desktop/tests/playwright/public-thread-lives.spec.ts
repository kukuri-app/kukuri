import { expect, test, type Page } from '@playwright/test';
import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

const TOPIC = 'kukuri:topic:general';
const activeColumn = (page: Page, title: string) =>
  page.getByRole('region', { name: new RegExp(`^${title} Column,.*Active,`) });

test.beforeEach(async ({ page }) => {
  await page.addInitScript((key) => localStorage.setItem(key, 'true'), DEVELOPER_MODE_STORAGE_KEY);
});

for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
  test(`live starts in a thread or composer and fills the column at ${viewport.width}px`, async ({ page }, info) => {
    await page.setViewportSize(viewport);
    await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&context=publicThreads');
    await activeColumn(page, 'Public threads').locator('.shell-column-footer').getByRole('button', { name: 'Create public thread' }).click();
    const create = page.getByRole('dialog', { name: 'Create a public thread' });
    await create.getByRole('textbox', { name: 'Title' }).fill('Watch party');
    await create.getByRole('textbox', { name: 'First post (optional)' }).fill('Let’s watch together');
    await create.getByRole('checkbox', { name: 'Start Live' }).check();
    await create.getByRole('button', { name: 'Create', exact: true }).click();
    const detail = activeColumn(page, 'Public thread');
    await expect(detail.getByRole('list', { name: 'Live now' }).getByRole('button', { name: /^Live Watch party / })).toBeVisible();
    await expect(detail.locator('.shell-column-header').getByRole('button', { name: 'Live', exact: true })).toHaveCount(0);
    await detail.getByRole('button', { name: /^Post to / }).click();
    await detail.getByRole('button', { name: 'Start Live', exact: true }).click();
    const start = page.getByRole('dialog', { name: 'Start Live' });
    await start.getByRole('textbox', { name: 'Live Title' }).fill('Night stream');
    await start.getByRole('textbox', { name: 'Post text' }).fill('Second broadcast');
    await start.getByRole('button', { name: 'Start Live', exact: true }).click();
    await expect(start).toBeHidden();
    const channel = new URLSearchParams(new URL(page.url()).hash.split('?')[1]).get('channel')!;
    await page.evaluate(async ({ topic, channel }) => {
      const api = window.__KUKURI_DESKTOP__!;
      const ended = await api.createLiveSession(topic, 'Finished broadcast', '', { kind: 'public_channel', channel_id: channel });
      await api.endLiveSession(topic, ended);
    }, { topic: TOPIC, channel });
    await detail.getByRole('list', { name: 'Live now' }).getByRole('button', { name: /^Live Watch party / }).click();
    const preview = page.getByRole('dialog', { name: 'Watch party', exact: true });
    await expect(preview.getByText('Let’s watch together', { exact: true })).toBeVisible();
    await page.screenshot({ path: info.outputPath('details.png') });
    await preview.getByRole('button', { name: 'Show large', exact: true }).click();
    const live = activeColumn(page, 'Live');
    await expect(live.locator('.live-column')).toHaveAttribute('data-layout', 'full');
    await expect(live.locator('.live-column-side')).toHaveCount(0);
    const full = await live.locator('.live-viewer-screen').boundingBox();
    const body = await live.locator('.shell-column-body').boundingBox();
    expect(full!.width).toBeGreaterThan(body!.width * 0.95);
    expect(full!.height).toBeGreaterThan(body!.height * 0.5);
    await page.screenshot({ path: info.outputPath('full.png') });
    await live.getByRole('button', { name: 'Split view', exact: true }).click();
    const side = live.getByRole('tabpanel');
    await expect(side.getByText('Night stream', { exact: true })).toBeVisible();
    await expect(side.getByText('Finished broadcast', { exact: true })).toHaveCount(0);
    await expect(side.getByText('Watch party', { exact: true })).toHaveCount(0);
    const half = await live.locator('.live-viewer-screen').boundingBox();
    if (viewport.width > 1000) expect(half!.width).toBeLessThan(full!.width * 0.55);
    else expect(half!.height).toBeLessThan(full!.height * 0.6);
    await page.screenshot({ path: info.outputPath('split-lives.png') });
    await live.getByRole('tab', { name: 'Public thread', exact: true }).click();
    await expect(side.getByText('Let’s watch together', { exact: true })).toBeVisible();
    await page.screenshot({ path: info.outputPath('split-thread.png') });
    await live.getByRole('tab', { name: 'Public thread', exact: true }).press('ArrowLeft');
    await expect(live.getByRole('tab', { name: 'Other live sessions', exact: true })).toBeFocused();
    await live.getByRole('button', { name: 'Full view', exact: true }).click();
    await expect(live.locator('.live-column-side')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}


test('local video, mobile camera selection, and manual thumbnail priority', async ({ page }, info) => {
  test.setTimeout(60_000);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral&context=publicThreads');
  await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
  await page.evaluate(() => {
    Object.defineProperty(navigator.mediaDevices, 'enumerateDevices', { value: async () =>
      ['front', 'rear'].map((deviceId) => ({ deviceId, label: deviceId, kind: 'videoinput', groupId: '' })) });
    Object.defineProperty(navigator.mediaDevices, 'getUserMedia', { value: async (constraints: MediaStreamConstraints) => {
      const video = constraints.video as MediaTrackConstraints;
      const device = (video.deviceId as ConstrainDOMStringParameters | undefined)?.exact ?? 'front';
      const canvas = document.createElement('canvas');
      canvas.width = 640; canvas.height = 360;
      const context = canvas.getContext('2d')!;
      context.fillStyle = device === 'rear' ? '#16754a' : '#b35b30'; context.fillRect(0, 0, 640, 360);
      context.fillStyle = '#ffffff'; context.font = '40px sans-serif'; context.fillText(String(device), 40, 180);
      const stream = canvas.captureStream(10);
      const track = stream.getVideoTracks()[0];
      const settings = track.getSettings.bind(track);
      track.getSettings = () => ({ ...settings(), deviceId: String(device) });
      return stream;
    } });
  });
  await activeColumn(page, 'Public threads').locator('.shell-column-footer').getByRole('button', { name: 'Create public thread' }).click();
  const create = page.getByRole('dialog', { name: 'Create a public thread' });
  await create.getByRole('textbox', { name: 'Title' }).fill('Camera live');
  await create.getByRole('checkbox', { name: 'Start Live' }).check();
  const cover = await page.evaluate(() => {
    const canvas = document.createElement('canvas'); canvas.width = 384; canvas.height = 216;
    const context = canvas.getContext('2d')!; context.fillStyle = '#164ac4'; context.fillRect(0, 0, 384, 216);
    return canvas.toDataURL('image/png').split(',')[1];
  });
  await create.getByLabel('Choose thumbnail image').setInputFiles({ name: 'cover.png', mimeType: 'image/png', buffer: Buffer.from(cover, 'base64') });
  await expect(create.getByRole('img', { name: 'Live thumbnail' })).toBeVisible();
  await create.getByRole('button', { name: 'Create', exact: true }).click();
  const detail = activeColumn(page, 'Public thread');
  const tile = detail.getByRole('list', { name: 'Live now' }).getByRole('button', { name: /^Live Camera live / });
  await expect(tile.locator('img')).toBeVisible();
  await tile.click();
  const dialog = page.getByRole('dialog', { name: 'Camera live', exact: true });
  await dialog.getByRole('button', { name: 'Show large', exact: true }).click();
  const live = activeColumn(page, 'Live');
  await live.getByRole('button', { name: 'Broadcast camera' }).click();
  const bar = page.getByRole('region', { name: 'Broadcast controls for Camera live' });
  await expect(bar.getByRole('combobox', { name: 'Camera to use' })).toHaveValue('front');
  await expect(bar.getByRole('combobox', { name: 'Camera to use' })).toBeInViewport();
  await expect.poll(() => live.locator('video').evaluate((video: HTMLVideoElement) => video.videoWidth)).toBe(640);
  await page.screenshot({ path: info.outputPath('local-video.png') });
  const ids = await page.evaluate(async () => {
    const topic = 'kukuri:topic:general'; const session = (await window.__KUKURI_DESKTOP__!.listLiveSessions(topic, {
      kind: 'public_channel', channel_id: new URLSearchParams(location.hash.split('?')[1]).get('channel')!,
    }))[0];
    return { topic, session: session.session_id, chosen: await window.__KUKURI_DESKTOP__!.getLiveThumbnail(topic, session.session_id) };
  });
  await bar.getByRole('combobox', { name: 'Camera to use' }).selectOption('rear');
  await expect(bar.getByRole('combobox', { name: 'Camera to use' })).toHaveValue('rear');
  expect(await page.evaluate(({ topic, session }) => window.__KUKURI_DESKTOP__!.getLiveThumbnail(topic, session), ids)).toEqual(ids.chosen);
  await live.getByRole('button', { name: 'Open Live menu' }).click();
  await page.getByRole('menuitem', { name: 'Close Live' }).click();
  await tile.click();
  await dialog.getByRole('button', { name: 'Remove image', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Remove image', exact: true })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Close dialog' }).click();
  await bar.getByRole('combobox', { name: 'Camera to use' }).selectOption('front');
  await expect.poll(() => page.evaluate(({ topic, session }) => window.__KUKURI_DESKTOP__!.getLiveThumbnail(topic, session), ids)).toMatchObject({ custom: false, data_url: expect.stringMatching(/^data:image\/jpeg;base64,/) });
  await bar.getByRole('button', { name: 'Stop video broadcast' }).click();
  await expect(bar).toHaveCount(0);
});


for (const width of [1440, 390]) {
  test(`Japanese live carousel and layouts at ${width}px`, async ({ page }, info) => {
    await page.addInitScript(() => localStorage.setItem('kukuri.desktop.locale', 'ja'));
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
    await page.evaluate(async () => {
      const api = window.__KUKURI_DESKTOP__!;
      const topic = 'kukuri:topic:general';
      const thread = await api.createPublicChannel(topic, 'みんなで観る配信', '配信について、この公開スレッドで話しましょう。');
      const scope = { kind: 'public_channel' as const, channel_id: thread.channel_id };
      const canvas = document.createElement('canvas'); canvas.width = 384; canvas.height = 216;
      const context = canvas.getContext('2d')!; context.fillStyle = '#21594e'; context.fillRect(0, 0, 384, 216);
      context.fillStyle = '#ffffff'; context.font = '24px sans-serif'; context.fillText('ライブのサムネイル', 40, 115);
      for (const title of ['日本語のライブ', 'もう一つの配信']) {
        const id = await api.createLiveSession(topic, title, 'カメラや画面を使った配信です', scope, '配信を始めました。一緒に見てください。');
        await api.setLiveThumbnail({ topic, session_id: id, custom: true, data_url: canvas.toDataURL('image/jpeg', 0.5) });
      }
      location.hash = `/timeline?topic=kukuri%3Atopic%3Ageneral&channel=${thread.channel_id}`;
    });
    await expect(page.locator('.live-carousel-track img').first()).toBeVisible();
    await page.screenshot({ path: info.outputPath('ja-carousel.png') });
    await page.locator('.live-carousel-track button').filter({ hasText: '日本語のライブ' }).click();
    const dialog = page.getByRole('dialog', { name: '日本語のライブ', exact: true });
    await expect(dialog.getByText('配信を始めました。一緒に見てください。', { exact: true })).toBeVisible();
    await page.screenshot({ path: info.outputPath('ja-details.png') });
    await dialog.getByRole('button', { name: '大きく表示', exact: true }).click();
    const live = page.locator('.live-column');
    await expect(live).toHaveAttribute('data-layout', 'full');
    await live.getByRole('button', { name: '半分表示', exact: true }).click();
    await expect(live.getByRole('tabpanel').getByText('もう一つの配信', { exact: true })).toBeVisible();
    await page.screenshot({ path: info.outputPath('ja-split.png') });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}


test('live details keep the thread post behind the existing adult content gate', async ({ page }) => {
  await page.goto('/');
  await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
  await page.evaluate(async () => {
    const api = window.__KUKURI_DESKTOP__!;
    const topic = 'kukuri:topic:general';
    const thread = await api.createPublicChannel(topic, 'Labeled conversation', 'Adult text must stay hidden');
    const label = <T extends { object_id: string; content_labels?: string[] | null }>(post: T): T =>
      post.object_id === thread.first_post_id ? { ...post, content_labels: ['adult'] } : post;
    const getThread = api.getPublicChannel.bind(api);
    api.getPublicChannel = async (...args) => {
      const view = await getThread(...args);
      return view?.first_post ? { ...view, first_post: label(view.first_post) } : view;
    };
    const listTimeline = api.listTimeline.bind(api);
    api.listTimeline = async (...args) => { const page = await listTimeline(...args); return { ...page, items: page.items.map(label) }; };
    const listThread = api.listThread.bind(api);
    api.listThread = async (...args) => { const page = await listThread(...args); return { ...page, items: page.items.map(label) }; };
    await api.createLiveSession(topic, 'Gated live', '', { kind: 'public_channel', channel_id: thread.channel_id });
    location.hash = `/timeline?topic=kukuri%3Atopic%3Ageneral&channel=${thread.channel_id}`;
  });
  const detail = activeColumn(page, 'Public thread');
  await expect(detail.getByText('Adult text must stay hidden', { exact: true })).toHaveCount(0);
  await detail.locator('.live-carousel-track button').click();
  const dialog = page.getByRole('dialog', { name: 'Gated live', exact: true });
  await expect(dialog.locator('[data-testid^="post-adult-gated-"]')).toBeVisible();
  await expect(dialog.getByText('Adult text must stay hidden', { exact: true })).toHaveCount(0);
});

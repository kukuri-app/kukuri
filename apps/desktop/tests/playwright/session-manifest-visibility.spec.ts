import { expect, test } from '@playwright/test';
import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';
import type { SessionDisplayRequest } from '../../src/lib/api';

declare global { interface Window { sessionDisplayCalls: SessionDisplayRequest[] } }

test('an individual live requests its missing manifest when displayed and releases it when hidden', async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 350 });
  await page.addInitScript((key) => localStorage.setItem(key, 'true'), DEVELOPER_MODE_STORAGE_KEY);
  await page.goto('/');
  await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
  await page.evaluate(() => {
    const api = window.__KUKURI_DESKTOP__!;
    window.sessionDisplayCalls = [];
    api.getLiveSession = async () => null;
    api.setSessionDisplay = async (request) => { window.sessionDisplayCalls.push(request); };
    location.hash = '/live?sessionId=live-late';
  });
  const live = page.getByRole('region', { name: /^Live Column/ });
  await expect.poll(() => page.evaluate(() => window.sessionDisplayCalls.some((request) => request.session_id === 'live-late' && request.visible))).toBe(true);
  await expect(live.getByText("This live session can't be loaded", { exact: true })).toBeVisible();
  await live.evaluate((element) => { (element as HTMLElement).style.display = 'none'; });
  await expect.poll(() => page.evaluate(() => window.sessionDisplayCalls.at(-1)?.visible)).toBe(false);
});

test('a completed manifest acquisition updates the visible session list without navigating again', async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 980 });
  await page.addInitScript((key) => window.localStorage.setItem(key, 'true'), DEVELOPER_MODE_STORAGE_KEY);
  await page.goto('/');
  await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
  await page.evaluate(async () => {
    const api = window.__KUKURI_DESKTOP__!;
    const topic = 'kukuri:topic:general';
    const id = await api.createLiveSession(topic, 'Delivered session', 'manifest arrived');
    const sessions = await api.listLiveSessions(topic);
    const getStatus = api.getSyncStatus.bind(api);
    let ready = false;
    api.listLiveSessions = async () => ready ? sessions : [];
    const getLiveSession = api.getLiveSession.bind(api);
    api.getLiveSession = async (topic, session) => ready ? getLiveSession(topic, session) : null;
    api.listSessionCandidates = async () => ready ? [] : [{ replica_id: 'test-replica', session_id: id, kind: 'live' }];
    api.getSyncStatus = async () => ({ ...await getStatus(), last_sync_ts: ready ? 2000 : 1000 });
    api.setSessionDisplay = async (request) => { if (request.visible) ready = true; };
    location.hash = `/live?sessionId=${id}`;
  });
  // 開始 post の中の Live も timeline の列に同じタイトルを出すので、Live の列の中で確かめる。
  const live = page.getByRole('region', { name: /^Live Column/ });
  await expect(live.getByText('Delivered session', { exact: true })).toBeVisible({ timeout: 15000 });
  await expect(live.getByText('Session information is not available yet.', { exact: true })).toHaveCount(0);
});

test('opening an uncached detail brings its candidate into view past existing cards', async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 350 });
  await page.addInitScript((key) => window.localStorage.setItem(key, 'true'), DEVELOPER_MODE_STORAGE_KEY);
  await page.goto('/');
  await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
  await page.evaluate(async () => {
    const api = window.__KUKURI_DESKTOP__!;
    await api.createLiveSession('kukuri:topic:general', 'Existing live', 'already listed');
    window.sessionDisplayCalls = [];
    api.listSessionCandidates = async () => [];
    api.setSessionDisplay = async (request) => { window.sessionDisplayCalls.push(request); };
  });
  await page.addStyleTag({ content: '.shell-stream-layout .post-card { min-height: 600px; }' });
  await page.evaluate(() => { window.location.hash = '/live?sessionId=live-target'; });
  await expect.poll(() => page.evaluate(() => window.sessionDisplayCalls.some((request) => request.session_id === 'live-target' && request.visible))).toBe(true);
  await expect(page.getByText("This live session can't be loaded", { exact: true })).toBeVisible();
});

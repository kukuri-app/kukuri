import { expect, test } from '@playwright/test';

declare global {
  interface Window {
    __profileRefreshTest: { calls: number; hold: boolean; fail: boolean; release: () => void };
  }
}

for (const width of [1280, 1024, 390]) {
  for (const populated of [false, true]) {
    test(`profile refresh keeps its layout at ${width}px, populated=${populated}`, async ({ page }, testInfo) => {
      const locale = width === 1280 ? 'ja' : width === 1024 ? 'en' : 'zh-CN';
      await page.setViewportSize({ width, height: 800 });
      await page.addInitScript(({ locale, populated }) => {
        localStorage.setItem('kukuri.desktop.locale', locale);
        localStorage.setItem('kukuri.desktop.theme', populated ? 'light' : 'dark');
        window.__profileRefreshTest = { calls: 0, hold: false, fail: false, release: () => undefined };
        let desktopApi = window.__KUKURI_DESKTOP__;
        Object.defineProperty(window, '__KUKURI_DESKTOP__', {
          configurable: true, get: () => desktopApi,
          set: (api: typeof desktopApi) => {
            desktopApi = api;
            if (!api) return;
            const seeded = populated ? api.createPost('kukuri:topic:general', 'Profile layout regression post') : Promise.resolve();
            const profile = api.getMyProfile.bind(api);
            api.getMyProfile = async () => ({ ...await profile(), display_name: 'きんぎょ Display Name', name: 'kingyosun', picture_asset: null });
            const read = api.listProfileTimeline.bind(api);
            api.listProfileTimeline = async (...args) => {
              const control = window.__profileRefreshTest;
              control.calls += 1;
              await seeded;
              if (control.hold) await new Promise<void>((resolve) => { control.release = resolve; });
              if (control.fail) throw new Error('Profile refresh test failure');
              return read(...args);
            };
          },
        });
      }, { locale, populated });
      await page.goto('/');
      const profile = page.locator('[data-column-id]').filter({ has: page.locator('.profile-overview-header') });
      const refresh = profile.locator('.shell-column-context-actions button[aria-busy]');
      await expect(refresh).toHaveAttribute('aria-busy', 'false');
      const names = profile.locator('.profile-overview-names');
      await expect(names.locator('h3')).toHaveText('きんぎょ Display Name');
      await expect(names.locator('small')).toHaveText('kingyosun');
      await expect(profile.locator('.profile-overview-header')).toHaveCSS('padding-bottom', '4px');
      const assertSameRow = async () => {
        const [identityBox, editBox] = await profile.locator('.profile-overview-header').evaluate((header) =>
          [...header.children].map((node) => node.getBoundingClientRect().toJSON())
        );
        expect(editBox.x).toBeGreaterThanOrEqual(identityBox.x + identityBox.width);
        expect(editBox.y).toBeLessThan(identityBox.y + identityBox.height);
        expect(editBox.y + editBox.height).toBeGreaterThan(identityBox.y);
      };
      await assertSameRow();
      const initialCalls = await page.evaluate(() => window.__profileRefreshTest.calls);
      for (const kind of ['profile', 'explore', 'timeline', 'profile']) {
        const column = page.locator(`.shell-column-surface[data-column-id^="column:${kind}:"]`).first();
        await column.scrollIntoViewIfNeeded();
        await column.click({ position: { x: 20, y: 450 } });
      }
      await expect.poll(() => page.evaluate(() => window.__profileRefreshTest.calls)).toBe(initialCalls);
      await refresh.scrollIntoViewIfNeeded();
      const body = profile.locator('.shell-column-body');
      const before = await body.boundingBox();
      const summary = profile.locator('.profile-overview-connections');
      const summaryBefore = await summary.boundingBox();
      const feed = profile.locator('.shell-column-content').last();
      const feedBefore = await feed.boundingBox();
      await page.screenshot({ path: testInfo.outputPath('profile-before.png') });
      await page.evaluate(() => { window.__profileRefreshTest.hold = true; });
      await refresh.click();
      await expect(refresh).toHaveAttribute('aria-busy', 'true');
      await expect(refresh.locator('svg')).toHaveCSS('animation-name', 'profile-refresh-rotation');
      await refresh.press('Enter');
      await expect.poll(() => page.evaluate(() => window.__profileRefreshTest.calls)).toBe(initialCalls + 1);
      // Sample while the response is held; a final screenshot would miss the original flicker.
      for (let frame = 0; frame < 4; frame += 1) {
        await page.evaluate(() => new Promise(requestAnimationFrame));
        expect(await summary.boundingBox()).toEqual(summaryBefore);
        expect(await feed.boundingBox()).toEqual(feedBefore);
        expect(await body.boundingBox()).toEqual(before);
      }
      await page.screenshot({ path: testInfo.outputPath('profile-refreshing.png') });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await expect(refresh.locator('svg')).toHaveCSS('animation-name', 'none');
      await page.emulateMedia({ reducedMotion: 'no-preference' });
      await page.evaluate(() => { document.documentElement.dataset.reducedMotion = 'reduce'; });
      await expect(refresh.locator('svg')).toHaveCSS('animation-name', 'none');
      await page.evaluate(() => {
        delete document.documentElement.dataset.reducedMotion;
        window.__profileRefreshTest.hold = false;
        window.__profileRefreshTest.release();
      });
      await expect(refresh).toHaveAttribute('aria-busy', 'false');
      await expect(refresh).toBeFocused();
      expect(await feed.boundingBox()).toEqual(feedBefore);
      await page.screenshot({ path: testInfo.outputPath('profile-after.png') });
      await page.evaluate(() => { window.__profileRefreshTest.fail = true; });
      await refresh.click();
      await expect(profile.getByText('Profile refresh test failure')).toBeVisible();
      await expect(refresh).toHaveAttribute('aria-busy', 'false');
      await expect(feed).toBeVisible();
      await page.evaluate(() => { window.__profileRefreshTest.fail = false; });
      await refresh.click();
      await expect(profile.getByText('Profile refresh test failure')).toHaveCount(0);
      await page.evaluate(() => {
        const api = window.__KUKURI_DESKTOP__!;
        const read = api.getMyProfile.bind(api);
        api.getMyProfile = async () => ({ ...await read(),
          display_name: 'とても長い表示名🐟'.repeat(8), name: 'long_username_'.repeat(10),
        });
      });
      await refresh.click();
      await expect(names.locator('small')).toHaveText('long_username_'.repeat(10));
      for (const element of [names, names.locator('h3'), names.locator('small')]) {
        expect(await element.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(true);
      }
      await assertSameRow();
      await page.screenshot({ path: testInfo.outputPath('profile-long-names.png') });
    });
  }
}

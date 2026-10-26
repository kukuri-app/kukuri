import { expect, test } from '@playwright/test';
import { browserDesktopMockSeed } from '../../src/mocks/browserSeed';

const NATIVE = 'b'.repeat(64);
const PEER = 'c'.repeat(64);
const TOPIC = 'kukuri:topic:general';

declare global {
  interface Window {
    __dmFollowRace: {
      profiles: Array<() => void>;
      publicReads: Array<() => void>;
      holdPublic: boolean;
      finished: number;
      clicks: number;
    };
  }
}

test('follow前のプロフィール応答がMessage押下中に届いても指定した相手の会話を開いて送信する', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  const post = { ...browserDesktopMockSeed.seedPosts![TOPIC][0],
    object_id: 'web-peer-post', envelope_id: 'web-peer-envelope', root_id: 'web-peer-post', author_pubkey: PEER, author_name: 'web peer',
    content: 'web peer post', following: false, followed_by: true, mutual: false,
  };
  const seed = { ...browserDesktopMockSeed,
    seedPosts: { [TOPIC]: [...browserDesktopMockSeed.seedPosts![TOPIC], post] },
    authorSocialViews: { ...browserDesktopMockSeed.authorSocialViews,
      [PEER]: { name: 'web peer', following: false, followed_by: true, mutual: false },
    },
  };
  await page.addInitScript(({ seed, peer }) => {
    localStorage.setItem('kukuri.desktop.locale', 'en');
    localStorage.setItem('kukuri.desktop.theme', 'dark');
    Object.assign(window, { __KUKURI_PROMO_MOCK_SEED__: seed });
    const control = window.__dmFollowRace = { profiles: [], publicReads: [], holdPublic: false, finished: 0, clicks: 0 } as Window['__dmFollowRace'];
    let followed = false;
    let api: typeof window.__KUKURI_DESKTOP__;
    Object.defineProperty(window, '__KUKURI_DESKTOP__', {
      configurable: true, get: () => api,
      set: (value: NonNullable<typeof api>) => {
        api = value;
        const profile = value.listProfileTimeline.bind(value);
        value.listProfileTimeline = async (key, ...args) => {
          const result = await profile(key, ...args);
          if (key === peer && !followed) {
            await new Promise<void>(resolve => control.profiles.push(resolve));
            control.finished++;
          }
          return result;
        };
        const follow = value.followAuthor.bind(value);
        value.followAuthor = async key => {
          const result = await follow(key);
          if (key === peer) { followed = true; control.holdPublic = true; }
          return result;
        };
        const timeline = value.listTimeline.bind(value);
        value.listTimeline = async (...args) => {
          if (control.holdPublic) await new Promise<void>(resolve => control.publicReads.push(resolve));
          return timeline(...args);
        };
      },
    });
    document.addEventListener('click', event => {
      const button = (event.target as Element).closest('.author-detail-action-buttons button');
      if (button?.textContent === 'Message' && button.closest<HTMLElement>('section[data-column-id]')?.dataset.columnId?.endsWith(`:${peer}`)) control.clicks++;
    }, true);
  }, { seed, peer: PEER });
  await page.goto('/');
  const publicColumn = page.locator('section[data-column-id^="column:timeline:"][data-column-id$=":-:-"]');
  await publicColumn.locator('article').filter({ hasText: 'browser mock peer post' }).locator('button.post-meta-author').click();
  await page.locator(`section[data-column-id$=":${NATIVE}"]`).getByRole('button', { name: 'Message', exact: true }).click();
  await expect(page.locator(`section[data-column-id^="column:conversation:"][data-column-id$=":${NATIVE}"]`)).toBeVisible();
  await publicColumn.locator('article').filter({ hasText: 'web peer post' }).locator('button.post-meta-author').click();
  const profile = page.locator(`section[data-column-id^="column:profile:"][data-column-id$=":${PEER}"]`);
  await expect.poll(() => page.evaluate(() => window.__dmFollowRace.profiles.length)).toBeGreaterThan(0);
  await profile.getByRole('button', { name: 'Follow', exact: true }).click();
  const message = profile.getByRole('button', { name: 'Message', exact: true });
  await message.hover();
  await page.screenshot({ path: testInfo.outputPath('before-message.png') });
  await message.evaluate(button => button.addEventListener('pointerdown', () => {
    for (const finish of window.__dmFollowRace.profiles.splice(0)) finish();
  }, { once: true }));
  // 自動再clickを使わず、古い応答の反映をpointerdownとpointerupの間へ置く。
  await page.mouse.down();
  await expect.poll(() => page.evaluate(() => window.__dmFollowRace.finished)).toBeGreaterThan(0);
  await page.evaluate(() => new Promise(requestAnimationFrame));
  await page.mouse.up();
  await page.screenshot({ path: testInfo.outputPath('after-message.png') });
  await page.evaluate(() => {
    window.__dmFollowRace.holdPublic = false;
    for (const finish of window.__dmFollowRace.publicReads.splice(0)) finish();
  });
  await expect(message).toBeVisible();
  expect(await page.evaluate(() => window.__dmFollowRace.clicks)).toBe(1);
  const conversation = page.locator(`section[data-column-id^="column:conversation:"][data-column-id$=":${PEER}"]`);
  await expect(conversation).toBeVisible();
  const composer = conversation.getByPlaceholder('Write a message');
  if (!(await composer.isVisible())) await conversation.locator('button[aria-label^="Message to "]').click();
  await composer.fill('DM after follow');
  await conversation.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(conversation.getByText('DM after follow', { exact: true })).toBeVisible();
});

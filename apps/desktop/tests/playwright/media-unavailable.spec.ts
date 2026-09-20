import { expect, test, type Page } from '@playwright/test';

import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

const MEDIA_HASH = 'a'.repeat(64);

async function installUnavailableMediaScenario(
  page: Page,
  options: { developerMode: boolean; rejectInitialFetch: boolean }
) {
  await page.addInitScript(
    ({ developerModeKey, developerMode, mediaHash, rejectInitialFetch }) => {
      window.localStorage.setItem(developerModeKey, developerMode ? 'true' : 'false');
      let desktopApi: typeof window.__KUKURI_DESKTOP__;
      Object.defineProperty(window, '__KUKURI_DESKTOP__', {
        configurable: true,
        get: () => desktopApi,
        set: (value: NonNullable<typeof window.__KUKURI_DESKTOP__>) => {
          const originalListTimeline = value.listTimeline.bind(value);
          value.listTimeline = async (topic, cursor, limit, channelId) => {
            if (topic !== 'kukuri:topic:general') {
              return originalListTimeline(topic, cursor, limit, channelId);
            }
            const recovered = window.sessionStorage.getItem('issue-814-recovered') === 'true';
            return {
              items: [
                {
                  object_id: 'browser-unavailable',
                  envelope_id: 'browser-unavailable-envelope',
                  author_pubkey: 'f'.repeat(64),
                  author_name: 'fixture-author',
                  author_display_name: 'Fixture Author',
                  following: false,
                  followed_by: false,
                  mutual: false,
                  friend_of_friend: false,
                  object_kind: 'post',
                  is_threadable: true,
                  content: recovered ? 'Recovered body' : '[blob pending]',
                  content_status: recovered ? 'Available' : 'Missing',
                  attachments: [
                    {
                      hash: mediaHash,
                      mime: 'image/png',
                      bytes: 2048,
                      role: 'image_original',
                      status: recovered ? 'Available' : 'Missing',
                    },
                  ],
                  created_at: 1,
                  reply_to: null,
                  root_id: 'browser-unavailable',
                  channel_id: null,
                  audience_label: 'Public',
                },
              ],
              next_cursor: null,
            };
          };
          value.getBlobMediaPayload = async (_hash, mime) => {
            if (window.sessionStorage.getItem('issue-814-recovered') === 'true') {
              return { bytes_base64: 'ZmFrZS1pbWFnZQ==', mime };
            }
            if (rejectInitialFetch) {
              throw new Error('fixture blob unavailable');
            }
            return null;
          };
          desktopApi = value;
        },
      });
    },
    {
      developerModeKey: DEVELOPER_MODE_STORAGE_KEY,
      developerMode: options.developerMode,
      mediaHash: MEDIA_HASH,
      rejectInitialFetch: options.rejectInitialFetch,
    }
  );
}

// #1207: 自動取得は 5 秒・30 秒の待ちをはさんで 3 回試みる。実時間を待たず、時計を進めて上限へ到達させる。
async function exhaustAutomaticMediaFetch(page: Page) {
  for (const delay of [6_000, 31_000]) {
    await page.clock.runFor(delay);
  }
}

test('normal mode replaces unavailable media with the fetch failure and a retry control', async ({
  page,
}) => {
  await page.clock.install();
  await installUnavailableMediaScenario(page, {
    developerMode: false,
    rejectInitialFetch: false,
  });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  await expect(page.locator('[data-post-object-id="browser-unavailable"]')).toBeVisible();
  await expect(page.getByText('browser-unavailable-envelope')).toHaveCount(0);
  await expect(page.getByText('image/png')).toHaveCount(0);
  await expect(page.getByTestId('text-skeleton-browser-unavailable')).toHaveCount(0);
  await expect(page.getByTestId('media-skeleton-browser-unavailable')).toHaveCount(0);
  await expect(page.getByText('Content unavailable.')).toHaveCount(0);

  await exhaustAutomaticMediaFetch(page);
  await expect(page.getByText('Failed to load.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Retry loading' })).toBeVisible();
  await expect(page.getByTestId('media-skeleton-browser-unavailable')).toHaveCount(0);
});

test('developer mode reports a rejected fetch and an existing refresh can recover it', async ({
  page,
}) => {
  await installUnavailableMediaScenario(page, {
    developerMode: true,
    rejectInitialFetch: true,
  });
  await page.clock.install();
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  await expect(page.getByText('Content unavailable.')).toBeVisible();
  await exhaustAutomaticMediaFetch(page);
  await expect(page.getByText('Failed to load.')).toBeVisible();
  await expect(page.getByText('fixture blob unavailable')).toHaveCount(0);

  await page.evaluate(() => {
    window.sessionStorage.setItem('issue-814-recovered', 'true');
  });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Adev');
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');

  await expect(page.getByText('Recovered body')).toBeVisible();
  await expect(page.getByTestId('media-preview-browser-unavailable')).toBeVisible();
  await expect(page.getByText('Content unavailable.')).toHaveCount(0);
  await expect(page.getByText('Failed to load.')).toHaveCount(0);
});

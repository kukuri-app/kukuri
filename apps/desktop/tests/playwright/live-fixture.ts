import { expect, type Page } from '@playwright/test';

/** カラムの操作を検証するため、個別ライブをfixtureとして作って開く。 */
export async function openFixtureLive(page: Page, title = 'Column test live') {
  await page.waitForFunction(() => Boolean(window.__KUKURI_DESKTOP__));
  await page.evaluate(async (title) => {
    const id = await window.__KUKURI_DESKTOP__!.createLiveSession('kukuri:topic:general', title, 'Column continuity');
    location.hash = `/live?topic=kukuri%3Atopic%3Ageneral&sessionId=${encodeURIComponent(id)}`;
  }, title);
  const column = page.getByRole('region', { name: /^Live Column,.*Active,/ });
  await expect(column.locator('.live-stage')).toContainText(title);
  return column;
}

import { eventually } from './wait.mjs';

/** 先頭にいないときの新着は「Show N new post(s)」を押すと並ぶ。列の待機では、その列のボタンだけを押す。 */
export async function showNewPosts(root) {
  const button = root.$('.//button[starts-with(normalize-space(.), "Show ") and contains(., "new post")]');
  if (await button.isExisting()) await button.click().catch(() => undefined);
}

/** 列（id は `column:<kind>:<topic>:<channel>:<相手>`。channel と相手は無ければ `-`）。 */
export const columnOf = (browser, kind, tail) =>
  browser.$(`section[data-column-id^="column:${kind}:"][data-column-id$=":${tail}"]`);
export const publicColumn = (browser) => columnOf(browser, 'timeline', '-:-');
export const card = (root, text) => root.$(`article*=${text}`);

/** 投稿の作者（`author`）の profile の列を開く。開いていれば閉じて開き直す（列の中の設定は開いたときに読む）。 */
export async function reopenAuthorProfile(browser, authorPost, author, timeout = 90_000) {
  const opened = columnOf(browser, 'profile', author);
  if (await opened.isExisting()) {
    // 列の menu から閉じる（電話の幅では列の見出しの閉じる button は出ない）。
    await opened.$('button[aria-label^="Open "][aria-label$=" menu"]').click();
    await browser.$('//button[@role="menuitem"][starts-with(normalize-space(.), "Close ")]').click();
    await opened.waitForExist({ reverse: true, timeout });
  }
  const timeline = publicColumn(browser);
  // 閉じる前のページで見えていても、公開列では新着の反映・描画がまだの場合がある。
  const post = await eventually(`public post "${authorPost}"`, async () => {
    await showNewPosts(timeline);
    const target = await card(timeline, authorPost);
    return (await target.isExisting()) && target;
  }, timeout);
  await post.$('button.post-meta-author').click();
  const profile = columnOf(browser, 'profile', author);
  await profile.waitForExist({ timeout });
  return profile;
}

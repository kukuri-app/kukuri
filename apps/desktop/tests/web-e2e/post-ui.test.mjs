import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { remote } from 'webdriverio';
import { card, publicColumn, reopenAuthorProfile } from './post-ui.mjs';
import { eventually } from './wait.mjs';

const POST = 'native post for the profile';
let browser;
let missingReads;
before(async () => {
  browser = await remote({ logLevel: 'error', waitforTimeout: 1000, capabilities: {
    browserName: 'chrome',
    'goog:chromeOptions': { args: ['--headless=new', '--no-sandbox'] },
    ...(process.env.CHROMEDRIVER ? { 'wdio:chromedriverOptions': { binary: process.env.CHROMEDRIVER } } : {}),
  } });
  // 最初の未描画の確認まで描画を止め、要素を読み直す必要のある順序を固定する。
  browser.overwriteCommand('isExisting', async function (original, ...args) {
    const exists = await original(...args);
    if (this.selector === `article*=${POST}` && !exists) {
      missingReads++;
      await browser.execute(() => window.renderPost());
    }
    return exists;
  }, true);
});
after(async () => { await browser?.deleteSession(); });

async function fixture({ arrives = true, name = 'updated name' } = {}) {
  missingReads = 0;
  await browser.url(`data:text/html,${encodeURIComponent(`
    <section data-column-id="column:timeline:general:private:-"><button onclick="throw Error('wrong column')">Show 1 new post</button>
      ${arrives ? '' : `<article>${POST}<button class="post-meta-author" onclick="throw Error('wrong post')">other</button></article>`}
    </section>
    <section data-column-id="column:timeline:general:-:-" id="timeline"><button onclick="window.pending = true">Show 1 new post</button></section>
    <section data-column-id="column:profile:general:-:native" id="profile">
      <button aria-label="Open native menu" onclick="document.getElementById('close').hidden = false">menu</button>
      <article>${POST}</article>
    </section>
    <button role="menuitem" id="close" hidden onclick="document.getElementById('profile').remove(); this.hidden = true">Close native</button>
    <script>
      window.renderPost = () => {
        if (!window.pending || !${arrives} || document.querySelector('#timeline article')) return;
        const post = document.createElement('article');
        post.textContent = '${POST}';
        const author = document.createElement('button');
        author.className = 'post-meta-author'; author.textContent = 'native';
        author.onclick = () => {
          const profile = document.createElement('section');
          profile.dataset.columnId = 'column:profile:general:-:native';
          profile.textContent = '${name}'; document.body.append(profile);
        };
        post.append(author); document.getElementById('timeline').append(post);
      };
    </script>`)}`);
}

test('プロフィールを閉じた後、公開列の新着反映と対象投稿の描画を待って開き直す', async () => {
  await fixture();
  assert.ok((await browser.execute(() => document.body.innerText)).includes(POST), '閉じる前のページには投稿がある');
  const profile = await reopenAuthorProfile(browser, POST, 'native', 2000);
  assert.ok(missingReads > 0, '未描画を観測してから再取得した');
  assert.equal(await profile.getText(), 'updated name');
  assert.ok(await (await card(publicColumn(browser), POST)).isExisting(), '公開列の投稿を操作した');
});

test('投稿が届かなければ、ほかの列の同じ文で成功しない', async () => {
  await fixture({ arrives: false });
  await assert.rejects(reopenAuthorProfile(browser, POST, 'native', 100), /timed out:/);
});

test('まだ開いていない作者のプロフィールも公開列の投稿から開く', async () => {
  await fixture();
  await browser.execute(() => document.getElementById('profile').remove());
  assert.equal(await (await reopenAuthorProfile(browser, POST, 'native', 2000)).getText(), 'updated name');
});

test('プロフィールを開けても名前が反映されなければ失敗する', async () => {
  await fixture({ name: 'old name' });
  await assert.rejects(eventually('updated public profile', async () =>
    (await (await reopenAuthorProfile(browser, POST, 'native', 2000)).getText()).includes('updated name'), 0), /timed out: updated public profile/);
});

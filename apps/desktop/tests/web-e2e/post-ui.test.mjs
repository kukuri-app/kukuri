import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { after, before, test } from 'node:test';
import { remote } from 'webdriverio';
import { card, publicColumn, reopenAuthorProfile } from './post-ui.mjs';
import { eventually } from './wait.mjs';

const POST = 'native post for the profile';
const PEER = 'b'.repeat(64);
const OTHER = 'a'.repeat(64);
let browser;
let missingReads;
let server;
let origin;
before(async () => {
  const init = await readFile(new URL('./page-init.js', import.meta.url), 'utf8');
  server = createServer((request, response) => {
    response.setHeader('content-type', request.url === '/page-init.js' ? 'text/javascript' : 'text/html');
    response.end(request.url === '/page-init.js' ? init : '<script src="/page-init.js"></script><body></body>');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
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
after(async () => {
  await browser?.deleteSession();
  if (server) await new Promise(resolve => server.close(resolve));
});

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

// 別processのWebDriver自動取得と起動が競合しないよう、同じsessionを使う（#1943）。
const events = () => browser.execute(() => window.__kukuriDirectMessageEvents);
async function traceFixture() {
  await browser.url(origin);
  await browser.execute((peer, other) => {
    document.body.innerHTML = [other, peer].map(key => `
      <section data-column-id="column:profile:general:-:${key}">
        <div class="author-detail-action-buttons"><button><span>Message</span></button><button>Unfollow</button></div>
      </section>`).join('');
  }, PEER, OTHER);
}

test('指定した相手への実クリックと会話列の生成・消失を順に記録する', async () => {
  await traceFixture();
  const profile = browser.$(`section[data-column-id$=":${PEER}"]`);
  await profile.$('button=Unfollow').click();
  await profile.$('span=Message').click();
  await browser.execute(peer => {
    const parent = document.createElement('div');
    parent.innerHTML = `<section data-column-id="column:conversation:general:-:${peer}">private message body</section>`;
    document.body.append(parent);
    parent.remove();
  }, PEER);
  const recorded = await eventually('DMの列の消失記録', async () => {
    const value = await events();
    return value?.some(event => event.phase === 'column-removed') && value;
  }, 2000);
  assert.deepEqual(recorded.map(event => [event.phase, event.peerPubkey]), [
    ['message-click', PEER], ['column-added', PEER], ['column-removed', PEER],
  ]);
  assert.equal(recorded[0].isTrusted, true);
  assert.ok(recorded.every(event => typeof event.at === 'number'));
  assert.ok(!JSON.stringify(recorded).includes('private message body'));
});

test('pointerdownで要素が外れ、実clickが届かなければクリック済みと記録しない', async () => {
  await traceFixture();
  await browser.execute(peer => {
    document.querySelector(`section[data-column-id$=":${peer}"] button`).onpointerdown = event => event.currentTarget.remove();
  }, PEER);
  await browser.$(`section[data-column-id$=":${PEER}"] button`).click();
  assert.deepEqual(await events(), []);
});

test('APIの記録は直近64件を保ち、reloadで消える', async () => {
  await traceFixture();
  await browser.execute(peer => {
    for (let id = 1; id <= 65; id++) console.info('[kukuri.e2e.dm]', {
      phase: 'api-fulfilled', openId: id, peerPubkey: peer, method: 'openDirectMessage',
    });
  }, PEER);
  const recorded = await events();
  assert.equal(recorded.length, 64);
  assert.equal(recorded[0].openId, 2);
  assert.equal(recorded.at(-1).openId, 65);
  await browser.refresh();
  assert.deepEqual(await events(), []);
});

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { after, before, test } from 'node:test';
import { remote } from 'webdriverio';
import { eventually } from './wait.mjs';

const PEER = 'b'.repeat(64);
const OTHER = 'a'.repeat(64);
let browser;
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
  browser = await remote({ logLevel: 'error', capabilities: {
    browserName: 'chrome',
    'goog:chromeOptions': { args: ['--headless=new', '--no-sandbox'] },
    ...(process.env.CHROMEDRIVER ? { 'wdio:chromedriverOptions': { binary: process.env.CHROMEDRIVER } } : {}),
  } });
});
after(async () => {
  await browser?.deleteSession();
  if (server) await new Promise(resolve => server.close(resolve));
});

const events = () => browser.execute(() => window.__kukuriDirectMessageEvents);
async function fixture() {
  await browser.url(origin);
  await browser.execute((peer, other) => {
    document.body.innerHTML = [other, peer].map(key => `
      <section data-column-id="column:profile:general:-:${key}">
        <div class="author-detail-action-buttons"><button><span>Message</span></button><button>Unfollow</button></div>
      </section>`).join('');
  }, PEER, OTHER);
}

test('指定した相手への実クリックと会話列の生成・消失を順に記録する', async () => {
  await fixture();
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
  await fixture();
  await browser.execute(peer => {
    document.querySelector(`section[data-column-id$=":${peer}"] button`).onpointerdown = event => event.currentTarget.remove();
  }, PEER);
  await browser.$(`section[data-column-id$=":${PEER}"] button`).click();
  assert.deepEqual(await events(), []);
});

test('APIの記録は直近64件を保ち、reloadで消える', async () => {
  await fixture();
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

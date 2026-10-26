import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { gzipSync } from 'node:zlib';
import { chromium } from '@playwright/test';
import { pwaWorkerSource } from '../../scripts/webPwaShell.mjs';
import { waitForUnusedShellCache } from './pwa-shell-cache.mjs';

// 静的shellのcontract。SNS/WASM/Androidの受入試験は別の実scenarioで行う。
test('offline shell, explicit update, live-client retention, and static-only cache', { timeout: 45_000 }, async () => {
  let version = 'one';
  let failPreparation = false;
  const server = createServer(async (request, response) => {
    response.setHeader('Cache-Control', 'no-store');
    if (request.url === '/sw.js') {
      response.setHeader('Content-Type', 'text/javascript');
      response.end(await pwaWorkerSource(version, ['/', `/app-${version}.js`]));
    } else if (request.url === '/') {
      response.setHeader('Content-Type', 'text/html');
      response.setHeader('Content-Encoding', 'gzip');
      response.end(gzipSync(`<meta name="kukuri-shell-build" content="${version}"><script src="/app-${version}.js"></script><main>${version}</main>`));
    } else if (request.url === `/app-${version}.js`) {
      if (failPreparation) response.writeHead(503);
      else response.setHeader('Content-Type', 'text/javascript');
      response.end('globalThis.shellScriptLoaded = true;');
    } else {
      response.setHeader('Content-Type', 'application/json');
      response.end('{"private":"never cache API content"}');
    }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  const context = await browser.newContext();
  const cacheNames = page => page.evaluate(() => caches.keys());
  const report = page => page.evaluate(async () => {
    const worker = (await navigator.serviceWorker.ready).active;
    const channel = new MessageChannel();
    const ack = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('shell retention acknowledgement timed out')), 5000);
      channel.port1.onmessage = () => { clearTimeout(timeout); channel.port1.close(); resolve(); };
    });
    worker.postMessage({ type: 'hello', version: document.querySelector('meta[name="kukuri-shell-build"]').content }, [channel.port2]);
    await ack;
  });
  try {
    const first = await context.newPage();
    await first.goto(origin);
    await first.evaluate(() => navigator.serviceWorker.register('/sw.js'));
    await first.waitForFunction(() => navigator.serviceWorker.controller !== null);
    await report(first);
    await first.evaluate(() => fetch('/api/private'));
    assert.deepEqual(await first.evaluate(async () => (await (await caches.open('kukuri-shell-one')).keys()).map(request => new URL(request.url).pathname).sort()), ['/', '/app-one.js']);
    await context.setOffline(true);
    await first.reload();
    assert.equal(await first.locator('main').textContent(), 'one');
    await report(first);
    await context.setOffline(false);

    const old = await context.newPage();
    await old.goto(origin);
    await report(old);
    version = 'two';
    await first.evaluate(async () => (await navigator.serviceWorker.getRegistration()).update());
    await first.waitForFunction(async () => (await navigator.serviceWorker.getRegistration()).waiting !== null);
    assert.equal(await first.locator('main').textContent(), 'one', 'prepared update does not reload input');
    await first.evaluate(async () => {
      const registration = await navigator.serviceWorker.getRegistration();
      const changed = new Promise(resolve => navigator.serviceWorker.addEventListener('controllerchange', resolve, { once: true }));
      registration.waiting.postMessage({ type: 'apply' });
      await changed;
    });
    await first.reload();
    await report(first);
    assert.deepEqual((await cacheNames(first)).sort(), ['kukuri-shell-one', 'kukuri-shell-two']);
    await context.setOffline(true);
    assert.equal(await old.evaluate(() => fetch('/app-one.js').then(response => response.text())), 'globalThis.shellScriptLoaded = true;');
    await context.setOffline(false);
    await old.close();
    await waitForUnusedShellCache({ execute: callback => first.evaluate(callback) });
    assert.deepEqual(await cacheNames(first), ['kukuri-shell-two']);

    version = 'three'; failPreparation = true;
    await first.evaluate(async () => (await navigator.serviceWorker.getRegistration()).update()).catch(() => undefined);
    await first.waitForFunction(async () => (await navigator.serviceWorker.getRegistration()).installing === null);
    assert.deepEqual(await cacheNames(first), ['kukuri-shell-two']);
    await context.setOffline(true);
    await first.reload();
    assert.equal(await first.locator('main').textContent(), 'two', 'preparation failure preserves the prior usable build');
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});

// incognitoのHTTP cacheは1 entryを数MBまでしか持てない。同じ資源を並行に取得すると、
// 再検証で書込み途中のentryを読む側が、entryの破棄で失敗する（#1901）。
test('install and the page can fetch one large asset at once in an in-memory HTTP cache', { timeout: 30_000 }, async () => {
  const large = randomBytes(8 << 20);
  let overlap;
  const overlapped = new Promise(resolve => { overlap = resolve; });
  let requests = 0;
  const server = createServer(async (request, response) => {
    if (request.url === '/sw.js') {
      response.setHeader('Content-Type', 'text/javascript');
      response.end(await pwaWorkerSource('one', ['/large.bin']));
      return;
    }
    if (request.url !== '/large.bin') { response.setHeader('Content-Type', 'text/html'); response.end('<main>one</main>'); return; }
    if (requests++) overlap();
    if (request.headers['if-none-match'] === '"large"') { response.writeHead(304, { ETag: '"large"' }); response.end(); return; }
    response.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Cache-Control': 'max-age=0, must-revalidate', ETag: '"large"' });
    // 2本目の取得が届くまで本体の残りを止め、2本の取得を必ず重ねる。
    response.write(large.subarray(0, 1 << 20));
    await Promise.race([overlapped, new Promise(resolve => setTimeout(resolve, 5000))]);
    response.end(large.subarray(1 << 20));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  try {
    const page = await (await browser.newContext()).newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    assert.deepEqual(await page.evaluate(async () => {
      const worker = (await navigator.serviceWorker.register('/sw.js')).installing;
      const installed = new Promise(resolve => worker.addEventListener('statechange', () => { if (worker.state !== 'installing') resolve(worker.state); }));
      const fetched = await fetch('/large.bin').then(response => response.arrayBuffer()).then(body => body.byteLength, error => error.message);
      return { fetched, installed: await installed };
    }), { fetched: 8 << 20, installed: 'installed' });
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});

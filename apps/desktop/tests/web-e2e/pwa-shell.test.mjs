import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { chromium } from '@playwright/test';
import { pwaWorkerSource } from '../../scripts/webPwaShell.mjs';

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
      response.end(`<meta name="kukuri-shell-build" content="${version}"><script src="/app-${version}.js"></script><main>${version}</main>`);
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
    await report(first);
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

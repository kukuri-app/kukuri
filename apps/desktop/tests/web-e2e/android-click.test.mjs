import assert from 'node:assert/strict';
import test from 'node:test';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { attach } from 'webdriverio';
import { androidClick } from './android-click.mjs';

test('要素の差し替え後に位置を取り直し、成功した押下位置を返す', async () => {
  let refreshes = 0;
  const element = { waitForExist: async () => { refreshes++; } };
  const offset = { x: 30, y: 0 };
  const browser = {
    execute: async () => {
      if (refreshes === 1) throw Object.assign(new Error('stale element reference'), { name: 'stale element reference' });
      return { offset };
    },
    waitUntil: async (check) => {
      for (let attempt = 0; attempt < 3; attempt++) {
        const result = await check();
        if (result) return result;
      }
      throw new Error('timed out');
    },
  };
  assert.deepEqual(await androidClick(browser, element, (value) => value, 1000), offset);
  assert.equal(refreshes, 2);
});

test('stale以外のdriverの失敗を隠さない', async (context) => {
  // WebDriver session の初期照会だけ応答し、実際の waitUntil を使う。
  const server = createServer((_request, response) => {
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify({ value: 'unit-window' }));
  }).listen(0, '127.0.0.1');
  context.after(() => server.close());
  await once(server, 'listening');
  const browser = await attach({
    sessionId: 'android-click-unit', isW3C: true, capabilities: { browserName: 'chrome' },
    options: { hostname: '127.0.0.1', port: server.address().port, logLevel: 'silent', waitforInterval: 5 },
  });
  const error = new Error('session disconnected');
  const element = { waitForExist: async () => {} };
  let attempts = 0;
  browser.overwriteCommand('execute', async () => {
    if (++attempts === 1) throw error;
    return { offset: { x: 0, y: 0 } };
  });
  await assert.rejects(androidClick(browser, element, () => assert.fail('押下しない'), 1000), error);
});

test('中心が見えている場合も測定した位置でpointer actionsを使う', async () => {
  const offset = { x: 0, y: 0 };
  const element = { waitForExist: async () => {} };
  const browser = { execute: async () => ({ offset }), waitUntil: (check) => check() };
  assert.deepEqual(await androidClick(browser, element, (options) => options, 1000), offset);
});

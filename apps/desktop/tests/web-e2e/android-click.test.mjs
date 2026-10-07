import assert from 'node:assert/strict';
import test from 'node:test';
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

test('stale以外のdriverの失敗を隠さない', async () => {
  const error = new Error('session disconnected');
  const element = { waitForExist: async () => {} };
  const browser = { execute: async () => { throw error; }, waitUntil: (check) => check() };
  await assert.rejects(androidClick(browser, element, () => assert.fail('押下しない'), 1000), error);
});

test('中心が見えている場合も測定した位置でpointer actionsを使う', async () => {
  const offset = { x: 0, y: 0 };
  const element = { waitForExist: async () => {} };
  const browser = { execute: async () => ({ offset }), waitUntil: (check) => check() };
  assert.deepEqual(await androidClick(browser, element, (options) => options, 1000), offset);
});

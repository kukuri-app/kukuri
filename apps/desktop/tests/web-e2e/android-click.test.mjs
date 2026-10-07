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
      return offset;
    },
  };
  assert.deepEqual(await androidClick(browser, element, (value) => value, 1000), offset);
  assert.equal(refreshes, 2);
});

for (const failureAt of ['waitForExist', 'execute']) {
  test(`${failureAt}のstale以外のdriverの失敗を隠さない`, async () => {
    const error = new Error('session disconnected');
    let attempts = 0;
    const failOnce = async () => {
      if (++attempts === 1) throw error;
      return { x: 0, y: 0 };
    };
    const element = { waitForExist: failureAt === 'waitForExist' ? failOnce : async () => {} };
    const browser = { execute: failureAt === 'execute' ? failOnce : async () => ({ x: 0, y: 0 }) };
    await assert.rejects(androidClick(browser, element, () => assert.fail('押下しない'), 1000), error);
  });
}

test('中心が見えている場合も測定した位置でpointer actionsを使う', async () => {
  const offset = { x: 0, y: 0 };
  const element = { waitForExist: async () => {} };
  const browser = { execute: async () => offset };
  assert.deepEqual(await androidClick(browser, element, (options) => options, 1000), offset);
});

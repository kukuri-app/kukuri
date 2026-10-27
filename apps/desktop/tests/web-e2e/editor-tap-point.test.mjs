import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';
import { editorTapPoint } from './pwa-input-flow.mjs';

afterEach(() => {
  delete globalThis.devicePixelRatio;
  delete globalThis.visualViewport;
});

for (const [offsetLeft, offsetTop] of [[0, 0], [20, 500]]) {
  test(`the native tap stays inside the visible editor after viewport pan ${offsetLeft}/${offsetTop}`, () => {
    globalThis.devicePixelRatio = 2.625;
    globalThis.visualViewport = { offsetLeft, offsetTop };
    const element = { getBoundingClientRect: () => ({ left: 16 + offsetLeft, top: 100 + offsetTop, width: 280, height: 120 }) };
    assert.deepEqual(editorTapPoint(element), { x: 409.5, y: 526 });
  });
}

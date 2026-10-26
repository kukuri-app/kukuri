import assert from 'node:assert/strict';
import vm from 'node:vm';
import { test } from 'node:test';
import { pwaWorkerSource } from '../../scripts/webPwaShell.mjs';
import { waitForUnusedShellCache } from './pwa-shell-cache.mjs';

for (const reclaim of [true, false]) test(`shell cache wait ${reclaim ? 'collects after the last client exits' : 'rejects when collection leaves the old cache'}`, { timeout: 10_000 }, async () => {
  const data = new Map([
    ['kukuri-shell-one', new Response('old', { headers: { 'x-kukuri-shell-clients': '["old-client"]' } })],
    ['kukuri-shell-two', new Response('new')],
  ]);
  const caches = {
    async keys() { return [...data.keys()]; },
    async match(key, { cacheName }) { return data.get(cacheName)?.clone(); },
    async open(name) { return { async put(key, response) { data.set(name, response.clone()); } }; },
    async delete(name) { if (reclaim) return data.delete(name); return false; },
  };
  let snapshotReady;
  const snapshotTaken = new Promise(resolve => { snapshotReady = resolve; });
  let resume;
  const closed = new Promise(resolve => { resume = resolve; });
  let live = [{ id: 'old-client' }, { id: 'new-client' }];
  let first = true;
  const handlers = {};
  const worker = {
    postMessage(data, ports) {
      handlers.message({ data, ports, source: { id: 'new-client' }, waitUntil(pending) { worker.pending = pending; } });
    },
  };
  const context = vm.createContext({
    caches, Headers, Response, MessageChannel, setTimeout, clearTimeout,
    clients: { async matchAll() {
      const snapshot = live;
      if (first) { first = false; snapshotReady(); await closed; }
      return snapshot;
    } },
    registration: {},
    navigator: { serviceWorker: { ready: Promise.resolve({ active: worker }) } },
    document: { querySelector() { return { content: 'two' }; } },
    addEventListener(type, handler) { handlers[type] = handler; },
  });
  vm.runInContext(await pwaWorkerSource('two', ['/']), context);
  // 最後の起動時helloが旧clientを見た後、回収が完了する前に旧clientを終了する。
  worker.postMessage({ type: 'hello', version: 'two' }, []);
  await snapshotTaken;
  live = [{ id: 'new-client' }];
  resume();
  await worker.pending;
  assert.deepEqual([...data.keys()], ['kukuri-shell-one', 'kukuri-shell-two']);
  const browser = { execute(callback) { return vm.runInContext(`(${callback.toString()})()`, context); } };
  if (reclaim) {
    await waitForUnusedShellCache(browser, 1000);
    assert.deepEqual([...data.keys()], ['kukuri-shell-two']);
  } else {
    await assert.rejects(waitForUnusedShellCache(browser, 1000), /timed out: unused prior shell cache is reclaimed/);
    assert.deepEqual([...data.keys()], ['kukuri-shell-one', 'kukuri-shell-two']);
  }
});

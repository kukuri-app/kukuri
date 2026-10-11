import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import { test } from 'node:test';

test('a concurrent asset lookup cannot recreate a reclaimed shell cache', { timeout: 10000 }, async () => {
  const source = readFileSync(new URL('../../scripts/pwa-service-worker.mjs', import.meta.url), 'utf8').replace('/* KUKURI_PWA_BUILD */ null',
    JSON.stringify({ version: 'two', assets: ['/', '/new.js'] }));
  const data = new Map([
    ['kukuri-shell-one', new Map([['/', new Response('old')]])],
    ['kukuri-shell-two', new Map([['/', new Response('new')], ['https://shell.test/new.js', new Response('asset')]])],
  ]);
  let resumeFetch;
  const blocked = new Promise(resolve => { resumeFetch = resolve; });
  let reportSnapshot;
  const snapshotReady = new Promise(resolve => { reportSnapshot = resolve; });
  let pauseFirstKeys = true;
  const requestKey = key => typeof key === 'string' ? key : key.url;
  const caches = {
    async keys() {
      const snapshot = [...data.keys()];
      if (pauseFirstKeys) { pauseFirstKeys = false; reportSnapshot(); await blocked; }
      return snapshot;
    },
    async open(name) {
      if (!data.has(name)) data.set(name, new Map());
      const values = data.get(name);
      return {
        async match(key) { return values.get(requestKey(key))?.clone(); },
        async put(key, value) { values.set(requestKey(key), value.clone()); },
      };
    },
    async match(key, { cacheName }) { return data.get(cacheName)?.get(requestKey(key))?.clone(); },
    async delete(name) { return data.delete(name); },
  };
  const handlers = {};
  vm.runInNewContext(source, {
    caches, clients: { async matchAll() { return [{ id: 'new-client' }]; } }, registration: {},
    Headers, Response, URL, location: { origin: 'https://shell.test' },
    addEventListener(type, fn) { handlers[type] = fn; },
    async fetch() { return new Response('network'); },
  });
  let fetched;
  handlers.fetch({
    request: { method: 'GET', mode: 'cors', url: 'https://shell.test/new.js' },
    respondWith(value) { fetched = value; },
  });
  await snapshotReady;
  async function collect() {
    let finished;
    handlers.message({
      data: { type: 'hello', version: 'two' }, source: { id: 'new-client' }, ports: [],
      waitUntil(value) { finished = value; },
    });
    await finished;
  }
  await collect();
  assert.equal(data.has('kukuri-shell-one'), false, 'GC initially deletes the unused old build');
  resumeFetch();
  assert.equal(await (await fetched).text(), 'asset');
  await collect();
  assert.deepEqual([...data.keys()], ['kukuri-shell-two'], 'the stale fetch snapshot must not leave an old cache');
});

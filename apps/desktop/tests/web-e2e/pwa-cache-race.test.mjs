import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import { test } from 'node:test';

for (const operation of ['asset lookup', 'collection']) test(`a concurrent ${operation} cannot recreate a reclaimed shell cache`, { timeout: 10000 }, async () => {
  const source = readFileSync(new URL('../../scripts/pwa-service-worker.mjs', import.meta.url), 'utf8').replace('/* KUKURI_PWA_BUILD */ null',
    JSON.stringify({ version: 'two', assets: ['/', '/new.js'] }));
  const data = new Map([
    ['kukuri-shell-one', new Map([['/', new Response('old')]])],
    ['kukuri-shell-two', new Map([['/', new Response('new')], ['https://shell.test/new.js', new Response('asset')]])],
  ]);
  let resumeFetch;
  let blocked = new Promise(resolve => { resumeFetch = resolve; });
  let reportSnapshot;
  let snapshotReady = new Promise(resolve => { reportSnapshot = resolve; });
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
  const createWorker = () => {
    const handlers = {};
    vm.runInNewContext(source, {
    caches, clients: { async matchAll() { return [{ id: 'new-client' }]; } }, registration,
    Headers, Response, URL, location: { origin: 'https://shell.test' },
    addEventListener(type, fn) { handlers[type] = fn; },
    async fetch() { return new Response('network'); },
    });
    return handlers;
  };
  const registration = {};
  const handlers = createWorker();
  const other = createWorker();
  const collect = async (worker = handlers) => {
    let finished;
    worker.message({
      data: { type: 'hello', version: 'two' }, source: { id: 'new-client' }, ports: [],
      waitUntil(value) { finished = value; },
    });
    await finished;
  };
  let fetched;
  if (operation === 'asset lookup') handlers.fetch({
    request: { method: 'GET', mode: 'cors', url: 'https://shell.test/new.js' },
    respondWith(value) { fetched = value; },
  });
  else fetched = collect();
  await snapshotReady;
  await collect(other);
  assert.equal(data.has('kukuri-shell-one'), false, 'GC initially deletes the unused old build');
  resumeFetch();
  if (operation === 'asset lookup') assert.equal(await (await fetched).text(), 'asset');
  else await fetched;
  await collect();
  assert.deepEqual([...data.keys()], ['kukuri-shell-two'], 'a stale cache snapshot must not leave an old cache');
  // 回収の await 中に始まった install の、まだ HTML が無い cache を消さない。
  data.set('kukuri-shell-three', new Map());
  pauseFirstKeys = true;
  blocked = new Promise(resolve => { resumeFetch = resolve; });
  snapshotReady = new Promise(resolve => { reportSnapshot = resolve; });
  const preparing = collect();
  await snapshotReady;
  registration.installing = {};
  resumeFetch();
  await preparing;
  assert.equal(data.has('kukuri-shell-three'), true, 'an installation that started during collection stays intact');
});

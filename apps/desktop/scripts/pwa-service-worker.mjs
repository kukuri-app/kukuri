// buildが固定資源の一覧を埋める。本人データ・API・背景runtimeは扱わない。
const BUILD = /* KUKURI_PWA_BUILD */ null;
const { caches, clients, registration } = globalThis;
const PREFIX = 'kukuri-shell-';
const CACHE = PREFIX + BUILD.version;
const OWNERS = 'x-kukuri-shell-clients';
let work = Promise.resolve();
const enqueue = job => (work = work.catch(() => undefined).then(job));

async function updateOwners(cache, response, ids) {
  const headers = new Headers(response.headers);
  headers.set(OWNERS, JSON.stringify(ids));
  await cache.put('/', new Response(response.body, { status: response.status, headers }));
}

// 凍結中のtabも守るため、固定HTMLの内部headerにbuildの使用者を記録する。
// 各clientは1buildだけを所有し、close済みのIDはこの有限な集合から外す。
async function collect(owner, version) {
  let preserve = Boolean(registration.installing);
  const live = new Set((await clients.matchAll({ includeUncontrolled: true })).map(client => client.id));
  const pending = registration.waiting;
  let waiting;
  if (pending) {
    const channel = new MessageChannel();
    waiting = await new Promise(resolve => {
      const timeout = setTimeout(() => { channel.port1.close(); resolve(null); }, 3000);
      channel.port1.onmessage = event => { clearTimeout(timeout); channel.port1.close(); resolve(event.data); };
      pending.postMessage({ type: 'version' }, [channel.port2]);
    });
    if (waiting === null) preserve = true;
  }
  for (const name of (await caches.keys()).filter(name => name.startsWith(PREFIX))) {
    const html = await caches.match('/', { cacheName: name });
    if (!html) {
      if (!preserve && !registration.installing && registration.waiting === pending && name !== CACHE && name !== PREFIX + waiting) await caches.delete(name);
      continue;
    }
    const ids = JSON.parse(html.headers.get(OWNERS) || '[]').filter(id => live.has(id) && id !== owner);
    if (owner && name === PREFIX + version) ids.push(owner);
    if (preserve || ids.length || name === CACHE || name === PREFIX + waiting) await updateOwners(await caches.open(name), html, ids);
    else if (!registration.installing && registration.waiting === pending) await caches.delete(name);
  }
}

globalThis.addEventListener('install', event => {
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    if (await cache.match('/')) return;
    // HTTP cacheを通すと、同じ資源を並行に読むpage側と1 entryを取り合って片方が失敗する（#1901）。
    try { await cache.addAll(BUILD.assets.map(asset => new Request(asset, { cache: 'no-store' }))); }
    catch (error) { await caches.delete(CACHE); throw error; }
  })());
});
globalThis.addEventListener('activate', event => {
  event.waitUntil(enqueue(async () => { await clients.claim(); await collect(); }));
});
globalThis.addEventListener('message', event => {
  if (event.data?.type === 'version') event.ports[0]?.postMessage(BUILD.version);
  if (event.data?.type === 'apply') event.waitUntil(globalThis.skipWaiting());
  if (event.data?.type === 'hello' && event.source?.id) {
    event.waitUntil(enqueue(async () => {
      await collect(event.source.id, event.data.version);
      event.ports[0]?.postMessage('retained');
    }));
  }
});
globalThis.addEventListener('fetch', event => {
  const url = new URL(event.request.url);
  if (event.request.method !== 'GET' || url.origin !== globalThis.location.origin) return;
  event.respondWith((async () => {
    if (event.request.mode === 'navigate' && url.pathname === '/') {
      const html = await caches.match('/', { cacheName: CACHE });
      if (html) return html;
    }
    // 旧tabの遅延assetも、使用中の固定buildのcacheから読む。
    if (!url.search) {
      for (const name of (await caches.keys()).filter(name => name.startsWith(PREFIX))) {
        const hit = await caches.match(event.request, { cacheName: name });
        if (hit) return hit;
      }
    }
    return fetch(event.request);
  })());
});

// 2 つの instance が同じ PC で同時に動き、同意して直接つながり、画面と Rust の間で raw の bytes を受け渡せる(#1800 AC-2)。
import assert from 'node:assert/strict';

import { accept, connect, openTopic, waitForPeers } from '../session.mjs';

export async function run({ launch, log, topic }) {
  const instances = await launch('a', 'b');
  await Promise.all(instances.map(accept));
  log('both instances accepted the consent and are ready');

  await connect(instances);
  await Promise.all(instances.map((instance) => openTopic(instance, topic, 'timeline')));
  for (const entry of await Promise.all(instances.map((instance) => waitForPeers(instance, topic, 1)))) {
    log(`${entry.peer_count} peer(s) on the topic, path=${entry.active_path}`);
    assert.equal(entry.active_path, 'direct_p2p');
  }

  // raw の応答は ArrayBuffer で届く(CSP が IPC の custom protocol を止めると数の配列になる。#1799)。
  const kind = await instances[1].evaluate(
    `window.__TAURI_INTERNALS__.invoke('next_live_video', { request: { topic: ${JSON.stringify(topic)}, session_id: 'none' } })` +
      `.then((value) => value instanceof ArrayBuffer ? 'ArrayBuffer' : Object.prototype.toString.call(value))`
  );
  assert.equal(kind, 'ArrayBuffer');
  log('raw responses arrive as ArrayBuffer');
}

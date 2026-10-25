// live の転送(#1034 の runbook の手順 10、#1800 AC-4 の LV-10)。配信者 H の直接の視聴者 4 人(V1〜V4)の後の 5 人目(V5)に、
// 視聴者からの転送で描画と音が届く。転送している視聴者を止めると、V5 の描画は 10 秒ほどで戻る(配信者か別の視聴者から
// 受け直す)。転送された映像と音のずれは記録だけにする(6 つの app が CI の CPU を取り合い、測る値が揺れる。ずれの判定は
// LV-3。2026-10-14 のユーザー判断)。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { eventually } from '../../web-e2e/wait.mjs';
import { audioMode, isolated, record, speak } from '../audio.mjs';
import {
  CARD,
  createSession,
  drawsSince,
  injectCapture,
  measureSync,
  observeDraws,
  selectLive,
  startBroadcast,
  VOICE,
  watch,
} from '../live-media.mjs';
import { accept, connect, openTopic, waitForPeers } from '../session.mjs';
import { click, text } from '../ui.mjs';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const drawing = async (viewer) => (await drawsSince(viewer, Date.now() - 1_000)).length > 0;
/// `viewer` が作った media の接続の数(親を失って受け直すと増える)。
const connections = (viewer) => (readFileSync(viewer.log, 'utf8').match(/media connection established/g) ?? []).length;
/// 退出の後に V5 を見る間。
const AFTER_LEAVE_MS = 10_000;

/// `since` から今までの描画の間隔の最長(ms)。
function longestGap(draws, since) {
  let longest = 0;
  let previous = since;
  for (const { at } of draws) {
    longest = Math.max(longest, at - previous);
    previous = at;
  }
  return Math.max(longest, Date.now() - previous);
}

export async function run({ launch, log, topic }) {
  const [host, ...viewers] = await launch('host', 'first', 'second', 'third', 'fourth', 'fifth');
  const all = [host, ...viewers];
  await Promise.all(all.map(accept));
  await connect(all);
  await Promise.all(all.map((instance) => openTopic(instance, topic, 'timeline')));
  await Promise.all(all.map((instance) => waitForPeers(instance, topic, viewers.length)));
  await injectCapture(host);
  await Promise.all(viewers.map(observeDraws));
  await createSession(host, 'live-relay');
  await Promise.all(viewers.map(selectLive));
  await startBroadcast(host);
  const voice = audioMode ? speak('host', VOICE) : () => undefined;
  const direct = viewers.slice(0, 4);
  const fifth = viewers[4];
  const heard = audioMode ? record(fifth) : null;
  try {
    for (const viewer of direct) await watch(viewer);
    await watch(fifth);
    let level = null;
    if (heard) {
      await eventually('the fifth viewer to hear the host', () => heard.level(2_000) > 1e-3, 30_000);
      level = heard.level(2_000);
    }
    let sync = 'not measured';
    if (isolated) {
      voice();
      sync = `${(await measureSync(host, fifth, heard)).map(Math.round).join(', ')} ms`;
    }
    log(`LV-10: the fifth viewer draws and hears (${level?.toFixed(4)}) through a relay; the video is behind the sound by ${sync}`);

    // 転送している視聴者(V5 の親)を探す: 直接の視聴者を 1 人ずつ退出させ、退出の後に V5 が media の接続を作り直した
    // (受け直した)人が親。受け直しが速いと描画はほとんど途切れないので、途切れでは見分けない。
    let relaying = null;
    for (const viewer of direct) {
      const before = connections(fifth);
      const leftAt = Date.now();
      await click(viewer, text('common', 'actions.leave'), { within: CARD });
      await sleep(AFTER_LEAVE_MS);
      const reconnected = connections(fifth) - before;
      const gap = longestGap(await drawsSince(fifth, leftAt), leftAt);
      log(`LV-10: after ${viewer.name} left, the fifth viewer connected ${reconnected} time(s) and its drawing paused for at most ${Math.round(gap)} ms`);
      if (reconnected === 0) continue;
      await eventually('the fifth viewer to draw again', () => drawing(fifth), 30_000);
      relaying = viewer;
      break;
    }
    assert.ok(relaying, 'one of the direct viewers relays to the fifth viewer');
    log(`LV-10: the fifth viewer draws again after ${relaying.name}, who relayed to it, left`);
  } finally {
    voice();
    heard?.stop();
  }
}

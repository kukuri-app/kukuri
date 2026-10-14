// live の転送(#1034 の runbook の手順 10、#1800 AC-4 の LV-10)。配信者 H の直接の視聴者 4 人(V1〜V4)の後の 5 人目(V5)に、
// 視聴者からの転送で描画と音が届き、LV-3 と同じ範囲に入る。転送している視聴者を止めると、V5 の描画は 10 秒ほどで戻る
// (配信者か別の視聴者から受け直す)。
import assert from 'node:assert/strict';

import { eventually } from '../../web-e2e/wait.mjs';
import { audioMode, isolated, record, speak } from '../audio.mjs';
import {
  CARD,
  createSession,
  drawsSince,
  injectCapture,
  inSync,
  measureSync,
  observeDraws,
  t,
  VOICE,
  watch,
} from '../live-media.mjs';
import { accept, connect, openTopic, waitForPeers } from '../session.mjs';
import { click, text } from '../ui.mjs';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const drawing = async (viewer) => (await drawsSince(viewer, Date.now() - 1_000)).length > 0;

export async function run({ launch, log, topic }) {
  const [host, ...viewers] = await launch('host', 'first', 'second', 'third', 'fourth', 'fifth');
  const all = [host, ...viewers];
  await Promise.all(all.map(accept));
  await connect(all);
  await Promise.all(all.map((instance) => openTopic(instance, topic, 'live')));
  await Promise.all(all.map((instance) => waitForPeers(instance, topic, viewers.length)));
  await injectCapture(host);
  await Promise.all(viewers.map(observeDraws));
  await createSession(host, 'live-relay');
  await click(host, t('media.startBroadcast'), { within: CARD });
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
      const offsets = await measureSync(host, fifth, heard);
      sync = `${offsets.map(Math.round).join(', ')} ms`;
      assert.ok(inSync(offsets), `the relayed video is in sync with the sound (${sync})`);
    }
    log(`LV-10: the fifth viewer draws and hears (${level?.toFixed(4)}) through a relay; the video is behind the sound by ${sync}`);

    // 転送している視聴者(V5 の親)を探す: 直接の視聴者を 1 人ずつ退出させ、V5 の描画が止まった人が親。
    let recovered = null;
    for (const viewer of direct) {
      await click(viewer, text('common', 'actions.leave'), { within: CARD });
      const leftAt = Date.now();
      await sleep(4_000);
      if (await drawing(fifth)) continue;
      await eventually('the fifth viewer to draw again', () => drawing(fifth), 30_000);
      recovered = Date.now() - leftAt;
      break;
    }
    assert.ok(recovered !== null, 'one of the direct viewers relays to the fifth viewer');
    log(`LV-10: the fifth viewer draws again ${recovered} ms after the relaying viewer left`);
  } finally {
    voice();
    heard?.stop();
  }
}

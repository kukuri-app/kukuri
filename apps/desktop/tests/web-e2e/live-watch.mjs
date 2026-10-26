import assert from 'node:assert/strict';
import { eventually } from './wait.mjs';

export async function watchWithoutMicrophone(watch, timeout) {
  await watch({ restart: true, ms: 0 });
  let starting;
  // 起動の応答は最初の映像の到達を保証しない。同じ視聴の到達を待ってから無音の観測窓を始める。
  await eventually('native receives the video after restarting', async () => {
    starting = await watch();
    assert.equal(starting.state, 0, `native viewing ended: ${JSON.stringify(starting)}`);
    return starting.chunks > 0;
  }, timeout).catch(error => assert.fail(`${error.message}: ${JSON.stringify(starting)}`));
  const muted = await watch({ ms: 4_000 });
  assert.ok(muted.chunks > 0, `native still receives the video: ${JSON.stringify(muted)}`);
  assert.ok(
    muted.audio_clock_us === 0 || muted.video_us - muted.audio_clock_us > 2_000_000,
    `native receives no sound while the microphone is off: ${JSON.stringify(muted)}`
  );
  return muted;
}

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { watchWithoutMicrophone } from './live-watch.mjs';

const empty = { state: 0, chunks: 0, keyframes: 0, video_us: 0, audio_clock_us: 0 };
const video = { ...empty, chunks: 30, keyframes: 1, video_us: 10_000_000 };

test('再視聴の開始中に映像が未到達でも、到達後に4秒の無音を確認する', async () => {
  const calls = [];
  let reads = 0;
  const muted = await watchWithoutMicrophone(async (options) => {
    calls.push(options);
    // 購読を開始しても最初の観測窓が空で終わる順序を固定する。
    if (options?.restart || reads++ === 0) return empty;
    return video;
  }, 1000);
  assert.deepEqual(muted, video);
  assert.deepEqual(calls, [{ restart: true, ms: 0 }, undefined, undefined, { ms: 4_000 }]);
});

test('映像が届かなければ、音声が0でも成功しない', async () => {
  await assert.rejects(watchWithoutMicrophone(async () => empty, 0), /native.*video/);
});

test('到達後の観測窓で映像が止まれば失敗する', async () => {
  await assert.rejects(watchWithoutMicrophone(async (options) =>
    options?.restart || options?.ms === 4_000 ? empty : video
  ), /native still receives the video/);
});

test('micを止めても音声が映像に追いついていれば失敗する', async () => {
  await assert.rejects(watchWithoutMicrophone(async (options) =>
    options?.restart ? empty : { ...video, audio_clock_us: video.video_us }
  ), /native receives no sound/);
});

import { describe, expect, test } from 'vitest';

import { captureErrorKind, fitVideoSize, frameToShow, parseLiveVideoBatch } from './liveMedia';

/// Rust の `encode_screen_batch` と同じ形の応答を作る。
function batch(state: number, clockUs: number, chunks: Array<[number, boolean, number[]]>) {
  const buffer = new ArrayBuffer(11 + chunks.reduce((sum, [, , data]) => sum + 13 + data.length, 0));
  const view = new DataView(buffer);
  view.setUint8(0, state);
  view.setBigUint64(1, BigInt(clockUs), true);
  view.setUint16(9, chunks.length, true);
  let at = 11;
  for (const [timestamp, keyframe, data] of chunks) {
    view.setBigUint64(at, BigInt(timestamp), true);
    view.setUint8(at + 8, keyframe ? 1 : 0);
    view.setUint32(at + 9, data.length, true);
    new Uint8Array(buffer, at + 13, data.length).set(data);
    at += 13 + data.length;
  }
  return buffer;
}

describe('live media', () => {
  test('reads the video and the audio clock the runtime hands to the screen', () => {
    const parsed = parseLiveVideoBatch(batch(0, 10_020_000, [[10_000_000, true, [1, 2]], [10_033_000, false, [3]]]));
    expect(parsed.state).toBe('watching');
    expect(parsed.audioClockUs).toBe(10_020_000);
    expect(parsed.chunks.map(({ timestampUs, keyframe, data }) => [timestampUs, keyframe, [...data]])).toEqual([
      [10_000_000, true, [1, 2]],
      [10_033_000, false, [3]],
    ]);
    expect(parseLiveVideoBatch(batch(1, 0, [])).state).toBe('full');
    expect(parseLiveVideoBatch(batch(2, 0, [])).state).toBe('no_media');
    expect(parseLiveVideoBatch(batch(3, 0, [])).state).toBe('ended');
  });

  // ADR 0067 の live_media_presents_video_against_audio_clock(画面側): 映像は音声の再生時刻から 125 ms 先までの最新を出す。
  test('presents the newest frame within the lead of the audio clock', () => {
    const frames = [1_000_000, 1_033_000, 1_066_000, 1_200_000];
    expect(frameToShow(frames, 1_000_000)).toBe(2);
    expect(frameToShow(frames, 1_080_000)).toBe(3);
    // 音声より 125 ms 以上先の映像しか無ければ待つ。
    expect(frameToShow(frames.slice(3), 1_000_000)).toBe(-1);
    // 音声の無い配信は、届いた最新を出す。
    expect(frameToShow(frames, 0)).toBe(3);
    expect(frameToShow([], 1_000_000)).toBe(-1);
  });

  test('fits the captured size within the quality limits', () => {
    const camera = { width: 1280, height: 720, framerate: 30, bitrate: 1_500_000 };
    expect(fitVideoSize(1280, 720, camera)).toEqual({ width: 1280, height: 720 });
    expect(fitVideoSize(640, 481, camera)).toEqual({ width: 640, height: 480 });
    expect(fitVideoSize(2560, 1600, { ...camera, width: 1920, height: 1080 })).toEqual({ width: 1728, height: 1080 });
  });

  test('classifies capture failures', () => {
    const error = (name: string) => Object.assign(new Error(name), { name });
    expect(captureErrorKind(error('NotAllowedError'), 'screen')).toBe('denied');
    expect(captureErrorKind(error('NotFoundError'), 'camera')).toBe('cameraUnavailable');
    expect(captureErrorKind(error('NotFoundError'), 'screen')).toBe('failed');
    expect(captureErrorKind(error('NotSupportedError'), 'camera')).toBe('unsupported');
    expect(captureErrorKind(Object.assign(error('InvokeError'), { code: 'LIVE_BROADCAST_ELSEWHERE' }), 'camera')).toBe(
      'elsewhere'
    );
  });
});

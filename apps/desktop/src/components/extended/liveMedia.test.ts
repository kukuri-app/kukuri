import { describe, expect, test, vi } from 'vitest';

import tauriConfig from '../../../src-tauri/tauri.conf.json';
import { captureErrorKind, captureVideo, createLivePlayer, encodeVideo, fitVideoSize, frameToShow, parseLiveVideoBatch } from './liveMedia';

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
  test('captures the selected camera by deviceId', async () => {
    const getUserMedia = vi.fn(async () => ({} as MediaStream));
    vi.stubGlobal('VideoEncoder', class {});
    vi.stubGlobal('navigator', { mediaDevices: { getUserMedia } });
    try {
      await captureVideo('camera', { width: 1280, height: 720, framerate: 30, bitrate: 1_500_000 }, 'rear');
      expect(getUserMedia).toHaveBeenCalledWith({ audio: false, video: {
        width: { max: 1280 }, height: { max: 720 }, frameRate: { max: 30 }, deviceId: { exact: 'rear' },
      } });
    } finally { vi.unstubAllGlobals(); }
  });
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

  // 映像の chunk は raw の body と応答で受け渡す(ADR 0067 §9)。CSP が IPC の custom protocol を許さないと、Tauri は
  // postMessage の経路へ落ち、raw の body と応答を JSON の数の配列に変える(push は断られ、応答は読めない)。
  test('lets invoke use the IPC custom protocol that carries raw video bytes', () => {
    const connectSources = tauriConfig.app.security.csp['connect-src'].split(/\s+/);
    expect(connectSources).toEqual(expect.arrayContaining(['ipc:', 'http://ipc.localhost']));
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

  // #1809: 配信を始め直すと frame の時刻が前へ戻る。WebKitGTK の decoder は、前に出した frame より早い時刻の frame を出さない。
  test('draws a restarted broadcast whose timestamps go back', () => {
    class Decoder {
      state = 'unconfigured';
      private latest = -Infinity;
      constructor(private readonly init: { output: (frame: unknown) => void }) {}
      configure() {
        this.state = 'configured';
      }
      decode(chunk: { timestamp: number }) {
        if (chunk.timestamp < this.latest) return;
        this.latest = chunk.timestamp;
        this.init.output({ timestamp: chunk.timestamp, displayWidth: 2, displayHeight: 2, close() {} });
      }
      reset() {
        this.latest = -Infinity;
      }
      close() {
        this.state = 'closed';
      }
    }
    let draw = () => {};
    vi.stubGlobal('VideoDecoder', Decoder);
    vi.stubGlobal('EncodedVideoChunk', class {
      constructor(init: object) {
        Object.assign(this, init);
      }
    });
    vi.stubGlobal('requestAnimationFrame', (callback: () => void) => ((draw = callback), 1));
    vi.stubGlobal('cancelAnimationFrame', () => {});
    const drawn: number[] = [];
    const canvas = {
      width: 0,
      height: 0,
      getContext: () => ({ drawImage: (frame: { timestamp: number }) => drawn.push(frame.timestamp) }),
    } as unknown as HTMLCanvasElement;
    try {
      const player = createLivePlayer(canvas);
      const keyframe = (timestampUs: number) => ({ timestampUs, keyframe: true, data: new Uint8Array([1]) });
      player.push({ state: 'watching', audioClockUs: 76_000_000, chunks: [keyframe(76_000_000)] });
      draw();
      player.push({ state: 'watching', audioClockUs: 15_000_000, chunks: [keyframe(15_000_000)] });
      draw();
      expect(drawn).toEqual([76_000_000, 15_000_000]);
    } finally {
      vi.unstubAllGlobals();
    }
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

  // #1834: WebKitGTK 2.50 は `<video>` から作った frame の encode に失敗するので、`<video>` の frame は canvas から作る。
  test('makes the frames of the preview video from a canvas', () => {
    const sources: unknown[] = [];
    vi.stubGlobal(
      'VideoEncoder',
      class {
        state = 'configured';
        encodeQueueSize = 0;
        configure() {}
        encode() {}
        close() {}
      }
    );
    vi.stubGlobal(
      'VideoFrame',
      class {
        constructor(source: unknown) {
          sources.push(source);
        }
        close() {}
      }
    );
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
      drawImage() {},
    } as unknown as CanvasRenderingContext2D);
    let frame = () => {};
    const track = { addEventListener: () => {}, stop: () => {} };
    const capture = encodeVideo({
      stream: { getVideoTracks: () => [track], getTracks: () => [track] } as unknown as MediaStream,
      limits: { width: 640, height: 360, framerate: 15, bitrate: 1_000_000 },
      preview: {
        videoWidth: 640,
        videoHeight: 360,
        play: () => Promise.resolve(),
        requestVideoFrameCallback: (callback: () => void) => ((frame = callback), 0),
      } as unknown as HTMLVideoElement,
      push: async () => {},
      onEnded: () => {},
    });
    frame();
    capture.stop();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    expect(sources).toHaveLength(1);
    expect(sources[0]).toBeInstanceOf(HTMLCanvasElement);
  });

  // ADR 0067 §8: 画面が変わらない間も 1 秒ごとに前の frame を送り直す。送り直しの直前に画面が変わって frame が出ても、
  // 次の frame まで 1 秒を超えてあけない。
  test('re-sends a still screen every second even right after a changed frame', () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'performance'] });
    const encoded: number[] = [];
    vi.stubGlobal(
      'VideoEncoder',
      class {
        state = 'configured';
        encodeQueueSize = 0;
        configure() {}
        encode() {
          encoded.push(performance.now());
        }
        close() {
          this.state = 'closed';
        }
      }
    );
    vi.stubGlobal('VideoFrame', class { close() {} });
    let changed = () => {};
    const track = { addEventListener: () => {}, stop: () => {} };
    const capture = encodeVideo({
      stream: { getVideoTracks: () => [track], getTracks: () => [track] } as unknown as MediaStream,
      limits: { width: 640, height: 360, framerate: 15, bitrate: 1_000_000 },
      preview: {
        videoWidth: 640,
        videoHeight: 360,
        play: () => Promise.resolve(),
        requestVideoFrameCallback: (callback: () => void) => {
          changed = callback;
          return 0;
        },
      } as unknown as HTMLVideoElement,
      push: async () => {},
      onEnded: () => {},
    });
    vi.advanceTimersByTime(800);
    changed();
    vi.advanceTimersByTime(3_000);
    capture.stop();
    vi.useRealTimers();
    vi.unstubAllGlobals();
    const gaps = encoded.slice(1).map((at, index) => at - encoded[index]);
    expect(Math.max(...gaps)).toBeLessThanOrEqual(1_000);
  });
});

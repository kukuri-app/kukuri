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

// 音声時計と描画tickを制御し、復号済みframeの保持と解放を確かめる。
function playerFixture() {
  const active = new Set<object>();
  class Decoder {
    state = 'unconfigured';
    private latest = -Infinity;
    constructor(private readonly init: { output: (frame: unknown) => void }) {}
    configure() { this.state = 'configured'; }
    decode(chunk: { timestamp: number }) {
      if (chunk.timestamp < this.latest) return;
      this.latest = chunk.timestamp;
      const frame = { timestamp: chunk.timestamp, displayWidth: 2, displayHeight: 2,
        close() { active.delete(this); } };
      active.add(frame);
      this.init.output(frame);
    }
    reset() { this.latest = -Infinity; }
    close() { this.state = 'closed'; }
  }
  let draw = () => {};
  let now = 0;
  const time = vi.spyOn(performance, 'now').mockImplementation(() => now);
  vi.stubGlobal('VideoDecoder', Decoder);
  vi.stubGlobal('EncodedVideoChunk', class {
    constructor(init: object) { Object.assign(this, init); }
  });
  vi.stubGlobal('requestAnimationFrame', (callback: () => void) => ((draw = callback), 1));
  vi.stubGlobal('cancelAnimationFrame', () => {});
  const drawn: number[] = [];
  const canvas = { width: 0, height: 0,
    getContext: () => ({ drawImage: (frame: { timestamp: number }) => drawn.push(frame.timestamp) }),
  } as unknown as HTMLCanvasElement;
  const player = createLivePlayer(canvas);
  return { player, drawn, draw: () => draw(), advance: (ms: number) => { now += ms; }, retained: () => active.size,
    close() { player.close(); vi.unstubAllGlobals(); time.mockRestore(); } };
}

const keyframe = (timestampUs: number) => ({ timestampUs, keyframe: true, data: new Uint8Array([1]) });

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

  // #1809: WebKitGTKは以前より早いtimestampを同じdecoderで描かない。
  test('draws a restarted broadcast whose timestamps go back', () => {
    const fixture = playerFixture();
    try {
      fixture.player.push({ state: 'watching', audioClockUs: 76_000_000, chunks: [keyframe(76_000_000)] });
      fixture.draw();
      fixture.player.push({ state: 'watching', audioClockUs: 15_000_000, chunks: [keyframe(15_000_000)] });
      fixture.draw();
      expect(fixture.drawn).toEqual([76_000_000, 15_000_000]);
    } finally { fixture.close(); }
  });

  test('keeps presenting against a delayed audio clock when decoded frames reach the limit', () => {
    const fixture = playerFixture();
    const periods = new Set<number>();
    try {
      // Windowsの6端末試験で観測した、映像が音声時計より約1.6秒先に到着する状態。
      for (let index = 0; index < 180; index += 1) {
        const audio = 10_000_000 + index * 33_333;
        const before = fixture.drawn.length;
        fixture.player.push({ state: 'watching', audioClockUs: audio, chunks: [keyframe(audio + 1_600_000)] });
        expect(fixture.retained()).toBeLessThanOrEqual(30);
        fixture.draw();
        if (fixture.drawn.length > before) {
          periods.add(Math.floor(index / 30));
          const shown = fixture.drawn.at(-1)!;
          expect(shown - audio).toBeGreaterThanOrEqual(-45_000);
          expect(shown - audio).toBeLessThanOrEqual(125_000);
        }
        fixture.advance(33.333);
      }
      expect(fixture.drawn.length).toBeGreaterThan(0);
      expect([...periods]).toEqual([1, 2, 3, 4, 5]);
    } finally {
      fixture.close();
      expect(fixture.retained()).toBe(0);
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
  test('takes worker frames without a canvas readback and releases the retained heartbeat frame on stop', () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'performance'] });
    const workers: Array<{ onmessage: ((event: { data: object }) => void) | null; postMessage: ReturnType<typeof vi.fn>; terminate: ReturnType<typeof vi.fn> }> = [];
    vi.stubGlobal('MediaStreamTrackProcessor', undefined);
    vi.stubGlobal('Worker', class {
      onmessage = null;
      postMessage = vi.fn();
      terminate = vi.fn();
      constructor() { workers.push(this); }
    });
    const sources: unknown[] = [];
    const encoded: number[] = [];
    class Frame {
      displayWidth = 640;
      displayHeight = 360;
      close = vi.fn();
      constructor(source?: unknown) { if (source) sources.push(source); }
    }
    vi.stubGlobal('VideoFrame', Frame);
    vi.stubGlobal('VideoEncoder', class {
      state = 'configured'; encodeQueueSize = 0;
      configure() {}
      encode() { encoded.push(performance.now()); }
      close() { this.state = 'closed'; }
    });
    const context = vi.spyOn(HTMLCanvasElement.prototype, 'getContext');
    const clone = { stop: vi.fn() };
    const track = { addEventListener: vi.fn(), stop: vi.fn(), clone: vi.fn(() => clone) };
    const capture = encodeVideo({
      stream: { getVideoTracks: () => [track], getTracks: () => [track] } as unknown as MediaStream,
      limits: { width: 640, height: 360, framerate: 15, bitrate: 1_000_000 },
      preview: { play: async () => {}, requestVideoFrameCallback: vi.fn(), videoWidth: 640, videoHeight: 360 } as unknown as HTMLVideoElement,
      push: async () => {}, onEnded: vi.fn(),
    });
    try {
      expect(workers).toHaveLength(1);
      const worker = workers[0];
      worker.onmessage!({ data: { supported: true } });
      expect(worker.postMessage).toHaveBeenCalledWith({ track: clone }, [clone]);
      const first = new Frame();
      worker.onmessage!({ data: { frame: first } });
      vi.advanceTimersByTime(1000);
      expect(encoded).toEqual([0, 1000]);
      expect(sources).toEqual([first, first]);
      expect(context).not.toHaveBeenCalled();
      expect(first.close).not.toHaveBeenCalled();
      const second = new Frame();
      vi.advanceTimersByTime(100);
      worker.onmessage!({ data: { frame: second } });
      expect(first.close).toHaveBeenCalledTimes(1);
      capture.stop();
      expect(second.close).toHaveBeenCalledTimes(1);
      expect(worker.terminate).toHaveBeenCalledTimes(1);
      expect(track.stop).toHaveBeenCalledTimes(1);
      const late = new Frame();
      worker.onmessage!({ data: { frame: late } });
      expect(late.close).toHaveBeenCalledTimes(1);
      expect(encoded).toHaveLength(3);
    } finally {
      capture.stop(); vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks();
    }
  });

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

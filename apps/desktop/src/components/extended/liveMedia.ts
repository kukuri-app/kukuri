import type { LiveVideoLimits, LiveVideoSource } from '@/lib/api';

// #1034: live の映像の取り込み・圧縮(配信者)と復号・描画(視聴者)。ADR 0067 §4・§9。映像の bytes は memory にだけ置く。

/// 視聴の状態(Rust の `LiveWatchState` の code の順)。
export const LIVE_WATCH_STATES = ['watching', 'full', 'no_media', 'ended'] as const;
export type LiveWatchState = (typeof LIVE_WATCH_STATES)[number];

export type LiveVideoChunk = { timestampUs: number; keyframe: boolean; data: Uint8Array };
export type LiveVideoBatch = { state: LiveWatchState; audioClockUs: number; chunks: LiveVideoChunk[] };

/// 映像を音声より先に出してよい上限(映像の進み 125 ms)。
const VIDEO_LEAD_US = 125_000;
/// keyframe の間隔(group の長さ)。
const KEYFRAME_INTERVAL_US = 2_000_000;
/// 画面が変わらない間も前の frame を送り直す間隔(視聴者は 5 秒届かなければ親を失ったとみなす。ADR 0067 §8)。
const HEARTBEAT_US = 1_000_000;
/// encoder の待ちの上限(frame)。超えた frame は送らない。
const ENCODER_QUEUE = 2;
/// Rust へ送りかけの chunk と、復号して描く前の frame の上限(1 秒分)。
const PENDING_CHUNKS = 30;
const DECODED_FRAMES = 30;
const DECODER_CONFIG: VideoDecoderConfig = { codec: 'vp8', optimizeForLatency: true };

/// `next_live_video` の応答(Rust の `encode_screen_batch`)を読む。数値は little endian。
export function parseLiveVideoBatch(buffer: ArrayBuffer): LiveVideoBatch {
  const view = new DataView(buffer);
  const chunks: LiveVideoChunk[] = [];
  let at = 11;
  for (let index = 0; index < view.getUint16(9, true); index += 1) {
    const length = view.getUint32(at + 9, true);
    chunks.push({
      timestampUs: Number(view.getBigUint64(at, true)),
      keyframe: view.getUint8(at + 8) === 1,
      data: new Uint8Array(buffer, at + 13, length),
    });
    at += 13 + length;
  }
  return {
    state: LIVE_WATCH_STATES[view.getUint8(0)] ?? 'ended',
    audioClockUs: Number(view.getBigUint64(1, true)),
    chunks,
  };
}

/// 描く frame の位置: 音声の再生時刻に進みの上限を足した時刻までで最も新しい frame(それより前は捨てる)。音声が無ければ最も
/// 新しい frame。描けるものが無ければ -1。
export function frameToShow(timestamps: readonly number[], clockUs: number): number {
  let shown = timestamps.length - 1;
  while (clockUs > 0 && shown >= 0 && timestamps[shown] > clockUs + VIDEO_LEAD_US) shown -= 1;
  return shown;
}

/// 取り込んだ大きさを、縦横比を保って上限の中へ縮める(偶数に丸める)。
export function fitVideoSize(width: number, height: number, limits: LiveVideoLimits) {
  const scale = Math.min(1, limits.width / width, limits.height / height);
  const even = (value: number) => Math.max(2, Math.floor((value * scale) / 2) * 2);
  return { width: even(width), height: even(height) };
}

export type LiveCaptureError = 'denied' | 'cameraUnavailable' | 'unsupported' | 'elsewhere' | 'failed';

/// 配信の開始の失敗の理由。Ubuntu 22.04 の webview は camera を列挙できない(ADR 0067 §9)。この端末で作っていない
/// session は runtime が code で断る。
export function captureErrorKind(error: unknown, source: LiveVideoSource): LiveCaptureError {
  if ((error as { code?: unknown } | null)?.code === 'LIVE_BROADCAST_ELSEWHERE') return 'elsewhere';
  const name = error instanceof Error ? error.name : '';
  if (name === 'NotAllowedError') return 'denied';
  if (name === 'NotSupportedError') return 'unsupported';
  if (source === 'camera' && ['NotFoundError', 'NotReadableError', 'OverconstrainedError'].includes(name)) {
    return 'cameraUnavailable';
  }
  return 'failed';
}

/// camera か画面を、上限の大きさ・frame 数で取り込む。
export async function captureVideo(source: LiveVideoSource, limits: LiveVideoLimits, cameraId = ''): Promise<MediaStream> {
  if (typeof VideoEncoder === 'undefined') throw new DOMException('WebCodecs is unavailable', 'NotSupportedError');
  const video = { width: { max: limits.width }, height: { max: limits.height }, frameRate: { max: limits.framerate } };
  return source === 'camera'
    ? navigator.mediaDevices.getUserMedia({ video: { ...video, ...(cameraId ? { deviceId: { exact: cameraId } } : {}) }, audio: false })
    : navigator.mediaDevices.getDisplayMedia({ video, audio: false });
}

type FrameReader = { read(): Promise<{ done: boolean; value?: VideoFrame }>; cancel(): Promise<void> };
type TrackProcessor = new (init: { track: MediaStreamTrack }) => { readable: { getReader(): FrameReader } };

/// 取り込んだ映像を VP8 に圧縮して `push` へ順に渡す。`preview` は自分の映像の表示で、`MediaStreamTrackProcessor` の無い
/// WebKitGTK では frame の取り出しにも使う。取り込みが終わったら(画面の共有の停止など)`onEnded`。
export function encodeVideo(options: {
  stream: MediaStream;
  limits: LiveVideoLimits;
  preview: HTMLVideoElement;
  push: (keyframe: boolean, ageUs: number, data: Uint8Array) => Promise<void>;
  onEnded: () => void;
}): { stop(): void } {
  const { stream, limits, preview, push, onEnded } = options;
  const [track] = stream.getVideoTracks();
  const canvas = document.createElement('canvas');
  const pending: EncodedVideoChunk[] = [];
  let stopped = false;
  let pushing = false;
  let configured = '';
  let forceKeyframe = true;
  let lastFrameUs = -Infinity;
  let lastKeyframeUs = -Infinity;
  let reader: FrameReader | null = null;

  const send = async () => {
    if (pushing) return;
    pushing = true;
    while (!stopped && pending.length > 0) {
      const chunk = pending.shift()!;
      const data = new Uint8Array(chunk.byteLength);
      chunk.copyTo(data);
      await push(chunk.type === 'key', performance.now() * 1000 - chunk.timestamp, data).catch(() => {
        forceKeyframe = true;
      });
    }
    pushing = false;
  };
  const encoder = new VideoEncoder({
    output: (chunk) => {
      // 送りが 1 秒分より遅れたら、溜まった分を捨てて次の keyframe から送り直す。
      if (pending.length >= PENDING_CHUNKS) {
        pending.length = 0;
        forceKeyframe = true;
        return;
      }
      pending.push(chunk);
      void send();
    },
    error: () => {
      if (!stopped) onEnded();
    },
  });
  const encode = (source: VideoFrame | HTMLVideoElement, width: number, height: number) => {
    const capturedUs = performance.now() * 1000;
    if (
      stopped ||
      encoder.state === 'closed' ||
      encoder.encodeQueueSize > ENCODER_QUEUE ||
      capturedUs - lastFrameUs < 800_000 / limits.framerate ||
      width === 0
    ) {
      return;
    }
    lastFrameUs = capturedUs;
    const size = fitVideoSize(width, height, limits);
    const key = `${size.width}x${size.height}`;
    if (key !== configured) {
      encoder.configure({
        codec: 'vp8',
        ...size,
        bitrate: limits.bitrate,
        framerate: limits.framerate,
        latencyMode: 'realtime',
      });
      configured = key;
      forceKeyframe = true;
    }
    let frame: VideoFrame;
    // WebKitGTK 2.50(Ubuntu 22.04)は `<video>` から作った frame の encode に失敗するので、canvas に描いてから作る(#1834)。
    if (source instanceof VideoFrame && size.width === width && size.height === height) {
      frame = new VideoFrame(source, { timestamp: capturedUs });
    } else {
      canvas.width = size.width;
      canvas.height = size.height;
      canvas.getContext('2d')?.drawImage(source, 0, 0, size.width, size.height);
      frame = new VideoFrame(canvas, { timestamp: capturedUs });
    }
    const keyFrame = forceKeyframe || capturedUs - lastKeyframeUs >= KEYFRAME_INTERVAL_US;
    if (keyFrame) {
      forceKeyframe = false;
      lastKeyframeUs = capturedUs;
    }
    encoder.encode(frame, { keyFrame });
    frame.close();
  };

  preview.srcObject = stream;
  preview.muted = true;
  void preview.play().catch(() => undefined);
  track.addEventListener('ended', () => {
    if (!stopped) onEnded();
  });
  const Processor = (window as unknown as { MediaStreamTrackProcessor?: TrackProcessor }).MediaStreamTrackProcessor;
  if (Processor) {
    reader = new Processor({ track }).readable.getReader();
    void (async () => {
      while (!stopped) {
        const { done, value } = await reader.read().catch(() => ({ done: true, value: undefined }));
        if (done || !value) return;
        encode(value, value.displayWidth, value.displayHeight);
        value.close();
      }
    })();
  } else {
    const onFrame = () => {
      if (stopped) return;
      encode(preview, preview.videoWidth, preview.videoHeight);
      preview.requestVideoFrameCallback(onFrame);
    };
    preview.requestVideoFrameCallback(onFrame);
  }
  // tick ごとに 1 枚送る(直前の frame に近すぎる分は encode が捨てる)。前の frame からの時間で決めると、tick の直前に
  // 画面が変わったときや tick の揺れで、次の送り直しまで 2 秒近くあく。
  const heartbeat = window.setInterval(
    () => encode(preview, preview.videoWidth, preview.videoHeight),
    HEARTBEAT_US / 1000
  );
  return {
    stop() {
      stopped = true;
      window.clearInterval(heartbeat);
      void reader?.cancel().catch(() => undefined);
      stream.getTracks().forEach((each) => each.stop());
      preview.srcObject = null;
      if (encoder.state !== 'closed') encoder.close();
    },
  };
}

/// 受けた映像を復号し、音声の再生時刻に合わせて canvas に描く。
export function createLivePlayer(canvas: HTMLCanvasElement) {
  const context = canvas.getContext('2d');
  const frames: VideoFrame[] = [];
  let clock = { us: 0, at: 0 };
  const audioNow = () => clock.us === 0 ? 0 : clock.us + (performance.now() - clock.at) * 1000;
  let waitingKeyframe = true;
  let lastTimestampUs = -Infinity;
  const createDecoder = () => {
    const decoder = new VideoDecoder({
      output: (frame) => {
        frames.push(frame);
        if (frames.length > DECODED_FRAMES) {
          // 次に出せるframeを残す。全て先の映像なら、新しい到着で待機中のframeを追い出さない。
          const ready = frameToShow(frames.map((frame) => frame.timestamp), audioNow());
          frames.splice(ready > 0 ? 0 : frames.length - 1, 1)[0]?.close();
        }
      },
      error: () => {
        waitingKeyframe = true;
      },
    });
    decoder.configure(DECODER_CONFIG);
    return decoder;
  };
  let decoder = createDecoder();
  let animation = 0;
  const draw = () => {
    const index = frameToShow(
      frames.map((frame) => frame.timestamp),
      audioNow()
    );
    if (index >= 0) {
      const shown = frames[index];
      frames.splice(0, index).forEach((frame) => frame.close());
      frames.shift();
      if (canvas.width !== shown.displayWidth || canvas.height !== shown.displayHeight) {
        canvas.width = shown.displayWidth;
        canvas.height = shown.displayHeight;
      }
      context?.drawImage(shown, 0, 0);
      shown.close();
    }
    animation = requestAnimationFrame(draw);
  };
  animation = requestAnimationFrame(draw);
  const clear = () => {
    frames.splice(0).forEach((frame) => frame.close());
    clock = { us: 0, at: 0 };
    waitingKeyframe = true;
  };
  return {
    push(batch: LiveVideoBatch) {
      if (batch.audioClockUs > 0) clock = { us: batch.audioClockUs, at: performance.now() };
      for (const chunk of batch.chunks) {
        // 配信を始め直すと frame の時刻が前へ戻る。WebKitGTK の decoder は前に出した frame より早い時刻の frame を出さない
        // ので、作り直して次の keyframe から decode する(#1809)。
        if (chunk.timestampUs < lastTimestampUs && decoder.state !== 'closed') decoder.close();
        lastTimestampUs = chunk.timestampUs;
        if (decoder.state === 'closed') {
          decoder = createDecoder();
          waitingKeyframe = true;
        }
        if (waitingKeyframe && !chunk.keyframe) continue;
        waitingKeyframe = false;
        decoder.decode(
          new EncodedVideoChunk({
            type: chunk.keyframe ? 'key' : 'delta',
            timestamp: chunk.timestampUs,
            data: chunk.data,
          })
        );
      }
    },
    /// 視聴をやり直す前に、前の映像と時刻を捨てる。
    reset() {
      clear();
      if (decoder.state !== 'closed') {
        decoder.reset();
        decoder.configure(DECODER_CONFIG);
      }
    },
    close() {
      cancelAnimationFrame(animation);
      clear();
      if (decoder.state !== 'closed') decoder.close();
    },
  };
}

export const LIVE_THUMBNAIL_INTERVAL_MS = 30_000;


/** 手動画像と自動の静止画に同じ縮小・JPEG変換を使う。 */
export function liveThumbnailDataUrl(source: CanvasImageSource, width: number, height: number): string {
  const scale = Math.min(384 / width, 216 / height, 1);
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(width * scale));
  canvas.height = Math.max(1, Math.round(height * scale));
  canvas.getContext('2d')!.drawImage(source, 0, 0, canvas.width, canvas.height);
  return canvas.toDataURL('image/jpeg', 0.5);
}

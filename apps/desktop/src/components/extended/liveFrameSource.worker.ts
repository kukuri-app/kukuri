import type { FrameReader, TrackProcessor } from './liveMedia';

// WorkerのAPIを使い、画面上でvideo→canvas→VideoFrameの読み戻しを行わない。
const Processor = (globalThis as unknown as { MediaStreamTrackProcessor?: TrackProcessor }).MediaStreamTrackProcessor;
const port = globalThis as unknown as Worker;
let reader: FrameReader;
port.onmessage = async ({ data }: MessageEvent<{ track?: MediaStreamTrack; read?: boolean }>) => {
  try {
    if (data.track) reader = new Processor!({ track: data.track, maxBufferSize: 1 }).readable.getReader();
    else if (data.read) {
      const { done, value } = await reader.read();
      if (value) {
        try { port.postMessage({ frame: value }, [value]); }
        finally { value.close(); }
      } else port.postMessage({ done });
    }
  } catch { port.postMessage({ failed: true }); }
};
port.postMessage({ supported: Boolean(Processor) });

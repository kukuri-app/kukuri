// SA-4の診断(#1944)。既存の録音bufferを使い、失敗時だけPCMと計測値をartifactへ残す。
import { execFile } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

export function echoDiagnostics({ listener, speaker, heard, echo, voice, reply }) {
  const instances = [listener, speaker];
  const snapshots = [];
  const startedAt = Date.now();
  let pending = null;
  const pulse = (kind) => new Promise((resolve) => {
    execFile('pactl', ['--format=json', 'list', kind], { encoding: 'utf8', timeout: 500, maxBuffer: 65_536 }, (error, stdout) => {
      try {
        if (error) throw error;
        resolve(JSON.parse(stdout).flatMap((item) => {
          const instance = instances.find((each) => String(each.pid) === item.properties?.['application.process.id']);
          return instance ? [{
            instance: instance.name, index: item.index, sink: item.sink, source: item.source,
            buffer_latency_usec: item.buffer_latency_usec, sink_latency_usec: item.sink_latency_usec,
            source_latency_usec: item.source_latency_usec, sample_specification: item.sample_specification,
            resample_method: item.resample_method,
          }] : [];
        }));
      } catch (reason) {
        resolve({ error: String(reason.message).slice(0, 500) });
      }
    });
  });
  const sample = () => {
    if (pending || snapshots.length >= 12) return;
    const at = Date.now();
    const levels = { own: echo.level(1_000, voice), reply: echo.level(1_000, reply) };
    pending = Promise.all([pulse('sink-inputs'), pulse('source-outputs')])
      .then(([playback, capture]) => snapshots.push({ at, ...levels, playback, capture }))
      .finally(() => { pending = null; });
  };
  sample();
  const timer = setInterval(sample, 1_000);
  return {
    async stop() {
      clearInterval(timer);
      await pending;
    },
    async save({ near, own, reply: replyLevel, window, listener: listenerWindow }) {
      await this.stop();
      const dir = join(dirname(speaker.log), 'sa4');
      mkdirSync(dir, { recursive: true });
      const recording = (name, captured) => {
        // 受信が一度に届いても、保存量は4秒と境界の1 blockまで。切れた量もJSONへ残す。
        const frames = captured.frames.slice(-201);
        const pcm = Buffer.alloc(frames.reduce((sum, frame) => sum + frame.value.length, 0) * 2);
        let offset = 0;
        for (const { value } of frames) for (const sample of value) {
          pcm.writeInt16LE(Math.round(sample * 32768), offset);
          offset += 2;
        }
        const file = `${name}.s16le`;
        writeFileSync(join(dir, file), pcm);
        return {
          file, rate: captured.rate, format: 's16le', channels: 1, at: captured.at, ms: captured.ms,
          observed_blocks: captured.frames.length,
          blocks: frames.map(({ at, start, value }) => ({ at, start, samples: value.length })),
        };
      };
      const recordings = { listener: recording('listener', listenerWindow), speaker: recording('speaker', window) };
      writeFileSync(join(dir, 'measurement.json'), JSON.stringify({
        started_at: startedAt, near, own, reply: replyLevel, own_ratio: own / replyLevel,
        bands: { voice, reply }, recordings, snapshots,
      }, null, 2));
    },
  };
}

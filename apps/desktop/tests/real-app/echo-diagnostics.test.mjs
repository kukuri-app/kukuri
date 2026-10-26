import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { EventEmitter } from 'node:events';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setImmediate } from 'node:timers/promises';
import { afterEach, beforeEach, mock, test } from 'node:test';

const streams = new Map();
const pulse = mock.method(childProcess, 'execFile', (_command, args, _options, callback) => {
  callback(null, JSON.stringify([1, 2, 99].map((pid) => ({
    index: pid, sink: 7, source: 8, buffer_latency_usec: 20_000,
    sink_latency_usec: 5_000, source_latency_usec: 6_000,
    properties: { 'application.process.id': String(pid) },
    sample_specification: 's16le 1ch 48000Hz',
  }))));
});
mock.method(childProcess, 'spawn', (_command, args) => {
  const stdout = new EventEmitter();
  streams.set(args[0], stdout);
  return { stdout, kill() {} };
});
syncBuiltinESMExports();
process.env.REAL_APP_AUDIO = 'pulse';
const { record } = await import('./audio.mjs');

let dir;
beforeEach(() => {
  mock.timers.enable({ apis: ['Date', 'setInterval'], now: 1_000_000 });
  dir = mkdtempSync(join(tmpdir(), 'kukuri-echo-'));
});
afterEach(() => {
  mock.timers.reset();
  rmSync(dir, { recursive: true, force: true });
});

const voice = [500, 500];
const replyBand = [2000, 2000];
function block(own = 0.01, reply = 0.1) {
  const pcm = Buffer.alloc(960 * 2);
  for (let i = 0; i < 960; i++) {
    const value = own * Math.sin(2 * Math.PI * 500 * i / 48_000) + reply * Math.sin(2 * Math.PI * 2000 * i / 48_000);
    pcm.writeInt16LE(Math.round(value * 32768), i * 2);
  }
  return pcm;
}
function feed(pcm) {
  for (const stream of streams.values()) stream.emit('data', pcm);
}
function fixture() {
  const listener = { name: 'listener', pid: 1, log: join(dir, 'listener.log') };
  const speaker = { name: 'speaker', pid: 2, log: join(dir, 'speaker.log') };
  return { listener, speaker, heard: record(listener), echo: record(speaker), voice, reply: replyBand };
}

test('判定に使った録音窓を固定し、後から届くPCMを含めない', () => {
  const { echo } = fixture();
  feed(block());
  const window = echo.snapshot(4_000);
  const own = echo.level(4_000, voice, window);
  assert.ok(Math.abs(own - 0.01) < 0.0001);
  assert.equal(own, echo.level(4_000, voice));
  mock.timers.tick(20);
  feed(block(0.2));
  assert.equal(echo.level(4_000, voice, window), own);
  assert.ok(echo.level(4_000, voice) > own);
});

test('失敗の値と同じ窓のPCM・受信時刻・対象instanceの遅延を保存する', async () => {
  const { echoDiagnostics } = await import('./echo-diagnostics.mjs');
  const context = fixture();
  const diagnostics = echoDiagnostics(context);
  try {
    const pcm = block();
    for (let i = 0; i < 250; i++) {
      feed(pcm);
      mock.timers.tick(20);
    }
    feed(Buffer.concat(Array.from({ length: 250 }, () => pcm)));
    const window = context.echo.snapshot(4_000);
    const own = context.echo.level(4_000, voice, window);
    const reply = context.echo.level(4_000, replyBand, window);
    await diagnostics.save({ near: 0.1, own, reply, window, listener: context.heard.snapshot(4_000, window.at) });
    const saved = JSON.parse(readFileSync(join(dir, 'sa4', 'measurement.json'), 'utf8'));
    assert.equal(saved.own, own);
    assert.equal(saved.reply, reply);
    assert.equal(saved.own_ratio, own / reply);
    assert.equal(saved.near, 0.1);
    for (const name of ['listener', 'speaker']) {
      const recording = saved.recordings[name];
      assert.equal(recording.at, window.at);
      assert.equal(recording.rate, 48_000);
      assert.equal(recording.format, 's16le');
      assert.ok(recording.observed_blocks > 201, '一度に届いた分を含む窓で保存上限を確かめる');
      assert.equal(recording.blocks.at(-1).at, window.frames.at(-1).at);
      const data = readFileSync(join(dir, 'sa4', recording.file));
      assert.deepEqual(data, Buffer.concat(Array.from({ length: recording.blocks.length }, () => pcm)));
      assert.ok(data.length <= 201 * 960 * 2);
    }
    assert.ok(saved.snapshots.length > 0);
    assert.deepEqual(saved.snapshots[0].playback.map((item) => item.instance), ['listener', 'speaker']);
    assert.equal(saved.snapshots[0].capture[0].source_latency_usec, 6_000);
    assert.ok(!JSON.stringify(saved).includes('"index":99'));
  } finally {
    await diagnostics.stop();
  }
});

test('観測は最大12回で止まり、成功時は音声を保存せずtimerを回収する', async () => {
  const { echoDiagnostics } = await import('./echo-diagnostics.mjs');
  const diagnostics = echoDiagnostics(fixture());
  const before = pulse.mock.calls.length;
  for (let i = 0; i < 20; i++) {
    mock.timers.tick(1_000);
    await setImmediate();
  }
  await diagnostics.stop();
  const stopped = pulse.mock.calls.length;
  assert.ok(stopped - before <= 24);
  assert.ok(stopped - before > 0);
  mock.timers.tick(20_000);
  await setImmediate();
  assert.equal(pulse.mock.calls.length, stopped);
  assert.deepEqual(readdirSync(dir), []);
  for (const call of pulse.mock.calls.slice(before)) {
    assert.equal(call.arguments[2].timeout, 500);
    assert.equal(call.arguments[2].maxBuffer, 65_536);
  }
});

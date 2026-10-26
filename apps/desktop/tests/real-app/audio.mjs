// OS の標準の音声の入出力の境界で、入れた音と出た音を確かめる(#1800)。`REAL_APP_AUDIO` は CI の仮想デバイスの構成を
// 示し、手元では付けない(付けなければ音声の試験は飛ばす。既定の機器を使わないため)。
// - pulse(Linux): instance ごとに PulseAudio の null sink を入力用・出力用に作り、PULSE_SOURCE・PULSE_SINK で振り分ける。
//   入力は `<name>_in` の monitor、出力は `<name>_out`。
// - vbcable(Windows): 仮想の線(VB-CABLE)1 本を、全 instance が既定の入出力として共有する。入れる音は既定の出力
//   (線の入口)への再生で、全 instance の mic に入る。出た音は instance の process の音声 session の peak で測る。
import { execFileSync, spawn } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const RATE = 48_000;
const BLOCK = RATE / 50;
const KEEP_MS = 30_000;
const ON_MS = 150;
/// 音の始まりを探す幅(2 ms)。
const ONSET_WINDOW = RATE / 500;
/// 話す音の周期。大きさを比べるときは、この倍数の間で平均する。
export const PERIOD_MS = 400;

export const audioMode = process.env.REAL_APP_AUDIO ?? null;
/// instance ごとに入力と出力を分けられる構成か。
export const isolated = audioMode === 'pulse';

const pactl = (...args) => execFileSync('pactl', args, { encoding: 'utf8', env: { ...process.env, LC_ALL: 'C' } });
const sinkModules = () =>
  new Map(
    pactl('list', 'short', 'modules')
      .split('\n')
      .map((line) => [/sink_name=(\S+)/.exec(line)?.[1], line.split('\t')[0]])
      .filter(([sink]) => sink)
  );
const addSink = (sink) => {
  if (!sinkModules().has(sink)) pactl('load-module', 'module-null-sink', `sink_name=${sink}`, `rate=${RATE}`, 'channels=1');
};

/// instance の起動に渡す環境変数(音声を使わない実行では空)。instance は XDG の設定先を分けるので、server の場所も渡す。
export function audioEnv(name) {
  if (!isolated) return {};
  addSink(`${name}_in`);
  addSink(`${name}_out`);
  return {
    PULSE_SERVER: /^Server String: (.+)$/m.exec(pactl('info'))[1],
    PULSE_SOURCE: `${name}_in.monitor`,
    PULSE_SINK: `${name}_out`,
  };
}

/// `name` の入力デバイスを外す(開けない構成)。`plug` で付け直す。
export function unplug(name) {
  pactl('unload-module', sinkModules().get(`${name}_in`));
}

export function plug(name) {
  addSink(`${name}_in`);
}

/// `instance` の取り込みを自分の出力の monitor へ移す(スピーカーの音がマイクに入る部屋)。返す関数で戻す(mic を
/// off にする前に戻す)。PulseAudio の戻しの module は遅延を保つために変換の比を揺らし続け、エコーキャンセルが
/// 追えないため使わない(実際の部屋では起きない)。
export function hearOwnOutput(instance) {
  const id = pactl('list', 'source-outputs')
    .split(/\n(?=Source Output #)/)
    .find((block) => block.includes(`application.process.id = "${instance.pid}"`))
    ?.match(/^Source Output #(\d+)/)?.[1];
  pactl('move-source-output', id, `${instance.name}_out.monitor`);
  return () => pactl('move-source-output', id, `${instance.name}_in.monitor`);
}

/// `name` の入力(`sink` を指定すればその sink。共有の線では全 instance の入力)へ、話し声のように途切れる音(400 ms
/// ごとに 150 ms)を流し続ける。返す関数で止める。音は `band`(Hz、50 Hz 刻み)の周波数を、音ごとに無作為な位相で
/// 同時に鳴らす(帯域の雑音に近い)。途切れない音は雑音除去が弱め、1 つの周波数の音はエコーキャンセルが学習しきれ
/// ないか、遅れた自分の声から予測して消してしまうため(どれも実際の声では起きにくい)。
export function speak(name, band, sink = `${name}_in`) {
  const next = voice(band);
  if (!isolated) {
    // 30 秒分を繰り返し鳴らす。
    const file = join(tmpdir(), `kukuri-real-app-${name}.wav`);
    const pcm = Buffer.concat(Array.from({ length: 1_500 }, next));
    const header = Buffer.alloc(44);
    header.write('RIFF', 0);
    header.writeUInt32LE(36 + pcm.length, 4);
    header.write('WAVEfmt ', 8);
    header.writeUInt32LE(16, 16);
    header.writeUInt16LE(1, 20);
    header.writeUInt16LE(1, 22);
    header.writeUInt32LE(RATE, 24);
    header.writeUInt32LE(RATE * 2, 28);
    header.writeUInt16LE(2, 32);
    header.writeUInt16LE(16, 34);
    header.write('data', 36);
    header.writeUInt32LE(pcm.length, 40);
    writeFileSync(file, Buffer.concat([header, pcm]));
    const player = spawn(
      'powershell',
      ['-NoProfile', '-Command', `(New-Object Media.SoundPlayer '${file}').PlayLooping(); Start-Sleep -Seconds 86400`],
      { stdio: 'ignore' }
    );
    return () => player.kill();
  }
  const player = spawn(
    'pacat',
    ['--playback', `--device=${sink}`, '--format=s16le', `--rate=${RATE}`, '--channels=1', '--latency-msec=20'],
    { stdio: ['pipe', 'ignore', 'ignore'] }
  );
  let stopped = false;
  const pump = () => {
    while (!stopped) {
      if (!player.stdin.write(next())) {
        player.stdin.once('drain', pump);
        return;
      }
    }
  };
  player.stdin.on('error', () => undefined);
  pump();
  return () => {
    stopped = true;
    player.kill();
  };
}

/// `name` の入力へ無音を流し続け、`burst()` で話す音を 1 つ(150 ms)入れる(時刻を測る音。instance ごとの入力が要る)。
/// 書込みは実時間に合わせ、先へ溜めない(入れた時刻と入力に届く時刻を近づける)。
export function bursts(name, band) {
  const frequencies = bins(band);
  const each = 0.2 / Math.sqrt(frequencies.length);
  const player = spawn(
    'pacat',
    ['--playback', `--device=${name}_in`, '--format=s16le', `--rate=${RATE}`, '--channels=1', '--latency-msec=20'],
    { stdio: ['pipe', 'ignore', 'ignore'] }
  );
  player.stdin.on('error', () => undefined);
  const begin = Date.now();
  let written = 0;
  let left = 0;
  let phases = [];
  const timer = setInterval(() => {
    while (written * 1000 < (Date.now() - begin + 40) * RATE) {
      const block = Buffer.alloc(BLOCK * 2);
      for (let i = 0; i < BLOCK && left > 0; i += 1, left -= 1) {
        const at = (RATE * ON_MS) / 1000 - left;
        let value = 0;
        for (const [index, frequency] of frequencies.entries()) {
          value += Math.sin((2 * Math.PI * frequency * at) / RATE + phases[index]) * each;
        }
        block.writeInt16LE(Math.round(value * 32767), i * 2);
      }
      player.stdin.write(block);
      written += BLOCK;
    }
  }, 10);
  return {
    burst() {
      phases = frequencies.map(() => Math.random() * 2 * Math.PI);
      left = (RATE * ON_MS) / 1000;
    },
    stop() {
      clearInterval(timer);
      player.kill();
    },
  };
}

/// 話す音の 20 ms ずつの PCM(s16le・mono)を返す関数。強さは振幅 0.2 の 1 つの周波数の音と同じ。
function voice(band) {
  const frequencies = bins(band);
  const each = 0.2 / Math.sqrt(frequencies.length);
  const period = (RATE * PERIOD_MS) / 1000;
  let sample = 0;
  let phases = [];
  return () => {
    const block = Buffer.alloc(BLOCK * 2);
    for (let i = 0; i < BLOCK; i += 1, sample += 1) {
      if (sample % period === 0) phases = frequencies.map(() => Math.random() * 2 * Math.PI);
      let value = 0;
      if (sample % period < (RATE * ON_MS) / 1000) {
        for (const [index, frequency] of frequencies.entries()) {
          value += Math.sin((2 * Math.PI * frequency * sample) / RATE + phases[index]) * each;
        }
      }
      block.writeInt16LE(Math.round(Math.max(-1, Math.min(1, value)) * 32767), i * 2);
    }
    return block;
  };
}

/// `instance` の出力に出た音を記録する(直近 30 秒)。`level(ms, band)` は直近 `ms` の音の大きさ(RMS、共有の線では
/// peak)。`band` を指定すると、その帯域(`speak` と同じ 50 Hz 刻み)の成分の振幅(instance ごとの出力でだけ測れる)。
/// `onsets(since, threshold)` は、`since`(epoch の ms)より後に始まった音の時刻(instance ごとの出力でだけ測れる)。
/// `device` は記録する先(instance ごとの構成で、出力の代わりに入力などを記録する)。
export function record(instance, device = `${instance.name}_out.monitor`) {
  const values = [];
  // 記録の最初の sample の時刻(届いた時刻から、それまでの長さを引いた値の最小)。
  let origin = Infinity;
  let received = 0;
  const keep = (value) => {
    const now = Date.now();
    values.push({ at: now, start: received, value });
    while (values.length && values[0].at < now - KEEP_MS) values.shift();
  };
  const recorder = isolated
    ? spawn(
        'parec',
        [`--device=${device}`, '--format=s16le', `--rate=${RATE}`, '--channels=1', '--latency-msec=20'],
        { stdio: ['ignore', 'pipe', 'ignore'] }
      )
    : spawn(
        'powershell',
        [
          '-NoProfile',
          '-Command',
          `Add-Type -TypeDefinition (Get-Content -Raw -Encoding UTF8 '${fileURLToPath(new URL('./session-meter.cs', import.meta.url))}');` +
            ` [SessionMeter]::Run(${instance.pid})`,
        ],
        { stdio: ['ignore', 'pipe', 'ignore'] }
      );
  let pending = isolated ? Buffer.alloc(0) : '';
  recorder.stdout.on('data', (chunk) => {
    if (!isolated) {
      const lines = (pending + chunk).split(/\r?\n/);
      pending = lines.pop();
      for (const line of lines) keep(Number(line));
      return;
    }
    pending = Buffer.concat([pending, chunk]);
    while (pending.length >= BLOCK * 2) {
      const samples = new Float32Array(BLOCK);
      for (let i = 0; i < BLOCK; i += 1) samples[i] = pending.readInt16LE(i * 2) / 32768;
      pending = pending.subarray(BLOCK * 2);
      keep(samples);
      received += BLOCK;
      origin = Math.min(origin, Date.now() - (received * 1000) / RATE);
    }
  });
  return {
    // SA-4の判定と保存に同じ窓を使う。sampleはコピーせず、後の受信から切り離した一覧を返す。
    snapshot(ms, at = Date.now()) {
      return { at, ms, rate: RATE, frames: values.filter((each) => each.at >= at - ms && each.at <= at) };
    },
    level(ms, band = null, snapshot = null) {
      const inside = (snapshot?.frames ?? values.filter((each) => each.at >= Date.now() - ms)).map(({ value }) => value);
      if (!inside.length) return 0;
      const levels = isolated
        ? inside.map((samples) =>
            band === null ? rms(samples) : Math.hypot(...bins(band).map((frequency) => tone(samples, frequency)))
          )
        : inside;
      return levels.reduce((sum, value) => sum + value, 0) / levels.length;
    },
    onsets(since, threshold) {
      const found = [];
      let loud = -Infinity;
      for (const { start, value } of values) {
        for (let offset = 0; offset < value.length; offset += ONSET_WINDOW) {
          const time = origin + ((start + offset) * 1000) / RATE;
          if (time <= since || rms(value.subarray(offset, offset + ONSET_WINDOW)) <= threshold) continue;
          // 300 ms の無音の後を、次の音の始まりとする。
          if (time - loud > 300) found.push(time);
          loud = time;
        }
      }
      return found;
    },
    stop: () => recorder.kill(),
  };
}

/// `band` の 50 Hz 刻みの周波数(20 ms の block にちょうど収まり、隣の周波数へ漏れない)。
function bins([low, high]) {
  return Array.from({ length: (high - low) / 50 + 1 }, (_, index) => low + index * 50);
}

function rms(samples) {
  let sum = 0;
  for (const sample of samples) sum += sample * sample;
  return Math.sqrt(sum / samples.length);
}

/// Goertzel で `frequency` の成分の振幅を求める。
function tone(samples, frequency) {
  const coefficient = 2 * Math.cos((2 * Math.PI * frequency) / RATE);
  let previous = 0;
  let beforePrevious = 0;
  for (const sample of samples) {
    const current = sample + coefficient * previous - beforePrevious;
    beforePrevious = previous;
    previous = current;
  }
  const power = previous * previous + beforePrevious * beforePrevious - coefficient * previous * beforePrevious;
  return (2 * Math.sqrt(Math.max(power, 0))) / samples.length;
}

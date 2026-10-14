// 実際のアプリを複数動かす自動の試験の入口(#1800)。
// 使い方: node tests/real-app/run.mjs --app <配布と同じ build の実行 file> [--scenario <名前>...] [--root <作業 dir>]
import { mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';

import { launch } from './driver.mjs';
import { TOPIC } from './session.mjs';

const SCENARIOS = {
  smoke: () => import('./scenarios/smoke.mjs'),
  'spatial-audio': () => import('./scenarios/spatial-audio.mjs'),
};

const { values } = parseArgs({
  options: {
    app: { type: 'string' },
    scenario: { type: 'string', multiple: true },
    root: { type: 'string' },
  },
});
if (!values.app) throw new Error('--app is required');
const app = resolve(values.app);
const root = resolve(values.root ?? join(tmpdir(), 'kukuri-real-app'));
const names = values.scenario ?? Object.keys(SCENARIOS);

let failed = 0;
for (const name of names) {
  const dir = join(root, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const started = [];
  let port = 9500;
  const context = {
    /// 名前ごとに instance を起動する(終わったら止める)。
    async launch(...instanceNames) {
      const instances = await Promise.all(
        instanceNames.map((instanceName) => launch({ app, root: dir, name: instanceName, port: port++ }))
      );
      started.push(...instances);
      return instances;
    },
    /// instance を止め、同じ profile のまま起動し直す。
    async restart(instance) {
      await instance.close();
      const next = await launch({ app, root: dir, name: instance.name, port: instance.port, fresh: false });
      started[started.indexOf(instance)] = next;
      return next;
    },
    log: (message) => console.log(`[${name}] ${message}`),
    topic: TOPIC,
  };
  const begin = Date.now();
  try {
    const { run } = await SCENARIOS[name]();
    await run(context);
    console.log(`PASS ${name} (${((Date.now() - begin) / 1000).toFixed(1)} s)`);
  } catch (error) {
    failed += 1;
    console.log(`FAIL ${name}: ${error?.stack ?? error}`);
    // 失敗の時点で各 instance の画面に見えているもの(調べるため)。
    for (const instance of started) {
      const seen = await instance
        .evaluate(
          `[...document.querySelectorAll('h1, h2, h3, button, [role="status"], [role="alert"]')]` +
            `.filter((node) => node.getClientRects().length > 0)` +
            `.map((node) => (node.getAttribute('aria-label') ?? node.textContent ?? '').trim()).filter(Boolean).join(' | ')`
        )
        .catch((reason) => `unavailable: ${reason}`);
      console.log(`[${name}] ${instance.name} sees: ${seen}`);
    }
  } finally {
    await Promise.all(started.map((instance) => instance.close()));
  }
}
process.exitCode = failed ? 1 : 0;

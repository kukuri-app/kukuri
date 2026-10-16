// Dome の既定のアバター(blumochichi.vrm)が、VRM の texture の色で描かれる(#1141 AC-1)。texture が読めないと、
// material の白い基本色だけで描かれる。描かれた画面の中央(camera が見るアバターの中心)の色で確かめ、画像を残す。
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

import { eventually } from '../../web-e2e/wait.mjs';
import { accept, openTopic } from '../session.mjs';
import { createDome, text } from '../ui.mjs';

/// texture の水色は赤が青の半分ほど。白い基本色は照明の色(空の光が少し青い)のまま描かれ、赤と青が近い。
const MAX_RED_TO_BLUE = 0.65;

/// 描いた直後の frame(描画の requestAnimationFrame の後)を縮めて写し、中央の画素の赤÷青の中央値と平均の色を返す。
/// 目などの別の色が中央に入っても、体の色で判定するため中央値を使う。
const SAMPLE = `new Promise((resolve) => requestAnimationFrame(() => {
  const source = document.querySelector('.metaverse-viewport-canvas canvas');
  if (!source) return resolve(null);
  const copy = document.createElement('canvas');
  copy.width = 320;
  copy.height = Math.round((320 * source.height) / source.width);
  const context = copy.getContext('2d');
  context.drawImage(source, 0, 0, copy.width, copy.height);
  const { data } = context.getImageData(copy.width / 2 - 20, copy.height / 2 - 20, 40, 40);
  const ratios = [];
  const sum = [0, 0, 0];
  for (let index = 0; index < data.length; index += 4) {
    ratios.push(data[index] / Math.max(1, data[index + 2]));
    for (let channel = 0; channel < 3; channel += 1) sum[channel] += data[index + channel];
  }
  ratios.sort((left, right) => left - right);
  const [red, green, blue] = sum.map((value) => Math.round(value / ratios.length));
  resolve({ ratio: ratios[ratios.length >> 1], red, green, blue, png: copy.toDataURL('image/png') });
}))`;

export async function run({ launch, log, topic }) {
  const [instance] = await launch('a');
  await accept(instance);
  await openTopic(instance, topic, 'game');
  await createDome(instance, 'dome-avatar');
  // 読み込み中と読めなかったときは、代わりの青い形が描かれる。VRM を読めた(texture の読み込みも終えた)のを待つ。
  const loaded = text('metaverse', 'hud.avatarStatuses.sample-vrm');
  await eventually('the avatar VRM to load', () =>
    instance.evaluate(`document.querySelector('.metaverse-room-diagnostics')?.textContent.includes(${JSON.stringify(loaded)})`)
  );
  let seen = null;
  try {
    await eventually(
      'the avatar to be drawn in its texture colors',
      async () => {
        seen = (await instance.evaluate(SAMPLE)) ?? seen;
        return seen && seen.ratio < MAX_RED_TO_BLUE;
      },
      30_000
    );
  } finally {
    if (seen) {
      writeFileSync(join(dirname(instance.log), 'avatar.png'), Buffer.from(seen.png.split(',')[1], 'base64'));
      log(`the avatar center is rgb(${seen.red}, ${seen.green}, ${seen.blue}), red/blue median ${seen.ratio.toFixed(2)}`);
    }
  }
}

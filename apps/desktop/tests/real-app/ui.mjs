// 画面の操作(#1800)。文言は日本語の翻訳 file から引き、利用者と同じく見えている button を押す。
import { readFileSync } from 'node:fs';

import { eventually } from '../web-e2e/wait.mjs';

const locales = new Map();

/// 翻訳の文言(`namespace` の file の `key`、`.` 区切り)。
export function text(namespace, key) {
  if (!locales.has(namespace)) {
    const file = new URL(`../../src/i18n/locales/ja/${namespace}.json`, import.meta.url);
    locales.set(namespace, JSON.parse(readFileSync(file, 'utf8')));
  }
  const value = key.split('.').reduce((node, part) => node?.[part], locales.get(namespace));
  if (typeof value !== 'string') throw new Error(`no text for ${namespace}:${key}`);
  return value;
}

/// 名前(aria-label か表示の文字)が `label` の、見えている button のうち `nth` 番目(負なら後ろから)を押すまで待つ。
/// `prefix` なら名前が `label` で始まる button(状態の文字を後ろに持つ button)。`within` は探す範囲(CSS の selector)。
export async function click(instance, label, { nth = 0, prefix = false, within = 'body', timeout = 30_000 } = {}) {
  await eventually(
    `${instance.name} to click "${label}"`,
    () =>
      instance.evaluate(
        `(() => {
          const scope = document.querySelector(${JSON.stringify(within)});
          const visible = [...(scope?.querySelectorAll('button, [role="button"], [role="menuitem"], summary') ?? [])].filter((node) => {
            const name = (node.getAttribute('aria-label') ?? node.textContent ?? '').trim();
            return (${prefix} ? name.startsWith(${JSON.stringify(label)}) : name === ${JSON.stringify(label)})
              && node.getClientRects().length > 0 && !node.disabled;
          });
          const node = visible.at(${nth});
          if (!node) return false;
          node.scrollIntoView({ block: 'center' });
          node.click();
          return true;
        })()`,
        { gesture: true }
      ),
    timeout
  );
}

/// placeholder が `placeholder` の入力欄へ書く(React が受け取る input の event を送る)。
export async function fill(instance, placeholder, value) {
  await eventually(`${instance.name} to fill "${placeholder}"`, () =>
    instance.evaluate(
      `(() => {
        const node = [...document.querySelectorAll('input, textarea')].find(
          (each) => each.placeholder === ${JSON.stringify(placeholder)} && each.getClientRects().length > 0
        );
        if (!node) return false;
        const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(node), 'value').set;
        setter.call(node, ${JSON.stringify(value)});
        node.dispatchEvent(new Event('input', { bubbles: true }));
        return true;
      })()`
    )
  );
}

/// 見出しが `label` の選択欄で、表示が `option` の項目を選ぶ(React が受け取る change の event を送る)。
export async function choose(instance, label, option) {
  await eventually(`${instance.name} to choose "${option}" in "${label}"`, () =>
    instance.evaluate(
      `(() => {
        const field = [...document.querySelectorAll('label')].find(
          (each) => each.querySelector('span')?.textContent.trim() === ${JSON.stringify(label)} && each.getClientRects().length > 0
        )?.querySelector('select');
        const value = [...(field?.options ?? [])].find((each) => each.textContent.trim() === ${JSON.stringify(option)})?.value;
        if (!field || value === undefined || field.disabled) return false;
        Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(field, value);
        field.dispatchEvent(new Event('change', { bubbles: true }));
        return true;
      })()`
    )
  );
}

/// Metaverse の列で Dome を作り、稼働させて入る。
export async function createDome(instance, title) {
  const t = (key) => text('metaverse', key);
  await click(instance, t('create.action'));
  await fill(instance, t('create.titlePlaceholder'), title);
  await click(instance, t('create.action'), { nth: -1 });
  await click(instance, t('management.open'));
  await click(instance, t('management.startAndEnter'));
  await entered(instance);
}

/// Dome に入る(退出の button が出る)のを待つ。
export function entered(instance) {
  return eventually(
    `${instance.name} to enter a Dome`,
    async () => (await pressed(instance, text('metaverse', 'hud.leave'))) !== undefined,
    60_000
  );
}

/// `key` を押し、`hold()` が終わったら離す(window への key の event。画面は focus のある window の入力だけを受ける)。
/// 押している間は、実際の key と同じく繰り返しの keydown を送る(画面が押している key を一度忘れても続く)。
export async function press(instance, key, hold = async () => undefined) {
  const send = (type, repeat = false) =>
    instance.evaluate(
      `(window.dispatchEvent(new KeyboardEvent('${type}', { key: ${JSON.stringify(key)}, repeat: ${repeat} })), true)`
    );
  await send('keydown');
  const repeating = setInterval(() => send('keydown', true).catch(() => undefined), 100);
  try {
    await hold();
  } finally {
    clearInterval(repeating);
    await send('keyup');
  }
}

/// 画面が Rust へ渡す空間音声の要求(位置・話者・mic)と Rust の返事を、IPC の fetch で観測し始める。
export function observeAudio(instance) {
  return instance.evaluate(`(() => {
    const fetch = window.fetch;
    window.fetch = async (input, init) => {
      const response = await fetch(input, init);
      if (String(input).endsWith('/set_metaverse_audio')) {
        const status = await response.clone().json().catch(() => null);
        window.__audio = { request: JSON.parse(init.body).request, status };
      }
      return response;
    };
    return true;
  })()`);
}

/// 最後に観測した空間音声の要求と返事(`{ request, status }`)。
export function audioState(instance) {
  return instance.evaluate('window.__audio ?? null');
}

/// 名前が `label` の button の `aria-pressed`(button が無ければ undefined、属性が無ければ null)。
export function pressed(instance, label) {
  return instance.evaluate(
    `(() => {
      const node = [...document.querySelectorAll('button')].find((each) => each.getAttribute('aria-label') === ${JSON.stringify(label)});
      return node ? node.getAttribute('aria-pressed') : undefined;
    })()`
  );
}

// live の配信と視聴の試験の部品(#1800 AC-4)。camera は、webview の取り込みの API(許可の経路は本物を通す)が返す映像を
// 試験の映像(canvas)に置き換える。画面は Windows では本物(画面全体の自動の選択)、Linux の CI には画面の取り込みの
// portal が無いので置き換える。映像は視聴者の画面の canvas に描かれたもので確かめる。
import { eventually } from '../web-e2e/wait.mjs';
import { bursts, record } from './audio.mjs';
import { openTopic } from './session.mjs';
import { click, fill, text } from './ui.mjs';

export const t = (key) => text('live', key);
/// 個別ライブの列(映像はstage、参加・終了はfooter)。
export const CARD = '.shell-column-surface:has(.live-stage)';
export const DETAILS = '.live-details-dialog';
/// 配信者の配信の操作(shell の上部の bar。#1839 AC-3)。
export const BAR = 'section.shell-live-broadcast';
/// 配信者が話す音の帯域(Hz)。
export const VOICE = [300, 950];
const VIDEO = t('media.video');
const WINDOWS = process.platform === 'win32';
/// 試験の camera と、置き換えた画面の映像の大きさと赤の強さ。WebKitGTK の decoder は途中で大きさが変わっても最初の
/// 大きさで出すので、Linux は色で、本物の画面を使う Windows は大きさで見分ける。
const CAMERA = [640, 360];
const SCREEN = [1280, 720];
export const isCamera = (draw) => (WINDOWS ? draw.size === CAMERA.join('x') : draw.red < 100);
export const isScreen = (draw) => (WINDOWS ? draw.size !== CAMERA.join('x') : draw.red > 160);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/// camera(と Linux の画面)の映像を作り、取り込みの API を置き換える。`flashUntil` までは白く光らせ、光り始めて描いた
/// 時刻(epoch の ms)を `flashes` に残す。`deny` を真にすると、画面の選択を利用者が取り消したのと同じく断る。
const INJECTION = `(() => {
  if (window.__capture) return true;
  const sources = [${JSON.stringify(CAMERA)}, ${JSON.stringify(SCREEN)}].map(([width, height]) => {
    const canvas = document.createElement('canvas');
    [canvas.width, canvas.height] = [width, height];
    return canvas;
  });
  const state = { deny: false, flashUntil: 0, flashes: [] };
  let white = false;
  setInterval(() => {
    const now = performance.now();
    if (now < state.flashUntil && !white) state.flashes.push(performance.timeOrigin + now);
    white = now < state.flashUntil;
    for (const [index, canvas] of sources.entries()) {
      const context = canvas.getContext('2d');
      context.fillStyle = white ? '#fff' : 'rgb(' + [30, 220][index] + ',' + Math.round(80 + 60 * Math.sin(now / 300)) + ',60)';
      context.fillRect(0, 0, canvas.width, canvas.height);
    }
  }, 33);
  // WebKit は navigator.mediaDevices の JS の object を回収して作り直す(足した property が消える)ので、prototype を置き換える。
  const prototype = MediaDevices.prototype;
  const { getUserMedia, getDisplayMedia } = prototype;
  prototype.getUserMedia = async function (constraints) {
    const stream = await getUserMedia.call(this, constraints);
    stream.getTracks().forEach((track) => track.stop());
    return sources[0].captureStream(30);
  };
  prototype.getDisplayMedia = async function (constraints) {
    if (state.deny) throw new DOMException('the test cancelled the selection', 'NotAllowedError');
    return ${WINDOWS} ? getDisplayMedia.call(this, constraints) : sources[1].captureStream(15);
  };
  window.__capture = state;
  return true;
})()`;

/// 配信者の取り込みを置き換える(読み直した後は、もう一度呼ぶ)。Linux の webview は camera の疑似デバイスを持たないので、
/// inspector から有効にする(Windows は起動の設定の疑似の camera を使う)。
export async function injectCapture(host) {
  if (!WINDOWS) await host.send('Page.overrideSetting', { setting: 'MockCaptureDevicesEnabled', value: true });
  await host.evaluate(INJECTION);
}

/// 視聴者の canvas に描かれた frame を記録し始める(描いた時刻・左上の画素の色・frame の大きさ)。
export const observeDraws = (viewer) =>
  viewer.evaluate(`(() => {
    if (window.__draws) return true;
    const draws = [];
    const drawImage = CanvasRenderingContext2D.prototype.drawImage;
    CanvasRenderingContext2D.prototype.drawImage = function (source, ...rest) {
      const result = drawImage.call(this, source, ...rest);
      if (source instanceof VideoFrame && this.canvas.getAttribute('aria-label') === ${JSON.stringify(VIDEO)}) {
        const [red, , blue] = this.getImageData(2, 2, 1, 1).data;
        draws.push({ at: performance.timeOrigin + performance.now(), red, blue, size: source.displayWidth + 'x' + source.displayHeight,
          details: Boolean(this.canvas.closest('.live-details-dialog')), stage: Boolean(this.canvas.closest('.live-stage')) });
        if (draws.length > 3000) draws.shift();
      }
      return result;
    };
    window.__draws = draws;
    return true;
  })()`);

/// `viewer` の canvas に `since`(epoch の ms)より後に描かれた frame。
export const drawsSince = (viewer, since) => viewer.evaluate(`(window.__draws ?? []).filter((each) => each.at > ${since})`);

/// 個別ライブの列と配信の操作の bar の状態の表示(配信・視聴の状態と、失敗の理由)。
export const statusOf = (instance) =>
  instance.evaluate(
    `[...document.querySelectorAll('${CARD} [role="status"], ${CARD} [role="alert"], ${BAR} [role="status"], ${BAR} [role="alert"]')].map((node) => node.textContent).join(' | ')`
  );

/// live の列を閉じる(列の閉じる button)。列が消えるのを待つ。
export async function closeLiveColumn(instance) {
  await eventually(`${instance.name} to close the live column`, () =>
    instance.evaluate(`(() => {
      const button = document.querySelector('${CARD}')?.closest('[data-column-id]')?.querySelector('.shell-column-close-button');
      button?.click();
      return !document.querySelector('${CARD}');
    })()`), 30_000);
}

/// 投稿のサムネイルから情報のモーダルを開く。
export async function openLiveDetails(instance) {
  await eventually(`${instance.name} to open live details`, () => instance.evaluate(`(() => {
    if (document.querySelector('${DETAILS}')) return true;
    const button = document.querySelector('.post-live .live-thumbnail-open');
    if (!button) return false;
    button.scrollIntoView({ block: 'center' });
    button.click();
    return false;
  })()`, { gesture: true }), 60_000);
}

/// 投稿の情報を確認し、個別ライブを大きく表示する。
export async function selectLive(instance) {
  if (await instance.evaluate(`Boolean(document.querySelector('${CARD}'))`)) return;
  await openLiveDetails(instance);
  await click(instance, t('actions.stage'), { within: DETAILS });
  await eventually(`${instance.name} to show the live stage`, () => instance.evaluate(`Boolean(document.querySelector('${CARD}'))`), 30_000);
}

/// live の列を、開始投稿の情報から開き直す。
export async function reopenLiveColumn(instance, topic) {
  await openTopic(instance, topic, 'timeline');
  await selectLive(instance);
}

/// H が拡大表示からcameraの配信を始める(配信中の操作は shell の上部の bar)。
export async function startBroadcast(host) {
  await click(host, t('media.start.camera'), { within: CARD });
}

/// H が live session を作る。
export async function createSession(host, title) {
  await eventually(`${host.name} to open the post composer`, () => host.evaluate(`(() => {
    const column = document.querySelector('.shell-column-surface[data-active="true"]');
    if (column?.querySelector('.shell-column-composer')) return true;
    column?.querySelector('.shell-column-footer .shell-column-primary-action')?.click();
    return false;
  })()`, { gesture: true }), 30_000);
  await click(host, t('actions.start'), { within: '.shell-column-composer' });
  await fill(host, t('fields.placeholders.title'), title);
  await click(host, t('actions.start'), { nth: -1 });
  await selectLive(host);
}

/// `viewer` が session に参加し、描かれ始めるのを待つ。
export async function watch(viewer) {
  await click(viewer, text('common', 'actions.join'), { within: CARD, timeout: 60_000 });
  await eventually(`${viewer.name} to draw`, async () => (await drawsSince(viewer, Date.now() - 1_000)).length > 0, 60_000);
}

/// H の映像を白く光らせ、同時に H の入力へ音を 1 つ入れることを 5 回繰り返し、`viewer` の映像の遅れ(音に対する、配信者の
/// 側との差。ms)を返す。H の側は光り始めて描いた時刻と音が入力に届いた時刻、視聴者の側は光った frame を描いた時刻と音が
/// 出力に出た時刻(`heard`)。記録の遅れは両方で同じなので消える(instance ごとの入出力が要る)。
export async function measureSync(host, viewer, heard) {
  const said = record(host, `${host.name}_in.monitor`);
  const beeper = bursts(host.name, VOICE);
  const since = Date.now();
  try {
    await sleep(1_000);
    for (let i = 0; i < 5; i += 1) {
      await host.evaluate('(window.__capture.flashUntil = performance.now() + 150, true)');
      beeper.burst();
      await sleep(2_000);
    }
    const flashes = (await host.evaluate('window.__capture.flashes')).filter((at) => at > since);
    const drawn = (await drawsSince(viewer, since)).filter((draw) => draw.red > 200 && draw.blue > 200).map((draw) => draw.at);
    const [sent, out] = [said.onsets(since, 0.02), heard.onsets(since, 0.02)];
    const after = (times, at) => times.find((each) => each > at && each < at + 1_500) - at;
    return flashes.map((flash, index) => after(drawn, flash) - after(out, sent[index]));
  } finally {
    beeper.stop();
    said.stop();
  }
}

/// 映像の遅れが 45 ms・進みが 150 ms の範囲に入るか(5 回とも。2026-10-14 のユーザー判断で進みを 125 ms から広げた)。
export const inSync = (offsets) => offsets.length === 5 && offsets.every((offset) => offset >= -150 && offset <= 45);

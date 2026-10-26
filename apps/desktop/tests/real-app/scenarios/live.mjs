// live の映像・音声(#1034 の runbook の手順 1〜9、#1800 AC-4)。配信者 H が live session を作って配信し、視聴者 V1・V2 が
// 見る。映像は視聴者の画面の canvas に描かれたもの、音は OS の出力の境界で確かめる(取り込みの置き換えは live-media.mjs)。
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

import { eventually } from '../../web-e2e/wait.mjs';
import { audioMode, isolated, PERIOD_MS, record, speak } from '../audio.mjs';
import {
  BAR,
  CARD,
  DETAILS,
  closeLiveColumn,
  createSession,
  drawsSince,
  injectCapture,
  inSync,
  isCamera,
  isScreen,
  measureSync,
  observeDraws,
  openLiveDetails,
  selectLive,
  reopenLiveColumn,
  startBroadcast,
  statusOf,
  t,
  VOICE,
} from '../live-media.mjs';
import { accept, connect, openTopic, waitForPeers } from '../session.mjs';
import { click, text } from '../ui.mjs';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/// 大きさを測る間(話す音の周期の倍数)。
const WINDOW_MS = 5 * PERIOD_MS;
const logOf = (instance) => readFileSync(instance.log, 'utf8');
const JOIN = text('common', 'actions.join');

// 投稿→保存blob→別端末の通常media取得→WebViewのimgまで、元のPNG/GIFを保つ（#1950 AC-1）。
async function overlayImages(host, viewers, topic) {
  for (const kind of ['png', 'gif']) {
    const bytes = readFileSync(new URL(`../../fixtures/overlay-${kind === 'png' ? 'transparent.png' : 'animated.gif'}`, import.meta.url));
    const attachment = { file_name: `overlay.${kind}`, mime: `image/${kind}`, byte_size: bytes.length, data_base64: bytes.toString('base64'), role: 'image_original' };
    const id = await host.invoke('create_post', { request: { topic, content: kind === 'png' ? 'Overlay PNG' : '', reply_to: null, attachments: [attachment, attachment] } });
    const saved = (await host.invoke('list_thread', { request: { topic, thread_id: id, cursor: null, limit: 1 } })).items.find((post) => post.object_id === id);
    assert.ok(saved, 'the source post is available');
    for (const viewer of viewers) {
      if (process.platform === 'win32') await viewer.send('Page.bringToFront');
      else viewer.focus();
      const selector = `.live-stage [data-post-object-id="${id}"] .live-overlay-image img`;
      let observed;
      try {
        await eventually(`${viewer.name} to display the first ${kind}`, async () => {
          observed = await viewer.evaluate(`(() => {
            const images = document.querySelectorAll(${JSON.stringify(selector)});
            return { visibility: document.visibilityState, images: [...images].map((image) => ({ width: image.naturalWidth, complete: image.complete, src: image.src })),
              post: document.querySelector('.live-stage [data-post-object-id="${id}"]')?.textContent };
          })()`);
          return observed.images.length === 1 && observed.images[0].width === 48;
        }, 60_000);
      } catch (error) { throw new Error(`${error.message}; overlay=${JSON.stringify(observed)}`); }
      // asset URLのfetchはCSPのconnect-src外。通常APIで取得済みblobを照合し、表示URLを読み直さない。
      const payload = await viewer.invoke('get_blob_media_payload', { request: { hash: saved.attachments[0].hash, mime: attachment.mime, source_object_id: id } });
      assert.equal(payload.bytes_base64, attachment.data_base64, `${viewer.name} preserves ${kind} bytes through storage and fetch`);
    }
  }
}

/// (Linux)instance の window を最小化する。返す関数で戻す。
function minimize(instance) {
  const [id] = execFileSync('xdotool', ['search', '--sync', '--onlyvisible', '--pid', String(instance.pid), '--classname', '.'], {
    encoding: 'utf8',
  })
    .trim()
    .split('\n');
  execFileSync('xdotool', ['windowminimize', '--sync', id]);
  return () => execFileSync('xdotool', ['windowactivate', '--sync', id]);
}

export async function run({ launch, restart, log, topic }) {
  let [host, first, second] = await launch('host', 'first', 'second');
  const all = () => [host, first, second];
  await Promise.all(all().map(accept));
  await connect(all());
  await Promise.all(all().map((instance) => openTopic(instance, topic, 'timeline')));
  await Promise.all(all().map((instance) => waitForPeers(instance, topic, 2)));
  await injectCapture(host);
  await Promise.all([first, second].map(observeDraws));
  const ids = [];
  for (const viewer of [first, second]) {
    const status = await viewer.invoke('get_sync_status');
    ids.push(status.discovery.local_endpoint_id, status.local_author_pubkey);
  }
  await createSession(host, 'live-check');
  await Promise.all([first, second].map(selectLive));

  // LV-1: 配信の開始の前に参加すると「配信者はまだ映像を送っていません」。
  await click(first, JOIN, { within: CARD, timeout: 60_000 });
  await eventually('the first viewer to show the no-media state', async () => (await statusOf(first)).includes(t('media.states.no_media')), 30_000);
  log('LV-1: a viewer who joins before the broadcast sees that no video is sent yet');

  const heard = audioMode ? record(first) : null;
  let voice = audioMode ? speak('host', VOICE) : () => undefined;
  const failures = [];
  let unit = null; // V1 の出力に出た配信者の音の大きさ。
  /// `recorder` の出力に音が出るのを待ち、落ち着いてから測った大きさを返す(音声の試験をしない実行では null)。
  const expectSound = async (what, recorder = heard) => {
    if (!recorder) return null;
    await eventually(`to hear ${what}`, () => recorder.level(WINDOW_MS) > (unit ? 0.3 * unit : 1e-3), 30_000);
    await sleep(WINDOW_MS);
    return recorder.level(WINDOW_MS);
  };
  /// `recorder` の出力が `ms` 以内に無音になるのを待ち、かかった時間を返す。
  const expectSilence = async (what, ms, recorder = heard) => {
    if (!recorder) return null;
    const since = Date.now();
    while (recorder.level(PERIOD_MS) >= (unit ?? 0.05) * 0.002) {
      if (Date.now() - since > ms) throw new Error(`still hearing ${what} after ${ms} ms`);
      await sleep(100);
    }
    return Date.now() - since;
  };
  /// `viewer` の canvas に描かれ続けているか(3 秒の間に `minimum` 枚以上。camera は 1 秒あたり 5 枚。画面は動かなければ
  /// 1 秒ごとに送り直すだけなので 2 枚)。描いた枚数を返す。
  const expectDrawing = async (viewer, what, minimum = 15) => {
    const since = Date.now();
    await sleep(3_000);
    const draws = (await drawsSince(viewer, since)).length;
    assert.ok(draws >= minimum, `${viewer.name} draws ${what} (${draws} frames in 3 s)`);
    return draws;
  };
  /// `viewer` の canvas に、直近 1 秒に描かれたか。
  const drawing = async (viewer) => (await drawsSince(viewer, Date.now() - 1_000)).length > 0;
  try {
    // LV-2: camera で配信すると、視聴者の canvas に frame が描かれ続け、視聴者の出力に配信者の音が出る。
    await startBroadcast(host);
    await eventually('the host to broadcast', async () => (await statusOf(host)).includes(t('media.broadcasting')), 30_000);
    await eventually('the first viewer to draw the camera', async () => (await drawsSince(first, Date.now() - 1_000)).some(isCamera), 30_000);
    const frames = await expectDrawing(first, 'the camera');
    unit = await expectSound('the host');
    log(`LV-2: the viewer draws the camera (${frames} frames in 3 s)${unit === null ? '' : ` and hears the host as ${unit.toFixed(4)}`}`);

    await overlayImages(host, [first, second], topic);
    await expectDrawing(first, 'the camera under PNG/GIF overlays');
    await expectSound('the host under PNG/GIF overlays');
    log('LV-2 images: both viewers display the first PNG/GIF from the stored original bytes while video/audio continue');

    // LV-11(#1839 AC-3): H が live の列を閉じても、配信は続き、視聴者に描かれて音が出る。
    await closeLiveColumn(host);
    const awayFrames = await expectDrawing(first, 'the camera while the host has closed the live column');
    await expectSound('the host while the live column is closed');
    log(`LV-11: the broadcast goes on after the host closes the live column (${awayFrames} frames in 3 s)`);
    await reopenLiveColumn(host, topic);

    // LV-3: 視聴者が描いた frame と出た音の時刻の差が、映像の遅れ 45 ms・進み 150 ms の範囲に入る。
    if (isolated) {
      voice();
      try {
        const offsets = await measureSync(host, first, heard);
        log(`LV-3: the video is behind the sound by ${offsets.map(Math.round).join(', ')} ms`);
        assert.ok(inSync(offsets), 'the video is in sync with the sound');
      } catch (error) {
        // 他の項目は続けて確かめ、最後に失敗にする。
        failures.push(error);
      }
      voice = speak('host', VOICE);
      await expectSound('the host after the timed sounds');
    }

    // LV-4: H が mic を off にすると音だけが止まり、描画は続く。on で音が戻る。
    if (heard) {
      await click(host, t('media.disableMicrophone'), { within: BAR });
      const silentAfter = await expectSilence('the muted host', 5_000);
      await expectDrawing(first, 'the camera while the microphone is off');
      await click(host, t('media.enableMicrophone'), { within: BAR });
      await expectSound('the host again');
      log(`LV-4: the microphone off silences the viewer in ${silentAfter} ms while the video goes on, and on brings the sound back`);
    }

    // LV-12: stageと投稿から開いた詳細モーダルの2か所でも視聴は1つで、音は2重にならない。
    // stageを閉じても詳細で続き、音量0で無音になり、戻すと音が戻る。
    await openLiveDetails(first);
    await eventually('the first viewer to draw in both places', async () => {
      const draws = await drawsSince(first, Date.now() - 2_000);
      return draws.some((each) => each.details) && draws.some((each) => !each.details);
    }, 30_000);
    const bothLevel = await expectSound('the host while shown twice');
    if (unit !== null) assert.ok(bothLevel < 1.5 * unit, `one sound for two views (${bothLevel} vs ${unit})`);
    await closeLiveColumn(first);
    await expectDrawing(first, 'the camera in the post details after closing the live column');
    const setVolume = (value) =>
      first.evaluate(`(() => {
        const input = document.querySelector('${DETAILS} input[type="range"]');
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(String(value))});
        input.dispatchEvent(new Event('input', { bubbles: true }));
        return true;
      })()`);
    let mutedIn = null;
    if (heard) {
      await setVolume(0);
      mutedIn = await expectSilence('the host at volume 0', 5_000);
      await expectDrawing(first, 'the camera at volume 0');
      await setVolume(100);
      await expectSound('the host at full volume again');
    }
    log(`LV-12: two views share one watch${bothLevel === null ? '' : ` (sound ${bothLevel.toFixed(4)} vs ${unit.toFixed(4)})`}, the post details keeps drawing after the live column closes${mutedIn === null ? '' : `, volume 0 silences in ${mutedIn} ms`}`);

    // LV-13: 詳細から再び「大きく表示」しても視聴は1つで、stageと詳細の両方に描かれる。
    // stageを閉じた後も、元の投稿から開いた詳細で描画が続く。
    await click(first, t('actions.stage'), { within: DETAILS });
    await openLiveDetails(first);
    await eventually('the first viewer to draw on the stage and in the post details', async () => {
      const draws = await drawsSince(first, Date.now() - 2_000);
      return draws.some((each) => each.stage) && draws.some((each) => each.details);
    }, 30_000);
    const stagedLevel = await expectSound('the host while shown on the stage and in the post details');
    if (unit !== null) assert.ok(stagedLevel < 1.5 * unit, `one sound for the stage and the post details (${stagedLevel} vs ${unit})`);
    await closeLiveColumn(first);
    await expectDrawing(first, 'the camera in the post details after closing the staged live column');
    await reopenLiveColumn(first, topic);
    log(`LV-13: the stage and the post details share one watch${stagedLevel === null ? '' : ` (sound ${stagedLevel.toFixed(4)} vs ${unit.toFixed(4)})`}, the post details keeps drawing after the staged column closes`);

    // LV-6: 途中で参加した視聴者に、2 秒以内に最初の frame が描かれる。
    const joinedAt = Date.now();
    await click(second, JOIN, { within: CARD, timeout: 60_000 });
    const firstDraw = await eventually('the second viewer to draw the first frame', async () => (await drawsSince(second, joinedAt))[0], 10_000);
    const delay = Math.round(firstDraw.at - joinedAt);
    log(`LV-6: the viewer who joined later drew the first frame ${delay} ms after joining`);
    assert.ok(delay <= 2_000, 'the first frame within 2 seconds');

    // LV-7: 視聴者が退出すると描画と音が止まる。
    const secondHeard = audioMode ? record(second) : null;
    await expectSound('the host on the second viewer', secondHeard);
    await click(second, text('common', 'actions.leave'), { within: CARD });
    await expectSilence('the host after leaving', 5_000, secondHeard);
    await eventually('the viewer who left to stop drawing', async () => !(await drawing(second)), 5_000);
    secondHeard?.stop();
    log('LV-7: the viewer who left stops drawing and hearing');

    // LV-5: 画面で配信すると視聴者に描かれる。H の window を他の window の後ろに回しても止まらない(Linux は最小化した
    // ときを記録する。frame を表示中の <video> から取るため)。
    await click(host, t('media.sources.screen'), { within: BAR });
    const screen = await eventually('the first viewer to draw the screen', async () => (await drawsSince(first, Date.now() - 1_000)).find(isScreen)?.size, 30_000);
    await expectDrawing(first, 'the screen', 2);
    first.focus();
    await expectDrawing(first, 'the screen while the host is behind another window', 2);
    let whileMinimized = 'not checked';
    if (process.platform !== 'win32') {
      const restore = minimize(host);
      const since = Date.now();
      await sleep(3_000);
      whileMinimized = `${(await drawsSince(first, since)).length} frame(s) in 3 s`;
      restore();
    }
    log(`LV-5: the viewer draws the screen (${screen}), also behind another window; while minimized: ${whileMinimized}`);
    await click(host, t('media.sources.camera'), { within: BAR });
    await eventually('the first viewer to draw the camera again', async () => (await drawsSince(first, Date.now() - 1_000)).some(isCamera), 30_000);

    // LV-5: 画面の選択の取り消しで配信が止まり、理由が出る。
    await host.evaluate('(window.__capture.deny = true, true)');
    await click(host, t('media.sources.screen'), { within: BAR });
    await eventually('the host to show why the broadcast stopped', async () => (await statusOf(host)).includes(t('media.errors.denied')), 30_000);
    const deniedAt = Date.now();
    await eventually('the first viewer to stop drawing', async () => !(await drawing(first)), 10_000);
    log(`LV-5: cancelling the screen selection stops the broadcast with the reason (the viewer stopped in ${Date.now() - deniedAt} ms)`);
    await host.evaluate('(window.__capture.deny = false, true)');
    await click(host, text('common', 'actions.close'), { within: BAR });
    await startBroadcast(host);
    await eventually('the first viewer to draw after starting again', () => drawing(first), 30_000);
    await expectSound('the host after starting again');

    // LV-7: 配信の停止で、視聴者の描画と音が 5 秒以内に止まる。
    await click(host, t('media.stopBroadcast'), { within: BAR });
    const stoppedAt = Date.now();
    await expectSilence('the host after stopping', 5_000);
    await eventually('the viewer to stop drawing', async () => !(await drawing(first)), 5_000);
    log(`LV-7: the viewer stopped within ${Date.now() - stoppedAt} ms after the broadcast stopped`);

    // LV-8: 配信者の app を起動し直しても、配信は自動では始まらない。
    host = await restart(host);
    await accept(host);
    await reopenLiveColumn(host, topic);
    await waitForPeers(host, topic, 2);
    await eventually('the host to show the stage', () => host.evaluate(`Boolean(document.querySelector('${CARD}'))`), 30_000);
    await sleep(5_000);
    assert.ok(!(await statusOf(host)).includes(t('media.broadcasting')), 'the host does not broadcast after a restart');
    assert.ok(!(await drawing(first)), 'the first viewer draws nothing');
    log('LV-8: the broadcast does not start again after a restart');

    // LV-7: session の終了の後は参加できない。
    await click(host, text('common', 'actions.end'), { within: CARD });
    await eventually('the viewer who left to see the session end', () =>
      second.evaluate(`Boolean(document.querySelector('${CARD}')?.textContent.includes(${JSON.stringify(t('stage.ended'))})) && ![...document.querySelectorAll('${CARD} button')].some((node) => (node.getAttribute('aria-label') ?? node.textContent).trim() === ${JSON.stringify(JOIN)} && !node.disabled)`), 30_000);
    log('LV-7: nobody can join after the end');

    // LV-9: log の経路は direct_p2p。視聴者の識別子は media の log に出ない。
    const established = all().flatMap((instance) => logOf(instance).split('\n').filter((line) => line.includes('media connection established')));
    assert.ok(established.length > 0 && established.every((line) => line.includes('path="direct_p2p"')), `media over direct_p2p: ${established.join('\n')}`);
    const mediaLines = all().flatMap((instance) => logOf(instance).split('\n').filter((line) => /media|live/i.test(line)));
    for (const id of ids) assert.ok(!mediaLines.some((line) => line.includes(id)), 'no viewer identifiers in the media logs');
    log(`LV-9: ${established.length} media connection(s) over direct_p2p, no viewer identifiers in the media logs`);
  } finally {
    voice();
    heard?.stop();
  }
  if (failures.length) throw failures[0];
}

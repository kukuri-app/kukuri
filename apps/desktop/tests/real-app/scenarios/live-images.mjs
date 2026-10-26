// #1950 AC-1: 添付の取得を含む3端末の描画。Live単体のログ秘匿検査と入力を混ぜない。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { eventually } from '../../web-e2e/wait.mjs';
import { audioMode, PERIOD_MS, record, speak } from '../audio.mjs';
import { accept, connect, openTopic, waitForPeers } from '../session.mjs';
import { createSession, drawsSince, injectCapture, observeDraws, selectLive, startBroadcast, watch, VOICE } from '../live-media.mjs';

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
      } catch (error) { throw new Error(`${error.message}; overlay=${JSON.stringify(observed)}`, { cause: error }); }
      // asset URLのfetchはCSPのconnect-src外。通常APIで取得済みblobを照合し、表示URLを読み直さない。
      const payload = await viewer.invoke('get_blob_media_payload', { request: { hash: saved.attachments[0].hash, mime: attachment.mime, source_object_id: id } });
      assert.equal(payload.bytes_base64, attachment.data_base64, `${viewer.name} preserves ${kind} bytes through storage and fetch`);
    }
  }
}

export async function run({ launch, log, topic }) {
  const all = await launch('host', 'first', 'second');
  const [host, first, second] = all;
  await Promise.all(all.map(accept));
  await connect(all);
  await Promise.all(all.map((instance) => openTopic(instance, topic, 'timeline')));
  await Promise.all(all.map((instance) => waitForPeers(instance, topic, 2)));
  await injectCapture(host);
  await Promise.all([first, second].map(observeDraws));
  await createSession(host, 'overlay-images');
  await Promise.all([first, second].map(selectLive));
  await startBroadcast(host);
  await Promise.all([first, second].map(watch));
  const heard = audioMode ? record(first) : null;
  const stop = audioMode ? speak('host', VOICE) : () => {};
  try {
    if (heard) await eventually('to hear the host before overlays', () => heard.level(5 * PERIOD_MS) > 1e-3, 30000);
    const baseline = heard?.level(5 * PERIOD_MS) ?? 0;
    await overlayImages(host, [first, second], topic);
    const since = Date.now();
    await new Promise((resolve) => setTimeout(resolve, 3000));
    for (const viewer of [first, second]) assert.ok((await drawsSince(viewer, since)).length >= 15, `${viewer.name} keeps drawing under overlays`);
    if (heard) assert.ok(heard.level(5 * PERIOD_MS) > baseline * 0.3, 'audio continues under overlays');
    log('both viewers display the first PNG/GIF with original bytes while video/audio continue');
  } finally { stop(); heard?.stop(); }
}

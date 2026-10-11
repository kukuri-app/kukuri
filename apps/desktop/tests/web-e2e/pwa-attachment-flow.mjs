import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { eventually } from './wait.mjs';
import { webInvoke } from './web-invoke.mjs';
import { installPwa } from './pwa-entry.mjs';

export async function pwaAttachmentFlow({ openClient, greetNative, native, fixture, nativeCreatesChannel, joinChannel,
  publicColumn, openComposer, replaceValue, topic, origin, run, browserName, adb }) {
  const browser = await openClient('pwa-attachment', { ice: true });
  const hello = await greetNative(browser, 'pwa-attachment');
  if (browserName === 'android') await installPwa(browser, adb);
  const { pubkey: peer } = await fixture('/fixture/info');
  await native('follow_author', { request: { pubkey: hello.webPost.author_pubkey } });
  await webInvoke(browser, 'follow_author', { request: { pubkey: peer } });
  const channel = await nativeCreatesChannel(`attachment-${run}`);
  await joinChannel(browser, channel.token, `attachment-${run}`);
  const png = Buffer.from(await (await fetch(`${origin}/fixture/payload.png`)).arrayBuffer()).toString('base64');
  const video = (await readFile(new URL('../../../../crates/cn-safety-video/src/probes/benign.mp4', import.meta.url))).toString('base64');
  await browser.execute(() => {
    window.__attachmentMaxChunk = 0;
    window.__attachmentChunkReads = 0;
    const slice = Blob.prototype.slice;
    Blob.prototype.slice = function (start, end, type) {
      window.__attachmentMaxChunk = Math.max(window.__attachmentMaxChunk, Math.min(end, this.size) - start);
      window.__attachmentChunkReads++;
      return slice.call(this, start, end, type);
    };
    FileReader.prototype.readAsDataURL = function () { throw new Error('Web original must not become a full data URL'); };
  });
  // まず実UIのFile選択→共通JS invoke→WASM→保存を通す。fixtureのDataTransferだけを作る。
  const text = `attachment-ui-${run}`;
  const field = await openComposer(publicColumn(browser));
  await replaceValue(browser, field, text);
  await browser.execute(encoded => {
    const data = new DataTransfer();
    data.items.add(new File([Uint8Array.from(atob(encoded), c => c.charCodeAt(0))], 'original.png', { type: 'image/png' }));
    const input = document.querySelector('.shell-column-composer input[type=file]');
    input.files = data.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, png);
  await publicColumn(browser).$('img[alt="draft preview original.png"]').waitForDisplayed();
  await publicColumn(browser).$('button=Post').click();
  const uiPost = await eventually('the original File reaches the actual post', async () =>
    (await webInvoke(browser, 'list_timeline', { request: { topic, scope: { kind: 'public' }, cursor: null, limit: 50 } })).items.find(item => item.content === text));
  const uiPayload = await webInvoke(browser, 'get_blob_media_payload', { request: {
    hash: uiPost.attachments[0].hash, mime: 'image/png', source_object_id: uiPost.object_id,
  } });
  assert.equal(uiPayload.bytes_base64, png);

  // 各共有Rust callerの原本/posterとprivate/DM境界。入力bytesは試験fixtureだけがbase64から作る。
  const call = (command, request, parts, slot = 'attachments', mode = null, target = null) => browser.execute(async (command, request, parts, slot, mode, target) => {
    const url = performance.getEntriesByType('resource').find(item => item.name.includes('/kukuri_web_runtime-') && item.name.endsWith('.js')).name;
    const module = await import(url);
    const sources = [];
    const uploads = [];
    for (const part of parts) {
      const blob = new Blob([Uint8Array.from(atob(part.data), c => c.charCodeAt(0))], { type: part.mime });
      const identity = await module.prepare_attachment(blob);
      const reference = { id: crypto.randomUUID(), ...identity };
      sources.push({ reference, blob });
      uploads.push({ file_name: 'original', mime: part.mime, role: part.role, byte_size: blob.size, data_base64: '', source_ref: reference });
    }
    request[slot] = slot === 'attachments' ? uploads : uploads[0];
    const args = { request };
    window.__attachmentOldArgs = { command, args };
    const read = Blob.prototype.arrayBuffer;
    let count = 0;
    if (mode) Blob.prototype.arrayBuffer = function () {
      if (++count === (mode === 'failure' ? 2 : 1)) {
        if (mode === 'failure') return Promise.reject(new DOMException('mid-read failure', 'NotReadableError'));
        if (mode === 'restart') { window.__attachmentPendingRead = true; return new Promise(() => {}); }
        return read.call(this).then(async bytes => { await module.invoke('switch_account', { request: { account_id: target } }); return bytes; });
      }
      return read.call(this);
    };
    if (mode === 'restart') {
      void module.invoke_with_attachments(command, args, sources).catch(() => undefined);
      return { pending: true, old: window.__attachmentOldArgs };
    }
    try {
      return { value: await module.invoke_with_attachments(command, args, sources), hashes: sources.map(item => item.reference.hash) };
    } catch (error) { return { error }; }
    finally { Blob.prototype.arrayBuffer = read; }
  }, command, request, parts, slot, mode, target);
  const image = { data: png, mime: 'image/png', role: 'image_original' };
  const movie = [{ data: video, mime: 'video/mp4', role: 'video_manifest' }, { ...image, role: 'video_poster' }];
  for (const kind of ['post', 'reply', 'private', 'dm']) {
    for (const [index, parts] of [[image], movie].entries()) {
      const content = `attachment-${kind}-${index}-${run}`;
      const request = kind === 'dm' ? { pubkey: peer, text: content, reply_to_message_id: null } : {
        topic, content, reply_to: kind === 'reply' ? hello.webPost.object_id : null, content_labels: [],
        channel_ref: kind === 'private' ? { kind: 'private_channel', channel_id: channel.channelId } : { kind: 'public' },
      };
      const result = await call(kind === 'dm' ? 'send_direct_message' : 'create_post', request, parts);
      assert.equal(result.error, undefined, JSON.stringify(result.error));
      const page = await webInvoke(browser, kind === 'dm' ? 'list_direct_message_messages' : kind === 'reply' ? 'list_thread' : 'list_timeline', { request: {
        cursor: null, limit: 50, ...(kind === 'dm' ? { pubkey: peer } : kind === 'reply' ? { topic, thread_id: hello.webPost.object_id } : {
          topic, scope: kind === 'private' ? { kind: 'channel', channel_id: channel.channelId } : { kind: 'public' },
        }),
      } });
      const saved = page.items.find(item => (item.content ?? item.text) === content);
      assert.ok(saved, content);
      assert.equal(saved.attachments.length, parts.length);
      for (const item of saved.attachments) {
        const payload = await webInvoke(browser, 'get_blob_media_payload', { request: {
          hash: item.hash, mime: item.mime, source_object_id: kind === 'dm' ? null : saved.object_id,
        } });
        assert.equal(payload.bytes_base64, parts.find(part => part.role === item.role).data);
      }
    }
  }
  const profile = await call('set_my_profile', { name: 'attachment', display_name: 'attachment', about: '', nip05: null, clear_picture: false }, [{ ...image, role: 'profile_avatar' }], 'picture_upload');
  assert.equal(profile.error, undefined, JSON.stringify(profile.error));
  assert.equal(profile.value.picture_asset.hash, profile.hashes[0]);
  const custom = await call('create_custom_reaction_asset', { crop_rect: { x: 0, y: 0, size: 1 }, search_key: `attachment-${run}` }, [image], 'upload');
  assert.equal(custom.error, undefined, JSON.stringify(custom.error));
  const nativeCustom = await native('create_custom_reaction_asset', { request: { crop_rect: { x: 0, y: 0, size: 1 }, search_key: `native-${run}`,
    upload: { file_name: 'original', mime: image.mime, byte_size: Buffer.from(png, 'base64').length, data_base64: png, role: image.role } } });
  assert.equal(custom.value.blob_hash, nativeCustom.blob_hash);
  const failedText = `attachment-failed-${run}`;
  const postRequest = content => ({ topic, content, reply_to: null, content_labels: [], channel_ref: { kind: 'public' } });
  assert.ok((await call('create_post', postRequest(failedText), [image], 'attachments', 'failure')).error);
  const old = await browser.execute(() => window.__attachmentOldArgs);
  await assert.rejects(webInvoke(browser, old.command, old.args));
  const account = await webInvoke(browser, 'list_accounts');
  const operationId = await browser.execute(() => crypto.randomUUID());
  const next = await webInvoke(browser, 'create_account', { request: { account_id: account.active_account_id, operation_id: operationId } });
  const nextId = next.id;
  await webInvoke(browser, 'save_initial_profile', { request: { account_id: nextId, profile: { name: 'second', display_name: 'second', about: '', picture_upload: null, clear_picture: false, nip05: null } } });
  await webInvoke(browser, 'switch_account', { request: { account_id: account.active_account_id } });
  const switchedText = `attachment-switched-${run}`;
  assert.ok((await call('create_post', postRequest(switchedText), [image], 'attachments', 'switch', nextId)).error);
  await webInvoke(browser, 'switch_account', { request: { account_id: account.active_account_id } });
  const interruptedText = `attachment-interrupted-${run}`;
  const interrupted = await call('create_post', postRequest(interruptedText), [image], 'attachments', 'restart');
  await eventually('the interrupted source is reading before reload', () => browser.execute(() => window.__attachmentPendingRead === true));
  const measurement = await browser.execute(() => ({ max: window.__attachmentMaxChunk, reads: window.__attachmentChunkReads }));
  assert.ok(measurement.reads > 0 && measurement.max <= 1024 * 1024, JSON.stringify(measurement));
  await browser.refresh();
  await browser.$('[data-testid="control-center-trigger"]').waitForDisplayed({ timeout: 90_000 });
  await assert.rejects(webInvoke(browser, interrupted.old.command, interrupted.old.args));
  const after = await eventually('the restarted account reads its saved topic', () => webInvoke(browser, 'list_timeline', {
    request: { topic, scope: { kind: 'public' }, cursor: null, limit: 50 },
  }).catch(() => null));
  for (const content of [failedText, switchedText, interruptedText]) assert.ok(!after.items.some(item => item.content === content));
  // 新しい永続stagingは無く、reload後の一時参照は使えない。測定は現在の操作のchunkだけ。
  console.log('PWA attachment UI/raw originals/poster/shared callers/failure/account/restart: PASS');
}

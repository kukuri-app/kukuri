import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { eventually } from './wait.mjs';
import { webInvoke } from './web-invoke.mjs';
import { installPwa } from './pwa-entry.mjs';
import { waitForUnusedShellCache } from './pwa-shell-cache.mjs';

export async function pwaShellFlow({ openClient, greetNative, native, fixture, openDirectMessage,
  nativeCreatesChannel, joinChannel, openSettings, publicColumn, channelColumn, openComposer, replaceValue, openChannelDialog, channelDialogClosed, seesInChannel,
  origin, topic, run, browserName, adb }) {
  if (!['chrome', 'android'].includes(browserName)) throw new Error('PWA shell acceptance runs on Chrome');
  const browser = await openClient('pwa-shell', { ice: true });
  const hello = await greetNative(browser, 'pwa-shell');
  const oldTab = browserName === 'android' ? await installPwa(browser, adb) : null;
  const enter = async (field, text) => {
    if (browserName !== 'android') return replaceValue(browser, field, text);
    assert.equal(process.env.CI, 'true');
    assert.match(text, /^[a-zA-Z0-9 _-]+$/, 'this scenario uses ASCII fixture text for Android input');
    await field.click();
    await adb('shell', 'input', 'keycombination', 'KEYCODE_CTRL_LEFT', 'KEYCODE_A');
    await adb('shell', 'input', 'keyevent', 'KEYCODE_DEL');
    await adb('shell', 'input', 'text', text.replaceAll(' ', '%s'));
    await eventually('Android input reaches the PWA field', async () => await field.getProperty('value') === text);
  };
  const account = await webInvoke(browser, 'list_accounts');
  const settings = await webInvoke(browser, 'set_adult_content_display_enabled', { enabled: true });
  const assertAccount = async () => {
    const after = await webInvoke(browser, 'list_accounts');
    assert.equal(after.active_account_id, account.active_account_id);
    assert.equal(after.accounts.length, account.accounts.length);
    assert.equal(after.accounts[0].pubkey, account.accounts[0].pubkey);
    assert.deepEqual(await webInvoke(browser, 'get_content_display_settings'), settings);
  };
  const pubkey = hello.webPost.author_pubkey;
  const { pubkey: peer } = await fixture('/fixture/info');
  await native('follow_author', { request: { pubkey } });
  await browser.$(`article*=${hello.fromNative}`).waitForExist({ timeout: 90_000 });
  await openDirectMessage(browser, hello.fromNative, peer);
  const channel = await nativeCreatesChannel(`offline-${run}`);
  await joinChannel(browser, channel.token, `offline-${run}`);
  await eventually('shell controls the real application', () => browser.execute(() => navigator.serviceWorker.controller !== null));
  const build = () => browser.execute(() => document.querySelector('meta[name="kukuri-shell-build"]')?.content).catch(error => {
    if (error.message.includes('Cannot find context with specified id')) return null;
    throw error;
  });
  const firstBuild = await build();
  assert.ok(firstBuild);
  const png = Buffer.from(await (await fetch(`${origin}/fixture/payload.png`)).arrayBuffer());
  const video = await readFile(new URL('../../../../crates/cn-safety-video/src/probes/benign.mp4', import.meta.url));
  const attachment = (bytes, mime, role) => ({ file_name: `offline.${mime === 'image/png' ? 'png' : 'mp4'}`,
    mime, role, byte_size: bytes.length, data_base64: bytes.toString('base64') });
  const media = [[], [attachment(png, 'image/png', 'image_original')],
    [attachment(video, 'video/mp4', 'video_manifest'), attachment(png, 'image/png', 'video_poster')]];
  const network = offline => browser.setNetworkConditions({ offline, latency: 0, download_throughput: -1, upload_throughput: -1 });
  const accepted = [];
  const page = (invoke, kind) => invoke(kind === 'dm' ? 'list_direct_message_messages' : kind === 'reply' ? 'list_thread' : 'list_timeline', {
    request: { cursor: null, limit: 50, ...(kind === 'dm' ? { pubkey: peer } : kind === 'reply'
      ? { topic, thread_id: hello.webPost.object_id } : { topic,
        scope: kind === 'private' ? { kind: 'channel', channel_id: channel.channelId } : { kind: 'public' } }) },
  });
  const invoke = (command, args) => webInvoke(browser, command, args);
  const assertStored = async () => {
    for (const item of accepted) {
      const stored = (await page(invoke, item.kind)).items.find(value => (value.object_id ?? value.message_id) === item.id);
      assert.equal(stored?.content ?? stored?.text, item.text);
      if (item.kind === 'dm') assert.equal(stored.delivered, false);
      assert.equal(stored.attachments.length, media[item.index].length);
      for (const attachment of stored.attachments) {
        assert.notEqual(attachment.status, 'Missing');
        const payload = await invoke('get_blob_media_payload', { request: { hash: attachment.hash, mime: attachment.mime, source_object_id: item.kind === 'dm' ? null : item.id } });
        assert.equal(payload?.bytes_base64, media[item.index].find(value => value.role === attachment.role).data_base64);
      }
    }
  };
  try {
    await network(true);
    await eventually('the browser reports offline', () => browser.execute(() => !navigator.onLine));
    const failedText = `unaccepted storage failure ${run}`;
    const field = await openComposer(publicColumn(browser));
    await enter(field, failedText);
    await browser.execute(() => {
      const put = IDBObjectStore.prototype.put;
      window.__kukuriRestorePut = () => { IDBObjectStore.prototype.put = put; };
      IDBObjectStore.prototype.put = function (...args) {
        if (this.name === 'chunks') throw new DOMException('Injected storage failure', 'QuotaExceededError');
        return put.apply(this, args);
      };
    });
    try {
      await publicColumn(browser).$('button=Post').click();
      await publicColumn(browser).$('form [role="alert"]').waitForDisplayed();
      assert.equal(await field.getProperty('value'), failedText);
      assert.equal((await page(invoke, 'post')).items.some(item => item.content === failedText), false);
    } finally { await browser.execute(() => { window.__kukuriRestorePut(); delete window.__kukuriRestorePut; }); }
    // 操作の入口は実UI、媒体の組合せは実WASM commandの同じ受付境界で照合する。
    const uiPost = `UI offline post ${run}`;
    await enter(await openComposer(publicColumn(browser)), uiPost);
    await publicColumn(browser).$('button=Post').click();
    await browser.$(`article*=${uiPost}`).waitForDisplayed();
    const uiMessage = `UI offline DM ${run}`;
    const conversation = browser.$(`section[data-column-id^="column:conversation:"][data-column-id$="${peer}"]`);
    const messageField = conversation.$('textarea[placeholder="Write a message"]');
    if (!(await messageField.isDisplayed())) await conversation.$('button[aria-label^="Message to "]').click();
    await enter(messageField, uiMessage);
    await conversation.$('button=Send').click();
    await browser.$(`article*=${uiMessage}`).waitForDisplayed();
    for (const [index, attachments] of media.entries()) {
      for (const kind of ['post', 'reply', 'private', 'dm']) {
        const text = `offline ${kind} media-${index} ${run}`;
        const command = kind === 'dm' ? 'send_direct_message' : 'create_post';
        const request = kind === 'dm' ? { pubkey: peer, text, reply_to_message_id: null, attachments } : {
          topic, content: text, reply_to: kind === 'reply' ? hello.webPost.object_id : null,
          ...(kind === 'private' ? { channel_ref: { kind: 'private_channel', channel_id: channel.channelId } } : {}), attachments,
        };
        const id = await webInvoke(browser, command, { request });
        assert.equal(typeof id, 'string');
        accepted.push({ kind, text, id, index });
      }
    }
    await assertStored();
    const draft = `retained offline draft ${run}`;
    await enter(await openComposer(publicColumn(browser)), draft);
    const messageDraft = `retained message draft ${run}`;
    if (!(await messageField.isDisplayed())) await conversation.$('button[aria-label^="Message to "]').click();
    await enter(messageField, messageDraft);
    await browser.$(`article*=${hello.webPost.content}`).$('button[aria-label="Reply"]').click();
    const replyDraft = `retained reply draft ${run}`;
    await enter(browser.$('textarea[placeholder="Write a reply"]'), replyDraft);
    const assertDrafts = async () => {
      const saved = await browser.execute(() => localStorage.getItem('kukuri:column-drafts:v1'));
      for (const text of [draft, messageDraft, replyDraft]) assert.ok(saved.includes(text));
    };
    await browser.refresh();
    await browser.$('[data-testid="control-center-trigger"]').waitForDisplayed({ timeout: 90_000 });
    await assertAccount();
    assert.equal(await (await openComposer(publicColumn(browser))).getProperty('value'), draft);
    await assertDrafts();
    await assertStored();
    // 同originの2つ目の実buildを準備。延期しても現在の下書きと画面を保つ。
    await fixture('/fixture/shell-build', {});
    await network(false);
    await openSettings(browser, 'release');
    await browser.$('button=Check for updates').waitForClickable({ timeout: 90_000 });
    await browser.$('button=Check for updates').click();
    await browser.$('button=Save drafts and update').waitForDisplayed({ timeout: 90_000 });
    assert.equal(await build(), firstBuild, 'the prepared update is deferred');
    await browser.$('button=Save drafts and update').click();
    await eventually('the explicit update loads the next build', async () => {
      const current = await build();
      return current && current !== firstBuild;
    }, 90_000);
    await browser.$('[data-testid="control-center-trigger"]').waitForDisplayed({ timeout: 90_000 });
    await assertAccount();
    assert.equal(await (await openComposer(publicColumn(browser))).getProperty('value'), draft);
    await assertDrafts();
    if (oldTab && oldTab !== await browser.getWindowHandle()) {
      assert.equal(await browser.execute(async () => (await caches.keys()).filter(name => name.startsWith('kukuri-shell-')).length), 2, 'the original live tab keeps its prior build');
      const pwa = await browser.getWindowHandle();
      await browser.switchToWindow(oldTab); await browser.closeWindow(); await browser.switchToWindow(pwa);
      assert.equal(await browser.execute(() => matchMedia('(display-mode: standalone)').matches), true);
      await openSettings(browser, 'release');
      await browser.$('button=Check for updates').waitForClickable({ timeout: 90_000 });
      await browser.$('button=Check for updates').click();
    }
    await waitForUnusedShellCache(browser);
    // 一時Columnは再起動で閉じる。保存済みの所属からchannelを開き、既存の有効需要を戻す。
    const closeSettings = browser.$('#shell-settings-drawer .shell-settings-close');
    if (await closeSettings.isDisplayed()) await closeSettings.click();
    const channels = await openChannelDialog(browser);
    const open = channels.$(`button[aria-label="Open offline-${run}"]`);
    await open.waitForClickable({ timeout: 90_000 });
    await open.click();
    await channelDialogClosed(browser);
    await openComposer(channelColumn(browser, channel.channelId));
    await seesInChannel(browser, channel.channelId, accepted.find(item => item.kind === 'private').text);
    for (const item of accepted) {
      if (item.kind === 'dm') {
        await eventually('the same queued DM is delivered', async () => {
          const received = await native('list_direct_message_messages', { request: { pubkey, cursor: null, limit: 50 } });
          const copies = received.items.filter(message => message.text === item.text);
          assert.ok(copies.length <= 1);
          return copies.length === 1 && copies[0].message_id === item.id;
        });
      } else await eventually(`the same ${item.kind} is delivered`, async () => {
        const copies = (await page(native, item.kind)).items.filter(post => post.content === item.text);
        assert.ok(copies.length <= 1);
        return copies.length === 1 && copies[0].object_id === item.id;
      });
    }
    await eventually('the durable DM outbox receives the ACK', async () => {
      const messages = (await page(invoke, 'dm')).items;
      return accepted.filter(item => item.kind === 'dm').every(item => messages.some(message => message.message_id === item.id && message.delivered));
    });
    console.log('PWA real WASM offline/restart/update/receipt and same-ID delivery: PASS');
  } finally { await network(false); }
}

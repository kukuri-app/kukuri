import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { eventually } from './wait.mjs';
import { webInvoke } from './web-invoke.mjs';

export async function pwaShellFlow({ openClient, greetNative, native, fixture, openDirectMessage,
  nativeCreatesChannel, joinChannel, openSettings, publicColumn, openComposer, post, sendDirectMessage,
  nativeSees, nativeSeesInChannel, origin, topic, run, browserName }) {
  if (!['chrome', 'android'].includes(browserName)) throw new Error('PWA shell acceptance runs on Chrome');
  const browser = await openClient('pwa-shell', { ice: true });
  const hello = await greetNative(browser, 'pwa-shell');
  const account = await webInvoke(browser, 'list_accounts');
  const assertAccount = async () => {
    const after = await webInvoke(browser, 'list_accounts');
    assert.equal(after.active_account_id, account.active_account_id);
    assert.equal(after.accounts.length, account.accounts.length);
    assert.equal(after.accounts[0].pubkey, account.accounts[0].pubkey);
  };
  const pubkey = hello.webPost.author_pubkey;
  const { pubkey: peer } = await fixture('/fixture/info');
  await native('follow_author', { request: { pubkey } });
  await openDirectMessage(browser, hello.fromNative, peer);
  const channel = await nativeCreatesChannel(`offline-${run}`);
  await joinChannel(browser, channel.token, `offline-${run}`);
  await eventually('shell controls the real application', () => browser.execute(() => navigator.serviceWorker.controller !== null));
  const build = () => browser.execute(() => document.querySelector('meta[name="kukuri-shell-build"]').content);
  const firstBuild = await build();
  const png = Buffer.from(await (await fetch(`${origin}/fixture/payload.png`)).arrayBuffer());
  const video = await readFile(new URL('../../../../crates/cn-safety-video/src/probes/benign.mp4', import.meta.url));
  const attachment = (bytes, mime, role) => ({ file_name: `offline.${mime === 'image/png' ? 'png' : 'mp4'}`,
    mime, role, byte_size: bytes.length, data_base64: bytes.toString('base64') });
  const media = [[], [attachment(png, 'image/png', 'image_original')],
    [attachment(video, 'video/mp4', 'video_manifest'), attachment(png, 'image/png', 'video_poster')]];
  const network = offline => browser.setNetworkConditions({ offline, latency: 0, download_throughput: -1, upload_throughput: -1 });
  const accepted = [];
  try {
    await network(true);
    await eventually('the browser reports offline', () => browser.execute(() => !navigator.onLine));
    // 操作の入口は実UI、媒体の組合せは実WASM commandの同じ受付境界で照合する。
    const uiPost = `UI offline post ${run}`;
    await post(browser, uiPost);
    const uiMessage = `UI offline DM ${run}`;
    await sendDirectMessage(browser, peer, uiMessage);
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
        accepted.push({ kind, text, id });
      }
    }
    const draft = `retained offline draft ${run}`;
    await (await openComposer(publicColumn(browser))).setValue(draft);
    await browser.refresh();
    await browser.$('[data-testid="control-center-trigger"]').waitForDisplayed({ timeout: 90_000 });
    await assertAccount();
    assert.equal(await (await openComposer(publicColumn(browser))).getValue(), draft);
    const pending = await webInvoke(browser, 'list_direct_message_messages', { request: { pubkey: peer, cursor: null, limit: 50 } });
    for (const item of accepted.filter(item => item.kind === 'dm')) {
      const restored = pending.items.find(message => message.message_id === item.id);
      assert.equal(restored?.text, item.text);
      assert.equal(restored?.delivered, false);
    }
    // 同originの2つ目の実buildを準備。延期しても現在の下書きと画面を保つ。
    await fixture('/fixture/shell-build', {});
    await network(false);
    await openSettings(browser, 'release');
    await browser.$('button=Check for updates').waitForClickable({ timeout: 90_000 });
    await browser.$('button=Check for updates').click();
    await browser.$('button=Save drafts and update').waitForDisplayed({ timeout: 90_000 });
    assert.equal(await build(), firstBuild, 'the prepared update is deferred');
    await browser.$('button=Save drafts and update').click();
    await eventually('the explicit update loads the next build', async () => (await build()) !== firstBuild, 90_000);
    await browser.$('[data-testid="control-center-trigger"]').waitForDisplayed({ timeout: 90_000 });
    await assertAccount();
    assert.equal(await (await openComposer(publicColumn(browser))).getValue(), draft);
    await eventually('unused prior shell cache is reclaimed', () => browser.execute(async () =>
      (await caches.keys()).filter(name => name.startsWith('kukuri-shell-')).length === 1));
    for (const item of accepted) {
      if (item.kind === 'dm') {
        await eventually('the same queued DM is delivered', async () => {
          const received = await native('list_direct_message_messages', { request: { pubkey, cursor: null, limit: 50 } });
          const copies = received.items.filter(message => message.text === item.text);
          assert.ok(copies.length <= 1);
          return copies.length === 1 && copies[0].message_id === item.id;
        });
      } else if (item.kind === 'private') await nativeSeesInChannel(channel.channelId, item.text);
      else await nativeSees(item.text, post => post.object_id === item.id);
    }
    console.log('PWA real WASM offline/restart/update/receipt and same-ID delivery: PASS');
  } finally { await network(false); }
}

// Web クライアントの主要導線を実ブラウザで試す（#1220 W8 AC-2a・AC-2b・AC-2g、ADR 0060 §4）。`cargo xtask web-e2e` から呼ぶ。
// 相手は harness の web_e2e_fixture（同じ job の Community Node・native の相手・`dist-web` の配信）。
//
// - 直接経路: 通常の Chrome。native↔Web と Web↔Web の実データが WebRTC DataChannel の上の QUIC を通る。
// - relay fallback: 交換する SDP から ICE の候補を除いた Chrome（ICE が成立しない。W10 の T2 にあたる）。実データが relay
//   を通る。
// 実データの経路は、受け手が画像（乱数の画素の PNG）を取得する間に relay が中継した bytes で判定する。ブラウザには
// relay と WebRTC の他の経路が無いので、relay の中継が画像より十分小さければ、画像は WebRTC を通っている。

import assert from 'node:assert/strict';
import { writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { remote } from 'webdriverio';

const ORIGIN = process.env.KUKURI_WEB_E2E_ORIGIN ?? 'http://127.0.0.1:4180';
const TOPIC = 'kukuri:topic:general';
const topicId = (name) => `kukuri:topic:${name}`;
const RUN = Date.now().toString(36);
const WAIT = 90_000;
// 開いたままの profile で相手の follow し返しを待つ期限。相手の follow の offer は、送れなかったら 2〜64 秒後に 6 回
// （間隔の計 126 秒）送り直され（#1521 AC-1a）、各回は宛先の探索と送信で最長 10 秒ほどかかるので、それを覆う。
const FOLLOW_BACK_WAIT = 210_000;

async function fixture(route, body) {
  const response = await fetch(`${ORIGIN}${route}`, body === undefined ? undefined : {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  const value = await response.json();
  assert.ok(response.ok, `${route}: ${JSON.stringify(value)}`);
  return value;
}

const native = (command, args = {}) => fixture('/fixture/invoke', { command, args });
const relayedBytes = async () => (await fixture('/fixture/relay-bytes')).relayed_bytes;

async function eventually(what, check, timeout = WAIT) {
  const deadline = Date.now() + timeout;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

async function nativeTimeline(topic = TOPIC) {
  const page = await native('list_timeline', { request: { topic, scope: { kind: 'public' }, limit: 50 } });
  return page.items;
}

const nativeSees = (what, predicate, topic = TOPIC) =>
  eventually(`native sees ${what}`, async () => (await nativeTimeline(topic)).find(predicate));

/** native の相手は、desktop の UI と同じく topic の timeline の列を表示している（需要の購読）。 */
const nativeShows = (topic) =>
  native('set_scope_display', {
    request: { observer: `web-e2e-native-${topic}`, target: { kind: 'timeline', topic, scope: { kind: 'public' } }, visible: true },
  });

const nativeReact = (target, emoji) =>
  native('toggle_reaction', {
    request: { target_topic_id: TOPIC, target_object_id: target.object_id, reaction_key: { kind: 'emoji', emoji }, channel_ref: null },
  });

/** 新しい profile の Chrome を開き、初回同意 → Community Node の同意 → アカウントの profile まで進める。 */
async function openClient(name, { ice }) {
  const args = ['--headless=new', '--lang=en-US', '--window-size=1280,900', '--no-sandbox'];
  const browser = await remote({
    logLevel: 'warn',
    capabilities: {
      browserName: 'chrome',
      'goog:chromeOptions': { args },
      ...(process.env.CHROMEDRIVER ? { 'wdio:chromedriverOptions': { binary: process.env.CHROMEDRIVER } } : {}),
    },
  });
  browser.label = `${name}-${RUN}`;
  await browser.addInitScript(captureClipboard);
  if (!ice) await browser.addInitScript(withoutIceCandidates);
  await browser.url(ORIGIN);
  await acceptFirstRun(browser);
  const profile = await dialogWith(browser, 'Set up your profile');
  const displayName = profile.$('input');
  await displayName.waitForClickable({ timeout: WAIT });
  await displayName.setValue(browser.label);
  await profile.$('button=Save').click();
  await eventually(`${name} saves the profile`, async () => !(await findDialog(browser, 'Set up your profile')));
  return browser;
}

/** relay fallback の端: 送る SDP と受け取る SDP から ICE の候補を除き、ICE を成立させない（ページの script より先に動く）。 */
function withoutIceCandidates() {
  const strip = (description) =>
    description && {
      type: description.type,
      sdp: description.sdp.split(/\r?\n/).filter((line) => !line.startsWith('a=candidate:')).join('\r\n'),
    };
  const prototype = RTCPeerConnection.prototype;
  const local = Object.getOwnPropertyDescriptor(prototype, 'localDescription');
  Object.defineProperty(prototype, 'localDescription', { get() { return strip(local.get.call(this)); } });
  const setRemote = prototype.setRemoteDescription;
  prototype.setRemoteDescription = function (description) { return setRemote.call(this, strip(description)); };
}

/** 共有リンク（clipboard へ書く値）を控える（ページより先に動く）。headless では clipboard へ書けないことがある。 */
function captureClipboard() {
  window.__kukuriCopied = [];
  navigator.clipboard.writeText = async (text) => {
    window.__kukuriCopied.push(text);
  };
}

async function findDialog(browser, text) {
  for await (const dialog of browser.$$('[role=dialog]')) {
    // 閉じかけの dialog は、一覧を取ってから読むまでの間に消える（stale element）。
    if ((await dialog.getText().catch(() => '')).includes(text)) return dialog;
  }
  return null;
}

const dialogWith = (browser, text) => eventually(`dialog "${text}"`, () => findDialog(browser, text));

const pageText = (browser) => browser.execute(() => document.body.innerText);

/** 先頭にいないときの新着は「Show N new post(s)」を押すと並ぶ。 */
async function showNewPosts(browser) {
  const button = browser.$('//button[starts-with(normalize-space(.), "Show ") and contains(., "new post")]');
  if (await button.isExisting()) await button.click().catch(() => undefined);
}

const sees = (browser, text) =>
  eventually(`${browser.label} sees "${text}"`, async () => {
    await showNewPosts(browser);
    return (await pageText(browser)).includes(text);
  });

/** 列（id は `column:<kind>:<topic>:<channel>:<相手>`。channel と相手は無ければ `-`）。 */
const columnOf = (browser, kind, tail) =>
  browser.$(`section[data-column-id^="column:${kind}:"][data-column-id$=":${tail}"]`);
const publicColumn = (browser) => columnOf(browser, 'timeline', '-:-');
const channelColumn = (browser, channelId) => columnOf(browser, 'timeline', `${channelId}:-`);

async function openComposer(column) {
  await column.waitForExist({ timeout: WAIT });
  const composer = column.$('textarea[placeholder="Write a post"]');
  if (!(await composer.isDisplayed())) await column.$('button[aria-label^="Post to "]').click();
  return composer;
}

/** 列（既定は公開の列）の投稿欄から投稿する。`file` は添える画像。 */
async function post(browser, content, column = publicColumn(browser), file = null) {
  await (await openComposer(column)).setValue(content);
  if (file) await column.$('input[type=file]').addValue(file);
  await column.$('button=Post').click();
  await sees(browser, content);
}

const card = (browser, text) => browser.$(`article*=${text}`);

async function reply(browser, target, content) {
  await (await card(browser, target)).$('button[aria-label="Reply"]').click();
  await browser.$('textarea[placeholder="Write a reply"]').setValue(content);
  await browser.$('button=Reply').click();
}

async function react(browser, target, emoji) {
  await (await card(browser, target)).$('button[aria-label="React"]').click();
  await browser.$(`button[aria-label="${emoji}"]`).click();
}

const EMOJI = { 'thumbs-up': '👍', heart: '❤️', fire: '🔥', clap: '👏', 'party-popper': '🎉', sparkles: '✨', 'raised-hands': '🙌' };

/** 投稿の card に反応が出る。 */
const seesReaction = (browser, target, emoji) =>
  eventually(`${browser.label} sees ${emoji} on "${target}"`, async () => {
    await showNewPosts(browser);
    return browser.execute(
      (text, mark) => [...document.querySelectorAll('article')].some((node) => node.innerText.includes(text) && node.innerText.includes(mark)),
      target,
      emoji
    );
  });

/** 列の topic を切り替える（その topic の timeline を表示して購読する）。 */
const switchTopic = (browser, name) =>
  browser.$('select[aria-label="Timeline topic"]').selectByAttribute('value', topicId(name));

/** native との主要導線を、同じ操作で確かめる（直接経路の端でも fallback の端でも）。返り値は Web と native の投稿。 */
async function exchangeWithNative(browser, tag, reaction) {
  const fromWeb = `hello from ${tag} ${RUN}`;
  await post(browser, fromWeb);
  const webPost = await nativeSees(fromWeb, (item) => item.content === fromWeb);
  const fromNative = `hello from native to ${tag} ${RUN}`;
  await native('create_post', { request: { topic: TOPIC, content: fromNative, reply_to: null } });
  await sees(browser, fromNative);
  const nativePost = (await nativeTimeline()).find((item) => item.content === fromNative);
  const webReply = `reply from ${tag} ${RUN}`;
  await reply(browser, fromNative, webReply);
  await nativeSees(webReply, (item) => item.content === webReply && item.reply_to === nativePost.object_id);
  await react(browser, fromNative, reaction);
  await nativeSees(`the ${reaction} of ${tag}`, (item) =>
    item.object_id === nativePost.object_id && JSON.stringify(item.reaction_summary).includes(EMOJI[reaction])
  );
  const nativeReply = `native reply to ${tag} ${RUN}`;
  await native('create_post', { request: { topic: TOPIC, content: nativeReply, reply_to: webPost.object_id } });
  await sees(browser, nativeReply);
  await nativeReact(webPost, EMOJI['party-popper']);
  await seesReaction(browser, fromWeb, EMOJI['party-popper']);
  return { fromWeb, webPost, fromNative, nativePost };
}

/** 別の topic へ切り替えて、その topic の投稿が native と行き来する。 */
async function exchangeInTopic(browser, name, tag) {
  await nativeShows(topicId(name));
  await switchTopic(browser, name);
  const fromWeb = `${tag} in ${name} ${RUN}`;
  await post(browser, fromWeb);
  await nativeSees(fromWeb, (item) => item.content === fromWeb, topicId(name));
  const fromNative = `native in ${name} for ${tag} ${RUN}`;
  await native('create_post', { request: { topic: topicId(name), content: fromNative, reply_to: null } });
  await sees(browser, fromNative);
  await switchTopic(browser, 'general');
}

/** Web↔Web: 双方が投稿し、互いの投稿へ返信と反応をして、相手に届く。同じ投稿への別の人の 2 件目の反応も、続けて
 * （90 秒以内に）付けて届く（#1505 の修正の後の挙動）。 */
async function exchangeBetweenWeb(x, y, reactions) {
  const xPost = `hello from ${x.label} to ${y.label}`;
  const yPost = `hello from ${y.label} to ${x.label}`;
  await post(x, xPost);
  await post(y, yPost);
  await sees(x, yPost);
  await sees(y, xPost);
  const fromX = `reply from ${x.label} to ${y.label}`;
  await reply(x, yPost, fromX);
  await sees(y, fromX);
  await react(x, yPost, reactions[0]);
  await seesReaction(y, yPost, EMOJI[reactions[0]]);
  const fromY = `reply from ${y.label} to ${x.label}`;
  await reply(y, xPost, fromY);
  await sees(x, fromY);
  await react(y, xPost, reactions[1]);
  await seesReaction(x, xPost, EMOJI[reactions[1]]);
  await react(y, yPost, reactions[2]);
  await seesReaction(x, yPost, EMOJI[reactions[2]]);
}

/** `publish` が画像を添えて送ってから、受け手がその画像を読み込むまでに relay が中継した bytes と、画像の bytes
 * （`publish` の返り値。受け手が表示するのは添えた原本）を返す。画像が届く前に投稿の card と操作が出ていたか（欠けた
 * media があっても表示と操作が成り立つ）も返す。 */
async function relayedWhileLoading(browser, content, publish) {
  const before = await relayedBytes();
  const size = await publish();
  let shownBeforeImage = false;
  await eventually(`${browser.label} loads the image of "${content}"`, async () => {
    await showNewPosts(browser);
    const state = await browser.execute((text) => {
      const article = [...document.querySelectorAll('article')].find((node) => node.innerText.includes(text));
      if (!article) return 'none';
      // media は画面内の投稿だけ取得する。
      article.scrollIntoView({ block: 'center' });
      const image = article.querySelector('img[src^="blob:"]');
      if (image?.complete && image.naturalWidth > 0) return 'loaded';
      return article.querySelector('button[aria-label="Reply"]') ? 'card' : 'none';
    }, content);
    shownBeforeImage ||= state === 'card';
    return state === 'loaded';
  });
  return { relayed: (await relayedBytes()) - before, size, shownBeforeImage };
}

/** 直接経路なら relay の中継が画像より十分小さく、fallback なら画像の大半が relay を通っている。 */
function assertRoute(name, result, direct) {
  console.log(name, result);
  assert.ok(
    direct ? result.relayed < result.size / 4 : result.relayed >= result.size * 0.9,
    `${name} ${direct ? 'went through' : 'did not use'} the relay: ${JSON.stringify(result)}`
  );
}

/** 経路の判定に使う画像（乱数の画素の PNG。毎回違う内容）。 */
const payloadPng = async () => Buffer.from(await (await fetch(`${ORIGIN}/fixture/payload.png`)).arrayBuffer());

/** native が画像を添えて投稿・DM を送る（`request` に添付を足す）。返り値は画像の bytes 数。 */
const nativeWithImage = (command, request) => async () => {
  const png = await payloadPng();
  const image = { file_name: 'payload.png', mime: 'image/png', byte_size: png.length, data_base64: png.toString('base64'), role: 'image_original' };
  await native(command, { request: { ...request, attachments: [image] } });
  return png.length;
};

const postNativeImage = (content) => nativeWithImage('create_post', { topic: TOPIC, content, reply_to: null });

/** Web の投稿欄・DM 欄から添える画像（一時 file）。 */
async function payloadFile() {
  const png = await payloadPng();
  const file = path.join(await mkdtemp(path.join(tmpdir(), 'kukuri-web-e2e-')), 'payload.png');
  await writeFile(file, png);
  return { file, size: png.length };
}

/** Web が画像を添えて投稿する（`column` は公開か channel の列）。返り値は画像の bytes 数。 */
const postWebImage = (browser, content, column = publicColumn(browser)) => async () => {
  const { file, size } = await payloadFile();
  await post(browser, content, column, file);
  return size;
};

const channelScope = (channelId) => ({ kind: 'channel', channel_id: channelId });

const channelRef = (channelId) => ({ kind: 'private_channel', channel_id: channelId });

const nativePostsInChannel = (channelId, content) =>
  native('create_post', { request: { topic: TOPIC, content, reply_to: null, channel_ref: channelRef(channelId) } });

const nativeSeesInChannel = (channelId, content) =>
  eventually(`native sees "${content}" in ${channelId}`, async () => {
    const page = await native('list_timeline', { request: { topic: TOPIC, scope: channelScope(channelId), limit: 50 } });
    return page.items.some((item) => item.content === content);
  });

const nativeShowsChannel = (channelId) =>
  native('set_scope_display', {
    request: {
      observer: `web-e2e-native-${channelId}`,
      target: { kind: 'timeline', topic: TOPIC, scope: channelScope(channelId) },
      visible: true,
    },
  });

/** native が共有の token で channel に参加し、その channel の列を表示する。 */
async function nativeJoinsChannel(token) {
  const preview = await native('import_channel_access_token', { request: { token } });
  await nativeShowsChannel(preview.channel_id);
  return preview.channel_id;
}

/** native が招待制の channel を作って表示し、共有の token を返す。 */
async function nativeCreatesChannel(label) {
  const channel = await native('create_private_channel', {
    request: { topic: TOPIC, label, audience_kind: 'invite_only' },
  });
  await nativeShowsChannel(channel.channel_id);
  const access = await native('export_channel_access_token', {
    request: { topic: TOPIC, channel_id: channel.channel_id, expires_at: null },
  });
  return { channelId: channel.channel_id, token: access.token };
}

/** channel の列を画面に入れて、投稿が出るのを待つ。画面外の列は読み直さない（#765）。作成・参加の dialog を閉じると
 * focus が元の列へ戻り、channel の列は画面外に残る（#1517）ので、利用者と同じく列を画面に入れてから見る。 */
async function seesInChannel(browser, channelId, text) {
  await channelColumn(browser, channelId).scrollIntoView();
  await sees(browser, text);
}

/** channel の投稿が Web と native の間で行き来し、native の画像の経路を判定する。返り値は Web の投稿。 */
async function exchangeInChannelWithNative(browser, channelId, tag, direct) {
  const fromWeb = `${tag} in channel from ${browser.label}`;
  await post(browser, fromWeb, channelColumn(browser, channelId));
  await nativeSeesInChannel(channelId, fromWeb);
  const fromNative = `${tag} in channel from native to ${browser.label}`;
  await nativePostsInChannel(channelId, fromNative);
  await seesInChannel(browser, channelId, fromNative);
  const image = `${tag} channel image from native to ${browser.label}`;
  const publish = nativeWithImage('create_post', { topic: TOPIC, content: image, reply_to: null, channel_ref: channelRef(channelId) });
  assertRoute(`native→web channel (${tag})`, await relayedWhileLoading(browser, image, publish), direct);
  return fromWeb;
}

/** private channel の作成・参加の dialog（public の timeline の列の見出しから開く）。 */
async function openChannelDialog(browser) {
  await browser.$('button[aria-label="Create or join a private channel"]').click();
  return dialogWith(browser, 'Create / Join Private Channel');
}

/** 作成・参加の dialog が閉じ終わるまで待つ。閉じる途中は、元の列へ戻る focus がその列を active にし直し（#1517）、
 * その間の別の列の押下が失われる。 */
const channelDialogClosed = (browser) =>
  eventually('the channel dialog closes', async () => !(await findDialog(browser, 'Create / Join Private Channel')));

/** Web で招待制の channel を作り、共有の token（JSON）を返す。その channel の列が増える。 */
async function createChannel(browser, label) {
  const dialog = await openChannelDialog(browser);
  await dialog.$('input[placeholder="Channel name"]').setValue(label);
  await dialog.$('button=Create Channel').click();
  const copy = browser.$('[role=dialog] button[aria-label="Copy link"]');
  await copy.waitForClickable({ timeout: WAIT });
  await copy.click();
  const link = await eventually('the share link', () => browser.execute(() => window.__kukuriCopied.at(-1)));
  await browser.keys('Escape');
  await channelDialogClosed(browser);
  return new URL(link).searchParams.get('token');
}

/** Web で共有の token を貼って channel に参加し、その channel の列を開く（dialog は閉じる）。 */
async function joinChannel(browser, token, label) {
  const dialog = await openChannelDialog(browser);
  await dialog.$('textarea[placeholder="Paste a private channel invite, mutual grant, or mutuals+ share"]').setValue(token);
  await dialog.$('button=Join').click();
  const open = browser.$(`[role=dialog] button[aria-label="Open ${label}"]`);
  await open.waitForClickable({ timeout: WAIT });
  await open.click();
  await channelDialogClosed(browser);
}

/** 投稿の作者（`author`）の profile を開き、まだなら follow する。profile の操作の欄を返す。 */
async function openAuthorProfile(browser, authorPost, author) {
  await showNewPosts(browser);
  await (await card(browser, authorPost)).$('button.post-meta-author').click();
  const profile = columnOf(browser, 'profile', author);
  await profile.waitForExist({ timeout: WAIT });
  const actions = profile.$('.author-detail-action-buttons');
  await actions.waitForExist({ timeout: WAIT });
  const follow = actions.$('button=Follow');
  if (await follow.isExisting()) await follow.click();
  await actions.$('button=Unfollow').waitForExist({ timeout: WAIT });
  return actions;
}

/** 開いている `peer` の profile の列に、相互になって「Message」が出たら会話を開く。列を閉じず、開き直さずに待つ
 * （相手の follow は follow の offer で届き、送れなかったら送り直される。届くと開いている列が読み直される。#1521）。 */
async function messageFromOpenProfile(browser, peer) {
  const message = columnOf(browser, 'profile', peer).$('.author-detail-action-buttons').$('button=Message');
  await message.waitForExist({ timeout: FOLLOW_BACK_WAIT });
  await message.click();
}

/** 投稿の作者（`peer`）の profile を開いて follow し、相互になったら会話を開く。 */
async function openDirectMessage(browser, authorPost, peer) {
  await openAuthorProfile(browser, authorPost, peer);
  await messageFromOpenProfile(browser, peer);
}

/** `peer` との会話の列（「Message」で開く。会話を開く照会の後に増える）の投稿欄から DM を送る。`file` は添える画像。 */
async function sendDirectMessage(browser, peer, text, file = null) {
  const column = columnOf(browser, 'conversation', peer);
  await column.waitForExist({ timeout: WAIT });
  const composer = column.$('textarea[placeholder="Write a message"]');
  if (!(await composer.isDisplayed())) await column.$('button[aria-label^="Message to "]').click();
  await composer.setValue(text);
  if (file) await column.$('input[type=file]').addValue(file);
  await column.$('button=Send').click();
  await sees(browser, text);
}

/** Web が画像を添えて DM を送る。返り値は画像の bytes 数。 */
const sendWebImage = (browser, peer, text) => async () => {
  const { file, size } = await payloadFile();
  await sendDirectMessage(browser, peer, text, file);
  return size;
};

const nativeSeesDirectMessage = (pubkey, text) =>
  eventually(`native receives the message "${text}"`, async () => {
    const page = await native('list_direct_message_messages', { request: { pubkey, cursor: null, limit: 50 } });
    return page.items.some((item) => item.text === text);
  });

/** native との DM: 互いに follow し、Web と native が送り合い、native の画像の経路を判定する。`nativeParty` は native の
 * pubkey と投稿（profile を開く入口）。 */
async function exchangeDirectMessagesWithNative(browser, webPubkey, nativeParty, direct) {
  await native('follow_author', { request: { pubkey: webPubkey } });
  await openDirectMessage(browser, nativeParty.post, nativeParty.pubkey);
  const fromWeb = `dm from ${browser.label}`;
  await sendDirectMessage(browser, nativeParty.pubkey, fromWeb);
  await nativeSeesDirectMessage(webPubkey, fromWeb);
  const fromNative = `dm from native to ${browser.label}`;
  await native('send_direct_message', { request: { pubkey: webPubkey, text: fromNative, reply_to_message_id: null } });
  await sees(browser, fromNative);
  const image = `dm image from native to ${browser.label}`;
  const publish = nativeWithImage('send_direct_message', { pubkey: webPubkey, text: image, reply_to_message_id: null });
  assertRoute(`native→web dm (${browser.label})`, await relayedWhileLoading(browser, image, publish), direct);
}

/** Web↔Web の DM: 互いの投稿の作者を follow して会話を開いて送り合い、y から x への画像の経路を判定する。x・y は
 * `{ browser, pubkey, post }`（post は profile を開く入口）。x は y を follow したら profile を開いたまま、y が follow
 * し返すのを待つ（#1220 AC-2h）。 */
async function exchangeDirectMessagesBetweenWeb(x, y, direct) {
  await openAuthorProfile(x.browser, y.post, y.pubkey);
  await openDirectMessage(y.browser, x.post, x.pubkey);
  await messageFromOpenProfile(x.browser, y.pubkey);
  const fromY = `dm from ${y.browser.label} to ${x.browser.label}`;
  await sendDirectMessage(y.browser, x.pubkey, fromY);
  await sees(x.browser, fromY);
  const fromX = `dm from ${x.browser.label} to ${y.browser.label}`;
  await sendDirectMessage(x.browser, y.pubkey, fromX);
  await sees(y.browser, fromX);
  const image = `dm image from ${y.browser.label} to ${x.browser.label}`;
  const result = await relayedWhileLoading(x.browser, image, sendWebImage(y.browser, x.pubkey, image));
  assertRoute(`web→web dm (${x.browser.label})`, result, direct);
}

/** 設定のドロワーの section を開く。 */
async function openSettings(browser, section) {
  if (!(await browser.$('#shell-settings-drawer[data-open="true"]').isExisting())) {
    await browser.$('[data-testid="control-center-trigger"]').click();
    await browser.$('#shell-control-center button[aria-label="Settings"]').click();
  }
  await browser.$(`[data-testid="settings-section-${section}"]`).click();
}

/** 主要な 3 つの設定（表示と言語、成人向け表示、Community Node の接続設定）の変更が反映される（2026-10-03 ユーザー判断）。
 * 開発者モード（Community Node の状態の表示）は `assertConnected` で有効にした後に呼ぶ。 */
async function changeSettings(browser) {
  await openSettings(browser, 'appearance');
  await browser.$('button[role=radio][aria-label="Light"]').click();
  await eventually('the light theme', () => browser.execute(() => document.documentElement.dataset.theme === 'light'));
  await browser.$('button[role=radio][aria-label="Dark"]').click();
  await eventually('the dark theme', () => browser.execute(() => document.documentElement.dataset.theme === 'dark'));
  const language = browser.$('//select[option[@value="zh-CN"]]');
  await language.selectByAttribute('value', 'ja');
  await eventually('the Japanese UI', () =>
    browser.execute(() => document.documentElement.lang === 'ja' && document.body.innerText.includes('表示と言語'))
  );
  await language.selectByAttribute('value', 'en');
  await eventually('the English UI', () => browser.execute(() => document.documentElement.lang === 'en'));

  // 成人向け: 自己申告の投稿は、表示を有効にするまで本文を出さない。
  const adult = `adult from native ${RUN}`;
  const adultId = await native('create_post', {
    request: { topic: TOPIC, content: adult, reply_to: null, content_labels: ['adult'] },
  });
  await browser.$('button[aria-label="Close settings"]').click();
  await eventually('the gated adult post', async () => {
    await showNewPosts(browser);
    return browser.$(`[data-testid="post-adult-gated-${adultId}"]`).isExisting();
  });
  assert.ok(!(await pageText(browser)).includes(adult), 'the adult post is hidden by default');
  await openSettings(browser, 'safety');
  await browser.$('[data-testid="adult-content-display-toggle"]').click();
  await browser.$('button[aria-label="Close settings"]').click();
  await sees(browser, adult);

  // Community Node: node の追加と削除を保存できる。同意と認証の状態を示し、同意を撤回すると未同意・未認証になり、
  // 同意し直して認証し直せる。
  await openSettings(browser, 'community-node');
  const extra = 'http://127.0.0.1:9';
  const extraNode = `//h4[normalize-space()="${extra}"]`;
  const unsaved = async () => (await pageText(browser)).includes('Unsaved');
  const saveNodes = async () => {
    await eventually('the unsaved node edits', unsaved);
    await browser.$('button=Save Nodes').click();
    await eventually('the node list is saved', async () => !(await unsaved()));
  };
  await browser.$('button=Add Node').click();
  await (await browser.$$('input[aria-label="Base URL"]')).at(-1).setValue(extra);
  await saveNodes();
  await browser.$(`${extraNode}/ancestor::section[1]//button[normalize-space()="Remove"]`).click();
  await saveNodes();
  await eventually('the removed node is no longer listed', async () => !(await browser.$(extraNode).isExisting()));
  const status = (label) =>
    browser.$(`//dt[normalize-space()="${label}"]/following-sibling::dd[1]`).getText();
  await eventually('the node is authenticated', async () => (await status('Auth')).startsWith('yes'));
  assert.equal(await status('Consent'), 'accepted');
  const policies = () => dialogWith(browser, 'Community node policies');
  // 撤回と同意の後は dialog が閉じる（閉じ終わるまで overlay が下の操作を遮る）。
  const closed = async () => !(await findDialog(browser, 'Community node policies'));
  await browser.$('button=Consents').click();
  // 文書を読み込むと、すべて同意済みの表示になる。
  await (await policies()).$('button=All accepted').waitForExist({ timeout: WAIT });
  await (await policies()).$('button=Withdraw consent').click();
  await eventually('the consent is withdrawn', async () =>
    (await status('Consent')) === 'Consent withdrawn' && (await status('Auth')) === 'no' && (await closed())
  );
  await browser.$('button=Consents').click();
  const accept = (await policies()).$('button=Accept');
  await accept.waitForClickable({ timeout: WAIT });
  await accept.click();
  await eventually('the consent is accepted again', async () => (await status('Consent')) === 'accepted' && (await closed()));
  await browser.$('button=Authenticate').click();
  await eventually('the node is authenticated again', async () => (await status('Auth')).startsWith('yes'));
  await browser.$('button[aria-label="Close settings"]').click();
}

/** ブラウザの EndpointId と接続先（設定の Discovery の診断。開発者モードで出る）。接続先は topic の gossip の隣接なので、
 * 既知の相手（`peers`）のどれかを持てばよい。native が接続先なら、native もブラウザを接続先に持つ（双方の EndpointId の
 * 照合）。実データがどの経路を通ったかは relay の bytes で判定する。 */
async function assertConnected(browser, peers, nativeEndpoint) {
  await openSettings(browser, 'developer');
  await browser.$('label*=Enable developer mode').$('input[type=checkbox]').click();
  await browser.$('[data-testid="settings-section-discovery"]').click();
  const details = browser.$('//*[normalize-space(text())="Technical diagnostic details"]');
  await details.click();
  // 診断の「LOCAL ENDPOINT ID」と「CONNECTED PEERS」（表示は大文字）。
  // 接続先は今の接続の一覧なので、読み直しながら待つ。
  const { endpoint, connected } = await eventually(`${browser.label} connects to one of ${peers}`, async () => {
    await browser.$('button=Refresh diagnostics').click();
    const text = await pageText(browser);
    const local = text.match(/LOCAL ENDPOINT ID\s+([0-9a-f]{64})/);
    const list = text.match(/CONNECTED PEERS\s+([0-9a-f\s]+)/);
    return local && list && peers.some((peer) => list[1].includes(peer)) ? { endpoint: local[1], connected: list[1] } : null;
  });
  if (connected.includes(nativeEndpoint)) {
    await eventually(`native connects to ${endpoint}`, async () => {
      const page = await native('list_connectivity_peers', { request: { kind: 'connected', limit: 64 } });
      return JSON.stringify(page).includes(endpoint);
    });
  }
  await browser.keys('Escape');
  return endpoint;
}

/** 新しいアカウントの初回（同意・Community Node の同意）を進める。 */
async function acceptFirstRun(browser) {
  await browser.$('[data-testid="age-attestation-checkbox"]').click();
  await browser.$('button=Accept and continue').click();
  await (await dialogWith(browser, 'What is a community node?')).$('button=Review terms').click();
  await (await dialogWith(browser, 'Not now')).$('button=Accept').click();
}

/**
 * サイトデータが消えた後、export した鍵を初回の dialog の入口から import して、同じ公開鍵のアカウントへ戻る（#1217 AC-5、
 * ADR 0059 §6）。全履歴の回復は確かめない。
 */
async function recoverAfterSiteDataLoss(browser) {
  const passphrase = `recovery-${RUN}`;
  await openSettings(browser, 'account');
  // 保存の状態を偽らない: 示す状態は browser の答えと同じ。
  const persisted = await browser.execute(() => navigator.storage.persisted());
  const notice = await browser.$('[data-testid="browser-storage-notice"]').getText();
  assert.ok(notice.includes(persisted ? 'does not delete' : 'may delete'), notice);
  await browser.$('[data-testid="export-acknowledge"]').click();
  await browser.$('[data-testid="export-passphrase"]').setValue(passphrase);
  await browser.$('[data-testid="export-passphrase-confirm"]').setValue(passphrase);
  await browser.$('[data-testid="export-submit"]').click();
  const envelope = browser.$('[data-testid="export-envelope"]');
  await envelope.waitForExist({ timeout: WAIT });
  const exported = await envelope.getValue();
  const [publicKey] = (await browser.$('[data-testid="export-result"]').getText()).match(/[0-9a-f]{64}/);

  // 利用者がサイトデータを消したのと同じく、同じ origin の保存先を消す。app を動かさない文書で消す（開いた接続が待たせない）。
  await browser.url(`${ORIGIN}/fixture/info`);
  await browser.execute(async () => {
    for (const { name } of await indexedDB.databases()) {
      await new Promise((resolve, reject) => {
        const request = indexedDB.deleteDatabase(name);
        request.onsuccess = resolve;
        request.onerror = () => reject(request.error);
      });
    }
    localStorage.clear();
    sessionStorage.clear();
  });

  // 開き直すと初回の同意から始まり、新しいアカウントの profile の設定になる（以前のアカウントは無い）。
  await browser.url(ORIGIN);
  await acceptFirstRun(browser);
  await (await dialogWith(browser, 'Set up your profile')).$('button=Restore a previous account').click();
  const add = await dialogWith(browser, 'Import an encrypted account key');
  await add.$('[data-testid="import-input"]').setValue(exported);
  await add.$('[data-testid="import-preview-button"]').click();
  await add.$('[data-testid="import-passphrase"]').setValue(passphrase);
  await add.$('[data-testid="import-submit"]').click();
  const switchNow = add.$('[data-testid="import-switch-now"]');
  await switchNow.waitForClickable({ timeout: WAIT });
  await switchNow.click();

  // 切り替えると画面を読み込み直す。出てくる初回の dialog を閉じてから、使っているアカウントの公開鍵を見る。読み込み直しの
  // 途中の要素は使えないので、失敗したら次の回で見直す。
  await eventually('the restored account is active', async () => {
    try {
      for (const text of ['What is a community node?', 'Set up your profile']) {
        const dialog = await findDialog(browser, text);
        if (dialog) {
          await dialog.$('button=Later').click();
          return false;
        }
      }
      await openSettings(browser, 'account');
      const active = browser.$('[data-testid="account-list"]').$('li*=Active');
      const restored = (await active.isExisting()) && (await active.getText()).includes(publicKey);
      await browser.keys('Escape');
      return restored;
    } catch {
      return false;
    }
  });
}

/** native が `topic` に URL を書いた公開投稿を作る。`title` があれば、その投稿のリンクプレビューの record を書く（OGP は
 * 取得しない。AC-2g）。 */
async function nativeLinkPost(topic, tag, title = null) {
  const url = `https://example.com/kukuri-web-e2e/${tag}-${RUN}`;
  const content = `${tag} ${RUN} ${url}`;
  const objectId = await native('create_post', { request: { topic, content, reply_to: null } });
  const record = title && (await fixture('/fixture/link-preview', { object_id: objectId, url, title }));
  return { url, content, title, image: record?.image_data_url };
}

/** 公開の列の投稿のリンクプレビュー: slot の状態、card の題と読み込んだ画像の src、本文の link。preview は画面内の投稿
 * だけ読む。 */
const linkPreviewOf = (browser, content) =>
  browser.execute((text) => {
    const column = document.querySelector('section[data-column-id^="column:timeline:"][data-column-id$=":-:-"]');
    const article = [...(column?.querySelectorAll('article') ?? [])].find((node) => node.innerText.includes(text));
    if (!article) return null;
    article.scrollIntoView({ block: 'center' });
    const image = article.querySelector('img.link-preview-image');
    return {
      state: article.querySelector('.link-preview-slot')?.dataset.state,
      title: article.querySelector('.link-preview-title')?.textContent,
      image: image?.complete && image.naturalWidth > 0 ? image.getAttribute('src') : null,
      link: article.querySelector('a.smart-external-link')?.getAttribute('href'),
    };
  }, content);

/** card に record の題と画像が出る。 */
const seesLinkPreview = (browser, post) =>
  eventually(`${browser.label} sees the link preview of "${post.content}"`, async () => {
    await showNewPosts(browser);
    const preview = await linkPreviewOf(browser, post.content);
    return preview?.state === 'available' && preview.title === post.title && preview.image === post.image;
  });

/** record の無い投稿は、card を出さず URL（本文の link）だけを示す。 */
const seesOnlyUrl = (browser, post) =>
  eventually(`${browser.label} shows only the url of "${post.content}"`, async () => {
    await showNewPosts(browser);
    const preview = await linkPreviewOf(browser, post.content);
    return preview?.state === 'unavailable' && preview.link === post.url;
  });

/** 失敗したときの手がかり（CI だけで落ちたとき用）: 各 client の列の id・画面内か・本文の先頭。 */
async function dumpColumns(browser) {
  const columns = await browser.execute(() =>
    [...document.querySelectorAll('section.shell-column-surface')].map(
      (node) => `[${node.dataset.columnId} visible=${node.dataset.runtimeVisible ?? 'false'}]\n${node.innerText.slice(0, 800)}`
    )
  );
  console.log(`--- ${browser.label}\n${columns.join('\n')}`);
}

async function main() {
  const clients = [];
  await nativeShows(TOPIC);
  const { endpoint_id: nativeEndpoint, pubkey: nativePubkey } = await fixture('/fixture/info');
  try {
    // 直接経路: native↔Web。
    const a = await openClient('web-a', { ice: true });
    clients.push(a);
    const withA = await exchangeWithNative(a, 'web-a', 'thumbs-up');
    await exchangeInTopic(a, 'dev', 'web-a');
    const direct = await relayedWhileLoading(a, `native image ${RUN}`, postNativeImage(`native image ${RUN}`));
    assertRoute('native→web direct', direct, true);
    const aEndpoint = await assertConnected(a, [nativeEndpoint], nativeEndpoint);
    // AC-2b（直接経路）: native との DM と、主要な 3 つの設定。
    const aPubkey = withA.webPost.author_pubkey;
    await exchangeDirectMessagesWithNative(a, aPubkey, { pubkey: nativePubkey, post: withA.fromNative }, true);
    await changeSettings(a);

    // 直接経路: Web↔Web。
    const b = await openClient('web-b', { ice: true });
    clients.push(b);
    await sees(b, withA.fromWeb);
    await exchangeBetweenWeb(b, a, ['heart', 'fire', 'clap']);
    const webImage = `web image ${RUN}`;
    assertRoute('web→web direct', await relayedWhileLoading(b, webImage, postWebImage(a, webImage)), true);
    const bEndpoint = await assertConnected(b, [nativeEndpoint, aEndpoint], nativeEndpoint);
    // AC-2b（直接経路）: Web が作った private channel に native と別の Web が参加し、投稿が行き来する。Web↔Web の DM。
    const channelLabel = `channel-a-${RUN}`;
    const channelToken = await createChannel(a, channelLabel);
    const channelId = await nativeJoinsChannel(channelToken);
    const fromAInChannel = await exchangeInChannelWithNative(a, channelId, 'direct', true);
    await joinChannel(b, channelToken, channelLabel);
    await seesInChannel(b, channelId, fromAInChannel);
    const fromBInChannel = `web-b in channel ${RUN}`;
    await post(b, fromBInChannel, channelColumn(b, channelId));
    await seesInChannel(a, channelId, fromBInChannel);
    const channelImage = `channel image to web-b ${RUN}`;
    const toB = await relayedWhileLoading(b, channelImage, postWebImage(a, channelImage, channelColumn(a, channelId)));
    assertRoute('web→web channel direct', toB, true);
    const bHello = `hello from ${b.label} to ${a.label}`;
    const bPubkey = (await nativeSees(bHello, (item) => item.content === bHello)).author_pubkey;
    await exchangeDirectMessagesBetweenWeb(
      { browser: b, pubkey: bPubkey, post: bHello },
      { browser: a, pubkey: aPubkey, post: `hello from ${a.label} to ${b.label}` },
      true
    );
    // native が作った channel に Web が参加する（作成と招待の向きの逆）。fallback の端を開く前に確かめる（fallback の
    // 端の通信は relay を通るので、直接経路の判定に混ざる）。
    const nativeChannelLabel = `channel-native-${RUN}`;
    const nativeChannel = await nativeCreatesChannel(nativeChannelLabel);
    await joinChannel(a, nativeChannel.token, nativeChannelLabel);
    await exchangeInChannelWithNative(a, nativeChannel.channelId, 'native-owned', true);

    // relay fallback: ICE の成立しない Web。上限つきの timeline（1 ページ 20 件）も、この新しい端で確かめる。
    for (let index = 0; index < 25; index++) {
      await native('create_post', { request: { topic: TOPIC, content: `bulk ${RUN} ${index}`, reply_to: null } });
    }
    const c = await openClient('web-c', { ice: false });
    clients.push(c);
    await sees(c, `bulk ${RUN} 24`);
    const cards = await c.$$('.post-list article').length;
    assert.ok(cards <= 20, `the first page shows at most 20 posts, saw ${cards}`);
    await eventually('the next page of the timeline', async () => {
      await c.execute(() => {
        for (const body of document.querySelectorAll('.shell-column-body')) body.scrollTop = body.scrollHeight;
      });
      const more = await c.$('button=Load more');
      if (await more.isExisting()) await more.click();
      return (await c.$$('.post-list article').length) > cards;
    });
    const withC = await exchangeWithNative(c, 'web-c', 'clap');
    await exchangeInTopic(c, 'test', 'web-c');
    await exchangeBetweenWeb(c, a, ['sparkles', 'raised-hands', 'party-popper']);
    const fallback = await relayedWhileLoading(c, `native image fallback ${RUN}`, postNativeImage(`native image fallback ${RUN}`));
    assertRoute('native→web fallback', fallback, false);
    assert.ok(fallback.shownBeforeImage, 'the post and its actions are shown while the image is missing');
    await assertConnected(c, [nativeEndpoint, aEndpoint, bEndpoint], nativeEndpoint);
    const webFallback = `web image fallback ${RUN}`;
    assertRoute('web→web fallback', await relayedWhileLoading(c, webFallback, postWebImage(a, webFallback)), false);

    // AC-2b（fallback）: channel への参加と投稿、native と Web との DM。
    await joinChannel(c, channelToken, channelLabel);
    await seesInChannel(c, channelId, fromBInChannel);
    const fromCInChannel = await exchangeInChannelWithNative(c, channelId, 'fallback', false);
    await seesInChannel(a, channelId, fromCInChannel);
    const channelFallback = `channel image to web-c ${RUN}`;
    const toC = await relayedWhileLoading(c, channelFallback, postWebImage(a, channelFallback, channelColumn(a, channelId)));
    assertRoute('web→web channel fallback', toC, false);
    // fallback の端が作った channel に native と直接経路の端が参加し、native が作った channel に fallback の端が参加する
    // （作成・招待・参加の両方の向き）。
    const cChannelLabel = `channel-c-${RUN}`;
    const cChannelToken = await createChannel(c, cChannelLabel);
    const cChannelId = await nativeJoinsChannel(cChannelToken);
    const fromCInOwnChannel = await exchangeInChannelWithNative(c, cChannelId, 'fallback-owned', false);
    await joinChannel(a, cChannelToken, cChannelLabel);
    await seesInChannel(a, cChannelId, fromCInOwnChannel);
    const fromAInCChannel = `web-a in channel of web-c ${RUN}`;
    await post(a, fromAInCChannel, channelColumn(a, cChannelId));
    await seesInChannel(c, cChannelId, fromAInCChannel);
    const nativeChannelForCLabel = `channel-native-c-${RUN}`;
    const nativeChannelForC = await nativeCreatesChannel(nativeChannelForCLabel);
    await joinChannel(c, nativeChannelForC.token, nativeChannelForCLabel);
    await exchangeInChannelWithNative(c, nativeChannelForC.channelId, 'native-owned-fallback', false);
    const cPubkey = withC.webPost.author_pubkey;
    await exchangeDirectMessagesWithNative(c, cPubkey, { pubkey: nativePubkey, post: withC.fromNative }, false);
    await exchangeDirectMessagesBetweenWeb(
      { browser: c, pubkey: cPubkey, post: `hello from ${c.label} to ${a.label}` },
      { browser: a, pubkey: aPubkey, post: `hello from ${a.label} to ${c.label}` },
      false
    );

    // AC-2g: リンクプレビュー。native（投稿者）が record を書いた後に Web が投稿を表示する（表示が先だと、record の無い
    // 結果を 60 秒持つ）ように、a が今は表示していない topic に投稿してから、a をその topic へ切り替える。
    await nativeShows(topicId('dev'));
    const withPreview = await nativeLinkPost(topicId('dev'), 'with-preview', `preview title ${RUN}`);
    const urlOnly = await nativeLinkPost(topicId('dev'), 'url-only');
    await switchTopic(a, 'dev');
    await seesLinkPreview(a, withPreview);
    await seesOnlyUrl(a, urlOnly);
    // 投稿者の native を止めた後に開いた Web にも、record を読んだ Web（a）から card と画像が出る（AC-2f の中継）。
    await fixture('/fixture/shutdown', {});
    const d = await openClient('web-d', { ice: true });
    clients.push(d);
    await switchTopic(d, 'dev');
    await seesLinkPreview(d, withPreview);

    // サイトデータが消えた後の復旧（#1217 AC-5）。
    await recoverAfterSiteDataLoss(a);
  } catch (error) {
    for (const client of clients) await dumpColumns(client).catch(() => undefined);
    throw error;
  } finally {
    for (const client of clients) await client.deleteSession().catch(() => undefined);
  }
}

await main();
console.log('web e2e: PASS');

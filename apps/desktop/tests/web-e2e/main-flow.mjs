// Web クライアントの主要導線を実ブラウザで試す（#1220 W8 AC-2a、ADR 0060 §4）。`cargo xtask web-e2e` から呼ぶ。
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
  if (!ice) await browser.addInitScript(withoutIceCandidates);
  await browser.url(ORIGIN);
  await browser.$('[data-testid="age-attestation-checkbox"]').click();
  await browser.$('button=Accept and continue').click();
  await (await dialogWith(browser, 'What is a community node?')).$('button=Review terms').click();
  await (await dialogWith(browser, 'Not now')).$('button=Accept').click();
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

async function findDialog(browser, text) {
  for await (const dialog of browser.$$('[role=dialog]')) {
    if ((await dialog.getText()).includes(text)) return dialog;
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

async function openComposer(browser) {
  const composer = browser.$('textarea[placeholder="Write a post"]');
  if (!(await composer.isDisplayed())) await browser.$('button[aria-label^="Post to Public"]').click();
  return composer;
}

async function post(browser, content) {
  await (await openComposer(browser)).setValue(content);
  await browser.$('button=Post').click();
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

/** Web↔Web: 双方が投稿し、互いの投稿へ返信と反応をして、相手に届く。反応は他の反応の付いていない投稿へ付ける
 * （同じ投稿への 90 秒以内の 2 件目以降の反応は届かない既知の不具合 #1505 を避ける。直ったらこの条件を外す）。 */
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
}

/** 画像を添えた投稿の画像が読み込まれるまで待ち、その間に relay が中継した bytes と、投稿の画像のうち最も小さい版の
 * bytes（受け手が少なくとも取得する量。Web の投稿には preview の版が付く）を返す。画像が届く前に投稿の card と操作が
 * 出ていたか（欠けた media があっても表示と操作が成り立つ）も返す。 */
async function relayedWhileLoading(browser, content, publish) {
  const before = await relayedBytes();
  await publish();
  const posted = await nativeSees(content, (item) => item.content === content && item.attachments.length > 0);
  const size = Math.min(...posted.attachments.filter((a) => a.mime.startsWith('image/')).map((a) => a.bytes));
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

const postNativeImage = (content) => () => fixture('/fixture/post-image', { topic: TOPIC, content });

/** 画像を Web の投稿欄から添えて投稿する（fixture の画像を一時 file にして渡す）。 */
const postWebImage = (browser, content) => async () => {
  const png = Buffer.from(await (await fetch(`${ORIGIN}/fixture/payload.png`)).arrayBuffer());
  const file = path.join(await mkdtemp(path.join(tmpdir(), 'kukuri-web-e2e-')), 'payload.png');
  await writeFile(file, png);
  await (await openComposer(browser)).setValue(content);
  await browser.$('input[type=file]').addValue(file);
  await browser.$('button=Post').click();
};

/** ブラウザの EndpointId と接続先（設定の Discovery の診断。開発者モードで出る）。接続先は topic の gossip の隣接なので、
 * 既知の相手（`peers`）のどれかを持てばよい。native が接続先なら、native もブラウザを接続先に持つ（双方の EndpointId の
 * 照合）。実データがどの経路を通ったかは relay の bytes で判定する。 */
async function assertConnected(browser, peers, nativeEndpoint) {
  await browser.$('[data-testid="control-center-trigger"]').click();
  await browser.$('#shell-control-center').$('button*=Settings').click();
  await browser.$('[data-testid="settings-section-developer"]').click();
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

async function main() {
  const clients = [];
  await nativeShows(TOPIC);
  const { endpoint_id: nativeEndpoint } = await fixture('/fixture/info');
  try {
    // 直接経路: native↔Web。
    const a = await openClient('web-a', { ice: true });
    clients.push(a);
    const withA = await exchangeWithNative(a, 'web-a', 'thumbs-up');
    await exchangeInTopic(a, 'dev', 'web-a');
    const direct = await relayedWhileLoading(a, `native image ${RUN}`, postNativeImage(`native image ${RUN}`));
    console.log('native→web direct', direct);
    assert.ok(direct.relayed < direct.size / 4, `native→web image went through the relay: ${JSON.stringify(direct)}`);
    const aEndpoint = await assertConnected(a, [nativeEndpoint], nativeEndpoint);

    // 直接経路: Web↔Web。
    const b = await openClient('web-b', { ice: true });
    clients.push(b);
    await sees(b, withA.fromWeb);
    await exchangeBetweenWeb(b, a, ['heart', 'fire']);
    const webImage = `web image ${RUN}`;
    const webToWeb = await relayedWhileLoading(b, webImage, postWebImage(a, webImage));
    console.log('web→web direct', webToWeb);
    assert.ok(webToWeb.relayed < webToWeb.size / 4, `web→web image went through the relay: ${JSON.stringify(webToWeb)}`);
    const bEndpoint = await assertConnected(b, [nativeEndpoint, aEndpoint], nativeEndpoint);

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
    await exchangeWithNative(c, 'web-c', 'clap');
    await exchangeInTopic(c, 'test', 'web-c');
    await exchangeBetweenWeb(c, a, ['sparkles', 'raised-hands']);
    const fallback = await relayedWhileLoading(c, `native image fallback ${RUN}`, postNativeImage(`native image fallback ${RUN}`));
    console.log('native→web fallback', fallback);
    assert.ok(fallback.relayed >= fallback.size * 0.9, `native→web image did not use the relay: ${JSON.stringify(fallback)}`);
    assert.ok(fallback.shownBeforeImage, 'the post and its actions are shown while the image is missing');
    await assertConnected(c, [nativeEndpoint, aEndpoint, bEndpoint], nativeEndpoint);
    const webFallback = `web image fallback ${RUN}`;
    const webToWebFallback = await relayedWhileLoading(c, webFallback, postWebImage(a, webFallback));
    console.log('web→web fallback', webToWebFallback);
    assert.ok(webToWebFallback.relayed >= webToWebFallback.size * 0.9, `web→web image did not use the relay: ${JSON.stringify(webToWebFallback)}`);
  } finally {
    for (const client of clients) await client.deleteSession().catch(() => undefined);
  }
}

await main();
console.log('web e2e: PASS');

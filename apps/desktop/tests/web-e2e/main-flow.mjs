// Web クライアントの主要導線と復帰を実ブラウザで試す（#1220 W8 AC-2a・AC-2b・AC-2g・AC-4・AC-5a、W4 AC-5、#1482 J2、ADR 0060 §4）。
// `cargo xtask web-e2e [<scenario>...]` が、scenario ごとに新しい fixture を起動して `node main-flow.mjs <scenario>` で呼ぶ
// （一覧は `--list`。#1559）。相手は harness の web_e2e_fixture（同じ job の Community Node・native の相手・`dist-web` の
// 配信）。各 scenario は新しい client で始め、前提もその中で作るので、他の scenario の状態に依存しない。
//
// ブラウザは `KUKURI_WEB_E2E_BROWSER`（`chrome`（既定）・`firefox`・`safari`）。safaridriver は同時に 1 つの session しか開けない
// ので、`safari` では各 scenario の最初の client だけを Safari にし、2 台目からは Chrome にする（#1220 AC-5b）。driver で作れない
// 段は、未確認の制約として PASS の行に示す。page の script より先に動く試験の script は、fixture が配信に足す（`page-init.js`）。
// - 直接経路: 通常のブラウザ。native↔Web と Web↔Web の実データが WebRTC DataChannel の上の QUIC を通る。
// - relay fallback: 交換する SDP から ICE の候補を除いたブラウザ（ICE が成立しない。W10 の T2 にあたる）。実データが relay
//   を通る。
// 実データの経路は、受け手が画像（乱数の画素の PNG）を取得する間に relay が中継した bytes で判定する。ブラウザには
// relay と WebRTC の他の経路が無いので、relay の中継が画像より十分小さければ、画像は WebRTC を通っている。

import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { Key, remote } from 'webdriverio';

const ORIGIN = process.env.KUKURI_WEB_E2E_ORIGIN ?? 'http://127.0.0.1:4180';
const TOPIC = 'kukuri:topic:general';
const topicId = (name) => `kukuri:topic:${name}`;
const RUN = Date.now().toString(36);
const WAIT = 90_000;
// 開いたままの profile で相手の follow し返しを待つ期限。相手の follow の offer は、送れなかったら 2〜64 秒後に 6 回
// （間隔の計 126 秒）送り直され（#1521 AC-1a）、各回は宛先の探索と送信で最長 10 秒ほどかかるので、それを覆う。
const FOLLOW_BACK_WAIT = 210_000;
const BROWSER = process.env.KUKURI_WEB_E2E_BROWSER ?? 'chrome';
const BROWSERS = {
  chrome: {
    browserName: 'chrome',
    'goog:chromeOptions': { args: ['--headless=new', '--lang=en-US', '--window-size=1280,900', '--no-sandbox'] },
    ...(process.env.CHROMEDRIVER ? { 'wdio:chromedriverOptions': { binary: process.env.CHROMEDRIVER } } : {}),
    // 版を指定すると、WebdriverIO が自動更新されない Chrome for Testing と driver を組で入れる（macOS の runner の Chrome は
    // job の途中で自動更新され、runner の chromedriver と合わなくなる）。
    ...(process.env.KUKURI_WEB_E2E_CHROME_VERSION ? { browserVersion: process.env.KUKURI_WEB_E2E_CHROME_VERSION } : {}),
  },
  firefox: {
    browserName: 'firefox',
    'moz:firefoxOptions': {
      args: ['-headless', '--width=1280', '--height=900'],
      // JSON の viewer は特権の文書になり、page の script から保存先を消せない（site-data の `/fixture/info`）。
      prefs: { 'intl.accept_languages': 'en-US', 'intl.locale.requested': 'en-US', 'devtools.jsonview.enabled': false },
    },
    ...(process.env.GECKODRIVER ? { 'wdio:geckodriverOptions': { binary: process.env.GECKODRIVER } } : {}),
  },
  // safaridriver は WebDriver BiDi を持たない。
  safari: { browserName: 'safari', 'wdio:enforceWebDriverClassic': true },
};
/** 全選択の修飾 key（macOS は Command）。 */
const SELECT_ALL = process.platform === 'darwin' ? Key.Command : Key.Ctrl;
/** この browser の driver で作れず、確かめなかった段（未確認の制約。PASS にしない。#1220 AC-5a）。 */
const unconfirmed = [];

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

/** scenario が開いた client（失敗したときの手がかりと、終わりの CSP の確認・session の終了の対象）。 */
const clients = [];

/** 新しい profile のブラウザを開き、初回同意 → Community Node の同意まで進める（profile の dialog が出ている）。 */
async function startClient(name, { ice }) {
  const kind = BROWSER === 'safari' && clients.length > 0 ? 'chrome' : BROWSER;
  const paired = 'Safari with Safari (safaridriver opens one session at a time)';
  if (kind !== BROWSER && !unconfirmed.includes(paired)) unconfirmed.push(paired);
  const browser = await remote({ logLevel: 'warn', capabilities: BROWSERS[kind] });
  browser.label = `${name}-${RUN}`;
  console.log(browser.label, browser.capabilities.browserName, browser.capabilities.browserVersion);
  clients.push(browser);
  if (kind === 'safari') {
    await browser.setWindowSize(1280, 900);
    // 同じ macOS で Chrome の client が動くと Safari の page が focus を失い、safaridriver の押下と入力が page に届かない。
    // 押下・入力の前に Safari を開き直して focus を戻す（AppleScript の activate では page に focus が戻らなかった）。
    const activate = () =>
      new Promise((resolve, reject) => execFile('open', ['-a', 'Safari'], (error) => (error ? reject(error) : resolve())));
    for (const command of ['click', 'addValue', 'setValue']) {
      browser.overwriteCommand(command, async (original, ...args) => (await activate(), original(...args)), true);
    }
    browser.overwriteCommand('keys', async (original, ...args) => (await activate(), original(...args)));
  }
  if (!ice) {
    // relay fallback の端の印（`page-init.js` が読む）。cookie は同じ origin の文書で置く。
    await browser.url(`${ORIGIN}/fixture/info`);
    await browser.execute(() => {
      document.cookie = 'kukuri-e2e-without-ice=1; path=/';
    });
  }
  await browser.url(ORIGIN);
  await acceptFirstRun(browser);
  return browser;
}

/** 新しい profile のブラウザを開き、初回同意 → Community Node の同意 → アカウントの profile まで進める。 */
async function openClient(name, { ice }) {
  const browser = await startClient(name, { ice });
  const profile = await dialogWith(browser, 'Set up your profile');
  const displayName = profile.$('input');
  await displayName.waitForClickable({ timeout: WAIT });
  await displayName.setValue(browser.label);
  await profile.$('button=Save').click();
  await eventually(`${name} saves the profile`, async () => !(await findDialog(browser, 'Set up your profile')));
  return browser;
}

/** 今の page で CSP の違反が無い（page を読み込み直すと控えは消えるので、読み込み直す前に呼ぶ）。 */
const assertNoCspViolations = async (browser) =>
  assert.deepEqual(await browser.execute(() => window.__kukuriCspViolations), [], `${browser.label} has no CSP violations`);

async function findDialog(browser, text) {
  for await (const dialog of browser.$$('[role=dialog]')) {
    // 閉じかけの dialog は、一覧を取ってから読むまでの間に消える（stale element）。safaridriver は表示していない要素の文も
    // 返すので、表示も確かめる。
    if ((await dialog.getText().catch(() => '')).includes(text) && (await dialog.isDisplayed().catch(() => false))) return dialog;
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

/** 列（既定は公開の列）の投稿欄から投稿する。`file` は添える画像。欄に残った文（復元した下書き）は、利用者と同じく欄を押して
 * 全選択から消す（`setValue` の clear は Chrome では React の state を消さず、列が active になる再描画で文が戻る）。本文は要素へ
 * 入れる（safaridriver の key の操作は、同じ文字が続くと 2 つ目を落とす）。 */
async function post(browser, content, column = publicColumn(browser), file = null) {
  const composer = await openComposer(column);
  await composer.click();
  await browser.keys([SELECT_ALL, 'a']);
  await browser.keys(Key.Backspace);
  await composer.addValue(content);
  if (file) await attach(browser, column, file);
  await column.$('button=Post').click();
  await sees(browser, content);
}

/** 列の投稿欄に画像（`file`）を添える。safaridriver は file の input へ入力できないので、Safari では page で File を作って入れる。 */
async function attach(browser, column, file) {
  const input = column.$('input[type=file]');
  if (browser.capabilities.browserName !== 'Safari') return input.addValue(file);
  const data = (await readFile(file)).toString('base64');
  await browser.execute(
    (element, name, data) => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([Uint8Array.from(atob(data), (c) => c.charCodeAt(0))], name, { type: 'image/png' }));
      element.files = transfer.files;
      element.dispatchEvent(new Event('change', { bubbles: true }));
    },
    input,
    path.basename(file),
    data
  );
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

/** Web と native が公開の投稿を 1 件ずつ送り合う。返り値は双方の投稿（Web の投稿の `author_pubkey` が Web の公開鍵、native の
 * 投稿は profile を開く入口）。 */
async function greetNative(browser, tag) {
  const fromWeb = `hello from ${tag} ${RUN}`;
  await post(browser, fromWeb);
  const webPost = await nativeSees(fromWeb, (item) => item.content === fromWeb);
  const fromNative = `hello from native to ${tag} ${RUN}`;
  await native('create_post', { request: { topic: TOPIC, content: fromNative, reply_to: null } });
  await sees(browser, fromNative);
  const nativePost = (await nativeTimeline()).find((item) => item.content === fromNative);
  return { fromWeb, webPost, fromNative, nativePost };
}

/** native との主要導線を、同じ操作で確かめる（直接経路の端でも fallback の端でも）。返り値は Web と native の投稿。 */
async function exchangeWithNative(browser, tag, reaction) {
  const greeting = await greetNative(browser, tag);
  const { fromWeb, webPost, fromNative, nativePost } = greeting;
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
  return greeting;
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

/** native の command に base64 で添える画像。 */
const imageAttachment = (png) => ({
  file_name: 'payload.png', mime: 'image/png', byte_size: png.length, data_base64: png.toString('base64'), role: 'image_original',
});

/** native が画像を添えて投稿・DM を送る（`request` に添付を足す）。返り値は画像の bytes 数。`png` を省くと新しい画像。 */
const nativeWithImage = (command, request, png = null) => async () => {
  const image = png ?? (await payloadPng());
  await native(command, { request: { ...request, attachments: [imageAttachment(image)] } });
  return image.length;
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

/** channel の投稿が Web と native の間で行き来する。返り値は Web の投稿。 */
async function exchangePostsInChannel(browser, channelId, tag) {
  const fromWeb = `${tag} in channel from ${browser.label}`;
  await post(browser, fromWeb, channelColumn(browser, channelId));
  await nativeSeesInChannel(channelId, fromWeb);
  const fromNative = `${tag} in channel from native to ${browser.label}`;
  await nativePostsInChannel(channelId, fromNative);
  await seesInChannel(browser, channelId, fromNative);
  return fromWeb;
}

/** channel の投稿が Web と native の間で行き来し、native の画像の経路を判定する。返り値は Web の投稿。 */
async function exchangeInChannelWithNative(browser, channelId, tag, direct) {
  const fromWeb = await exchangePostsInChannel(browser, channelId, tag);
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

/** 投稿の作者（`author`）の profile の列を開く。開いていれば閉じて開き直す（列の中の設定は開いたときに読む）。 */
async function reopenAuthorProfile(browser, authorPost, author) {
  const opened = columnOf(browser, 'profile', author);
  if (await opened.isExisting()) await opened.$('button.shell-column-close-button').click();
  await showNewPosts(browser);
  await (await card(browser, authorPost)).$('button.post-meta-author').click();
  const profile = columnOf(browser, 'profile', author);
  await profile.waitForExist({ timeout: WAIT });
  return profile;
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
  if (file) await attach(browser, column, file);
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

/** 設定のドロワーの section を開く。閉じていた drawer は開くときに滑り込む（180 ms）ので、開き終わってから押す（動いている間の
 * 押下は、位置を求めてから送るまでの間に drawer が動くと、section の外に当たる。#1559）。 */
async function openSettings(browser, section) {
  if (!(await browser.$('#shell-settings-drawer[data-open="true"]').isExisting())) {
    await browser.$('[data-testid="control-center-trigger"]').click();
    await browser.$('#shell-control-center button[aria-label="Settings"]').click();
  }
  await eventually('the settings drawer opens', () =>
    browser.execute(() => {
      const drawer = document.querySelector('#shell-settings-drawer');
      return drawer.dataset.open === 'true' && drawer.getAnimations().length === 0;
    })
  );
  await browser.$(`[data-testid="settings-section-${section}"]`).click();
}

/** 開発者モード（Community Node の状態と診断の表示）を有効にする。保存されるので、reload の後は有効のまま。 */
async function enableDeveloperMode(browser) {
  await openSettings(browser, 'developer');
  const developerMode = browser.$('label*=Enable developer mode').$('input[type=checkbox]');
  if (!(await developerMode.isSelected())) await developerMode.click();
}

/** `settings`（W8 AC-2b）: 主要な 3 つの設定（表示と言語、成人向け表示、Community Node の接続設定）の変更が反映される
 * （2026-10-03 ユーザー判断）。 */
async function settings() {
  const browser = await openClient('web-a', { ice: true });
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

  // Community Node: node の追加と削除を保存できる。同意と認証の状態（開発者モードで出る）を示し、同意を撤回すると未同意・
  // 未認証になり、同意し直して認証し直せる。
  await enableDeveloperMode(browser);
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
  await enableDeveloperMode(browser);
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
  const age = browser.$('[data-testid="age-attestation-checkbox"]');
  await age.waitForClickable({ timeout: WAIT });
  await age.click();
  await browser.$('button=Accept and continue').click();
  await (await dialogWith(browser, 'What is a community node?')).$('button=Review terms').click();
  await (await dialogWith(browser, 'Not now')).$('button=Accept').click();
}

/**
 * `site-data`（W4 AC-5）: サイトデータが消えた後、export した鍵を初回の dialog の入口から import して、同じ公開鍵のアカウントへ
 * 戻る（#1217 AC-5、ADR 0059 §6）。全履歴の回復は確かめない。
 */
async function siteData() {
  const browser = await openClient('web-a', { ice: true });
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
  // 鍵の導出の Worker（export）が CSP の下で動いた。
  await assertNoCspViolations(browser);

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
  // 鍵の導出の Worker（import）が CSP の下で動いた（切り替えると読み込み直す）。
  await assertNoCspViolations(browser);
  await switchNow.click();

  // 切り替えると画面を読み込み直す。出てくる初回の dialog を閉じてから、使っているアカウントの公開鍵を見る。読み込み直しの
  // 途中の要素は使えないので、失敗したら次の回で見直す。
  await eventually('the restored account is active', async () => {
    try {
      // 2 つが重なって出ることがあるので、いちばん上（最後）の dialog から閉じる。
      const dialogs = await browser.$$('[role=dialog]');
      const top = dialogs.length ? dialogs[dialogs.length - 1] : null;
      if (top && /What is a community node\?|Set up your profile/.test(await top.getText())) {
        await top.$('button=Later').click();
        return false;
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

/** native の移行の状態が `state` になるまで待つ。 */
const nativeTransfer = (state) =>
  eventually(`the native transfer is ${state}`, async () => {
    const status = await native('get_account_transfer_status');
    return status.state === state && status;
  });

/** 移行元が native のときの、招待リンクと確認コードの承認（#1211）。 */
const nativeSource = {
  invite: async () => (await native('create_account_transfer_invite')).link,
  confirm: async (code) => {
    assert.equal((await nativeTransfer('confirming')).code, code);
    await native('decide_account_transfer', { request: { accept: true } });
  },
};

/** 移行元が同じアカウントの Web（`source`）のとき。アカウントの menu の「アカウント追加」→「別の端末へ移す」でリンクを出す。 */
const webSource = (source) => ({
  invite: async () => {
    await source.$('[data-testid="account-menu-trigger"]').click();
    await source.$('[role=menu]').$('button=Add account').click();
    await (await dialogWith(source, 'Move between devices')).$('button=Move to another device').click();
    const panel = source.$('[role=dialog] [data-testid="account-transfer-source"]');
    await panel.waitForExist({ timeout: WAIT });
    return eventually(`${source.label} shows the transfer link`, async () => (await panel.$('input').getValue()) || false);
  },
  confirm: async (code) => {
    const shown = source.$('[aria-label="Confirmation code"]');
    await shown.waitForExist({ timeout: WAIT });
    assert.equal((await shown.getText()).replace(/\s/g, ''), code);
    await source.$('button=Codes match').click();
  },
  // 移行元の完了（移行先の保存の ACK を受けた後）を待ってから閉じる。
  done: async () => {
    const completed = 'The other device has saved the account';
    await dialogWith(source, completed);
    await eventually(`${source.label} closes the transfer`, async () => {
      if (!(await findDialog(source, completed))) return true;
      await source.keys('Escape');
      return false;
    });
  },
});

/**
 * 新しい Web が、移行元（`from`。`nativeSource` か `webSource`）のリンクで `account` を受け取り、そのアカウントへ切り替える
 * （#1211、#1220 AC-3b・AC-3c）。初回の profile の dialog の「以前のアカウントを戻す」から入る。`history` は履歴の範囲（null は
 * 移さない）、`duringHistory` は履歴を受けている間の操作。`cut` を渡すと、接続の後にその bytes を受けた時点で WebRTC の経路だけを
 * 落とし（`page-init.js`）、完了の後に戻す。
 */
async function transferAccount(from, name, account, { history = null, duringHistory = null, cut = null } = {}) {
  const browser = await startClient(name, { ice: true });
  const link = await from.invite();
  await (await dialogWith(browser, 'Set up your profile')).$('button=Restore a previous account').click();
  await (await dialogWith(browser, 'Import an encrypted account key')).$('button=Move from another device').click();
  const target = browser.$('[role=dialog] [data-testid="account-transfer-target"]');
  await target.waitForExist({ timeout: WAIT });
  // W7 #1211 AC-5: 接続の前に、移るもの・移らないもの・移行元にアカウントが残ることを示す。
  const scope = await target.$('[data-testid="account-transfer-scope"]').getText();
  for (const shown of ['Moves:', 'follows and blocks', "Doesn't move:", 'The account stays on the source device.']) {
    assert.ok(scope.includes(shown), `the target shows "${shown}" before connecting`);
  }
  await target.$('input').setValue(link);
  if (history) await target.$('select').selectByAttribute('value', history);
  if (cut !== null) {
    await browser.execute((after) => {
      window.__kukuriCut = { after };
    }, cut);
  }
  await target.$('button=Connect').click();
  // 確認コードが両端末で同じことを確かめてから、両方で承認する。
  const shown = browser.$('[aria-label="Confirmation code"]');
  await shown.waitForExist({ timeout: WAIT });
  await browser.$('button=Codes match').click();
  await from.confirm((await shown.getText()).replace(/\s/g, ''));
  if (duringHistory) {
    await browser.$('[data-testid="account-transfer-history"]').waitForExist({ timeout: WAIT });
    await duringHistory(browser);
  }
  // AC-5: 完了の画面に移っていないもの（履歴を止めたときはその結果も）を示し、「このアカウントを使う」を押すと切り替える。
  const completed = browser.$('[role=dialog] [data-testid="account-transfer-completed"]');
  await completed.waitForExist({ timeout: WAIT });
  const result = await completed.getText();
  assert.ok(result.includes('Not moved:'), 'the target lists what was not moved');
  if (duringHistory) assert.ok(result.includes('stopped partway'), 'the target shows that the history stopped partway');
  if (cut !== null) {
    assert.ok(await browser.execute(() => window.__kukuriCut.done), `${browser.label} loses its WebRTC paths during the transfer`);
    await browser.execute(() => {
      window.__kukuriCut.released = true;
    });
  }
  await completed.$('button=Use this account').click();
  await from.done?.();
  // 「このアカウントを使う」で、そのアカウントへ切り替えて読み込み直す。Community Node の同意は端末とアカウントごとなので、もう一度
  // 同意する（W7 #1211 AC-4）。読み込み直しの途中の要素は使えないので、失敗したら次の回で見直す。
  await eventually(`${browser.label} uses the transferred account`, async () => {
    try {
      const onboarding = await findDialog(browser, 'What is a community node?');
      if (onboarding) {
        await onboarding.$('button=Review terms').click();
        await (await dialogWith(browser, 'Not now')).$('button=Accept').click();
        return false;
      }
      await openSettings(browser, 'account');
      const active = browser.$('[data-testid="account-list"]').$('li*=Active');
      const switched = (await active.isExisting()) && (await active.getText()).includes(account);
      await browser.keys('Escape');
      return switched;
    } catch {
      return false;
    }
  });
  return browser;
}

/** 設定の「アカウント」の、本人の別の端末との同期の状態（#1220 AC-3b）。同期が始まる前で表示が無いときは null（drawer は
 * 開いたままなので、次の回は読み直すだけになる）。 */
async function accountSyncState(browser) {
  await openSettings(browser, 'account');
  const state = await browser.$('[data-testid="account-sync-status"]').getAttribute('data-state').catch(() => null);
  if (state !== null) await browser.keys('Escape');
  return state;
}

/** アカウントの menu の「View profile」で、自分の profile の列（既定の列。id に公開鍵を含まない）を開く。 */
async function openOwnProfile(browser) {
  await browser.$('[data-testid="account-menu-trigger"]').click();
  await browser.$('[role=menu]').$('button=View profile').click();
  const profile = columnOf(browser, 'profile', '-:-');
  await profile.waitForExist({ timeout: WAIT });
  return profile;
}

/** 自分の profile の表示名を変える。 */
async function setOwnName(browser, name) {
  const own = await openOwnProfile(browser);
  await own.$('button=Edit Profile').click();
  await own.$('input[placeholder="Visible label"]').setValue(name);
  await own.$('button=Save Profile').click();
}

/**
 * `transfer`（#1220 AC-3b）: native のアカウントを新しい Web（e）へ移し、再 QR なしで profile・著者を常に表示する指定・private
 * channel の鍵が届くことを確かめる。同期の状態と中身、別のアカウントへの公開の profile の到達、別の Web（f）へ履歴を移す途中で
 * 止めても必須の移行が完了のままであることも確かめる（#1220 の範囲の固定、2026-10-04）。a は別のアカウントの Web で、native と
 * 投稿と DM を送り合っておく（同期の中身に DM の本文が混ざらないことの確認の対象）。
 */
async function transfer() {
  const { pubkey: nativePubkey } = await fixture('/fixture/info');
  const a = await openClient('web-a', { ice: true });
  const withA = await greetNative(a, 'web-a');
  const aPubkey = withA.webPost.author_pubkey;
  await exchangeDirectMessagesWithNative(a, aPubkey, { pubkey: nativePubkey, post: withA.fromNative }, true);
  const channelLabel = `channel-transfer-${RUN}`;
  const { channelId } = await nativeCreatesChannel(channelLabel);
  const e = await transferAccount(nativeSource, 'web-e', nativePubkey);

  // 同期の状態: e の設定と native の通信状態が、どちらも同期済み。
  await eventually('e is in sync with native', async () => (await accountSyncState(e)) === 'synced');
  await eventually('native is in sync with e', async () => {
    const marks = (await native('get_sync_status')).account_sync;
    return marks && Object.values(marks).every((mark) => !mark);
  });

  // profile: native で変えると e に、e で変えると native に届く。
  const mine = await native('get_my_profile');
  const nativeName = `native name ${RUN}`;
  await native('set_my_profile', { request: { name: mine.name, display_name: nativeName, about: mine.about, clear_picture: false } });
  await eventually('e shows the name set on native', async () => (await (await openOwnProfile(e)).getText()).includes(nativeName));
  const webName = `web-e name ${RUN}`;
  await setOwnName(e, webName);
  await eventually('native has the name set on e', async () => (await native('get_my_profile')).display_name === webName);
  // 別のアカウントの Web（a）には、公開の profile の更新がこれまでどおり届く（本人の端末間の同期と混同しない）。
  const nativePost = `native post for the profile ${RUN}`;
  await native('create_post', { request: { topic: TOPIC, content: nativePost, reply_to: null } });
  await sees(a, nativePost);
  await eventually(`${a.label} sees the profile changed on e`, async () =>
    (await (await reopenAuthorProfile(a, nativePost, nativePubkey)).getText()).includes(webName)
  );

  // 著者を常に表示する指定: native で付けると e に、e で外すと native に届く。
  const otherPost = `post of ${a.label} for the display exception ${RUN}`;
  await post(a, otherPost);
  await sees(e, otherPost);
  await native('set_author_trust_display_exception', { request: { author_pubkey: aPubkey, always_visible: true } });
  const toggle = () => columnOf(e, 'profile', aPubkey).$('[data-testid="author-trust-display-exception-toggle"]');
  await eventually('e has the display exception set on native', async () => {
    await reopenAuthorProfile(e, otherPost, aPubkey);
    return (await toggle().isExisting()) && toggle().isSelected();
  });
  await toggle().click();
  await eventually('native drops the display exception cleared on e', async () =>
    !(await native('list_author_trust_display_exceptions')).includes(aPubkey)
  );

  // private channel: e は移行の前の channel に投稿でき、native が鍵を更新した後の投稿を読める（新しい世代の鍵が届く）。
  await (await openChannelDialog(e)).$(`button[aria-label="Open ${channelLabel}"]`).click();
  await channelDialogClosed(e);
  const fromE = `web-e in the transferred channel ${RUN}`;
  await post(e, fromE, channelColumn(e, channelId));
  await nativeSeesInChannel(channelId, fromE);
  await native('export_channel_access_token', { request: { topic: TOPIC, channel_id: channelId, expires_at: null } });
  const afterUpdate = `native after the key update ${RUN}`;
  await nativePostsInChannel(channelId, afterUpdate);
  await seesInChannel(e, channelId, afterUpdate);
  const fromEAfter = `web-e after the key update ${RUN}`;
  await post(e, fromEAfter, channelColumn(e, channelId));
  await nativeSeesInChannel(channelId, fromEAfter);

  // 同期の中身: allowlist の種類だけで、接続の交渉（SDP・ICE）・DM・通知の中身を含まない（ADR 0061 §2）。
  const items = await fixture('/fixture/account-sync-items');
  assert.ok(items.some((item) => item.key === 'profile'), 'the profile is synced');
  for (const item of items) {
    assert.match(
      item.key,
      /^(profile$|trust\/always-visible\/|channel\/|follow\/|graph\/(follows|blocks|followers)\/)/,
      `${item.key} is in the allowlist`
    );
  }
  const payload = JSON.stringify(items);
  for (const leak of ['a=candidate', 'ice-ufrag', 'ice-pwd', 'a=fingerprint', 'dm from ', 'dm image from ']) {
    assert.ok(!payload.includes(leak), `the account sync carries no "${leak}"`);
  }

  // 任意の履歴: 別の新しい Web（f）が履歴を「直近 30 日」で受け、途中で止めても、必須の移行は完了のまま（#1211 AC-3）。
  // 履歴は本文と添付を送るので、止める間を作るために画像つきの投稿を先に作る。
  for (let index = 0; index < 8; index++) await postNativeImage(`history image ${RUN} ${index}`)();
  await transferAccount(nativeSource, 'web-f', nativePubkey, {
    history: 'month',
    duringHistory: (browser) => browser.$('button=Stop receiving history').click(),
  });
  const done = await native('get_account_transfer_status');
  assert.equal(done.state, 'completed');
  assert.ok(done.history?.stopped, `the history stopped partway: ${JSON.stringify(done.history)}`);
}

/** WebRTC の session の状態（作った順。`page-init.js`）。 */
const sessionStates = (browser) => browser.execute(() => window.__kukuriPeers.map((peer) => peer.connectionState));

/** WebRTC の session が 1 つ以上開き、交渉の途中の session が無くなるまで待つ。 */
const directPathOpens = (browser) =>
  eventually(`${browser.label} opens its WebRTC sessions`, async () => {
    const states = await sessionStates(browser);
    return states.includes('connected') && !states.some((state) => state === 'new' || state === 'connecting');
  });

/** 中断より前に作った `count` 件の session が閉じる（旧世代の交渉・candidate・callback を捨てる。ADR 0059 §5）。 */
const earlierSessionsClose = (browser, count) =>
  eventually(`${browser.label} closes the earlier sessions`, async () =>
    (await sessionStates(browser)).slice(0, count).every((state) => state === 'closed')
  );

/** native が公開投稿を `count` 件足す（履歴を増やす）。 */
async function addHistory(tag, count) {
  for (let index = 0; index < count; index++) {
    await native('create_post', { request: { topic: TOPIC, content: `${tag} ${RUN} ${index}`, reply_to: null } });
  }
}

/** 再開した画面の公開の列の最初の頁が窓（20 件）に収まり、閉じていない session が需要のある相手（`peers`）の数以下。どちらも
 * 履歴の総数に依らない（#1220 AC-4。件数に比例しないことは W4・W10・#1221 の試験が確かめる）。 */
async function assertWindowed(browser, peers) {
  const shown = () => publicColumn(browser).$$('.post-list article').length;
  await eventually(`${browser.label} shows the first page`, async () => (await shown()) > 0);
  const cards = await shown();
  assert.ok(cards <= 20, `the first page shows at most 20 posts, saw ${cards}`);
  await directPathOpens(browser);
  // 接続が戻った後も、頁は窓のまま（後から全件を読み足さない）。
  const later = await shown();
  const open = (await sessionStates(browser)).filter((state) => state !== 'closed').length;
  console.log(`${browser.label} windowed`, { cards, later, open });
  assert.ok(later <= 20, `the first page stays within 20 posts, saw ${later}`);
  assert.ok(open <= peers, `at most ${peers} WebRTC sessions, saw ${open}`);
}

/** native の画像が直接経路を通るまで、新しい画像で 3 回まで試す（復帰の直後は、相手ごとの交渉の再開を待つ）。 */
async function nativeImageGoesDirect(browser, tag) {
  for (let attempt = 1; ; attempt++) {
    await directPathOpens(browser);
    const image = `${tag}: native image ${attempt} ${RUN}`;
    const result = await relayedWhileLoading(browser, image, postNativeImage(image));
    console.log(`native→web after ${tag} (${attempt})`, result);
    if (result.relayed < result.size / 4) return;
    assert.ok(attempt < 3, `native→web after ${tag} went through the relay: ${JSON.stringify(result)}`);
  }
}

/** 復帰の後: Web の投稿が同じ公開鍵で native へ届き、native の投稿が Web に出て、native の画像は直接経路を通る。 */
async function resumesOnTheDirectPath(browser, pubkey, tag) {
  const fromWeb = `${tag}: from ${browser.label}`;
  await post(browser, fromWeb);
  await nativeSees(fromWeb, (item) => item.content === fromWeb && item.author_pubkey === pubkey);
  const fromNative = `${tag}: from native ${RUN}`;
  await native('create_post', { request: { topic: TOPIC, content: fromNative, reply_to: null } });
  await sees(browser, fromNative);
  await nativeImageGoesDirect(browser, tag);
}

/** Control Center を閉じ終わるまで待つ。 */
const closeControlCenter = (browser) =>
  eventually(`${browser.label} closes the control center`, async () => {
    if (!(await browser.$('#shell-control-center').isDisplayed().catch(() => false))) return true;
    await browser.keys('Escape');
    return false;
  });

/** Control Center に出る、参加中の channel の名前（退会の操作の label から）。 */
async function joinedChannelLabels(browser) {
  await browser.$('[data-testid="control-center-trigger"]').click();
  const labels = await browser.execute(() =>
    [...document.querySelectorAll('#shell-control-center button[aria-label^="Leave "]')].map((node) =>
      node.getAttribute('aria-label').replace(/^Leave (.*) channel$/, '$1')
    )
  );
  await closeControlCenter(browser);
  return labels;
}

/** Control Center から channel を退会する。名前が長いと、行の退会の操作が枠の外へ切れて押せない（desktop と共通の画面の不具合）
 * ので、短い名前の channel で行う。 */
async function leaveChannel(browser, label) {
  const leave = browser.$(`#shell-control-center button[aria-label="Leave ${label} channel"]`);
  await browser.$('[data-testid="control-center-trigger"]').click();
  await leave.click();
  await (await dialogWith(browser, 'Leave this channel?')).$('button=Yes').click();
  await eventually(`${browser.label} leaves ${label}`, async () => !(await leave.isExisting()));
  await closeControlCenter(browser);
}

/** native から見た Web の profile の版（署名した編集の時刻）。 */
const profileVersion = async (pubkey) => (await native('get_author_social_view', { request: { pubkey } })).updated_at;

/** DM の列の、`text` を含む message の配達の印（Delivered・Pending）と message の id。 */
const directMessageState = (browser, text) =>
  browser.execute((value) => {
    const article = [...document.querySelectorAll('section[data-column-id^="column:conversation:"] article')].find((node) =>
      node.innerText.includes(value)
    );
    const avatar = article?.querySelector('[data-testid^="dm-message-avatar-"]');
    return article
      ? { chip: article.querySelector('.reply-chip')?.textContent, id: avatar?.dataset.testid.replace('dm-message-avatar-', '') }
      : null;
  }, text);

/** 公開の列に読み込んだ投稿の画像の SHA-256 と、その投稿の card の数。 */
const shownImage = (browser, text) =>
  browser.execute(async (value) => {
    const column = document.querySelector('section[data-column-id^="column:timeline:"][data-column-id$=":-:-"]');
    const articles = [...(column?.querySelectorAll('article') ?? [])].filter((node) => node.innerText.includes(value));
    const image = articles[0]?.querySelector('img[src^="blob:"]');
    if (!image) return { cards: articles.length, sha256: null };
    const digest = await crypto.subtle.digest('SHA-256', await window.__kukuriObjectUrls.get(image.src).arrayBuffer());
    return { cards: articles.length, sha256: [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('') };
  }, text);

/**
 * `lifecycle`（#1220 AC-4）: 直接経路の端 a の reload・終了・凍結・回線全断が W4 の保存と復帰の入口へつながり、退会・世代・
 * version を巻き戻さないことを確かめる（ADR 0059 §4〜§6、ADR 0060 §4）。WebRTC の経路だけの喪失は `webrtc-loss`。需要のある
 * 相手は native と b（gossip の隣接は、その EndpointId のどれか）。最後に、AC-4 より前に a が作った channel へ、fallback の端 c が
 * 参加する（r9）。
 */
async function lifecycle() {
  const { endpoint_id: nativeEndpoint, pubkey: nativePubkey } = await fixture('/fixture/info');
  const a = await openClient('web-a', { ice: true });
  const withA = await greetNative(a, 'web-a');
  const aPubkey = withA.webPost.author_pubkey;
  const aEndpoint = await assertConnected(a, [nativeEndpoint], nativeEndpoint);
  // 前提: native と相互に follow する（回線全断の段の DM）。成人向け表示（runtime が保存し、既定は無効）を有効にする。a は native
  // の招待制の channel に参加し、投稿が行き来する。b は同じ topic を表示する。
  await native('follow_author', { request: { pubkey: aPubkey } });
  await openDirectMessage(a, withA.fromNative, nativePubkey);
  // r6 の前提: a が DM で画像を送る（画面が送信中の仮の添付を読みに行く。#1220 AC-4 r6）。
  const dmImage = `dm image from ${a.label} ${RUN}`;
  await sendWebImage(a, nativePubkey, dmImage)();
  await nativeSeesDirectMessage(aPubkey, dmImage);
  await openSettings(a, 'safety');
  await a.$('[data-testid="adult-content-display-toggle"]').click();
  await a.$('button[aria-label="Close settings"]').click();
  const channelLabel = `channel-native-${RUN}`;
  const channel = await nativeCreatesChannel(channelLabel);
  await joinChannel(a, channel.token, channelLabel);
  await exchangePostsInChannel(a, channel.channelId, 'native-owned');
  const b = await openClient('web-b', { ice: true });
  await sees(b, withA.fromWeb);
  const peerEndpoints = [nativeEndpoint, await assertConnected(b, [nativeEndpoint, aEndpoint], nativeEndpoint)];
  const peers = 2;
  // r9 の前提: a が作った招待制の channel に native と b が参加し、b が投稿する（AC-4 の段の後に fallback の端が参加する）。
  const ownLabel = `channel-a-${RUN}`;
  const ownToken = await createChannel(a, ownLabel);
  const ownChannelId = await nativeJoinsChannel(ownToken);
  await joinChannel(b, ownToken, ownLabel);
  const fromBInOwnChannel = `web-b in the channel of web-a ${RUN}`;
  await post(b, fromBInOwnChannel, channelColumn(b, ownChannelId));

  // 旧 state の再送の準備: a が参加中の channel の世代を native が更新し、a は新しい世代の投稿を読める。新しい世代は W6 の背景の
  // 配布（epoch 制御の送信と引継ぎの grant）で届くので、届くまでの時間を記録し、待ちの上限は通常より長くする。次に、a は native
  // の別の channel に参加してから退会し、owner の native はその channel の世代を更新して投稿する（旧い参加への配布）。
  await native('rotate_private_channel', { request: { topic: TOPIC, channel_id: channel.channelId } });
  const afterRotation = `after the rotation ${RUN}`;
  await nativePostsInChannel(channel.channelId, afterRotation);
  const rotatedAt = Date.now();
  await channelColumn(a, channel.channelId).scrollIntoView();
  await eventually(`${a.label} reads the new generation`, async () => {
    await showNewPosts(a);
    return (await pageText(a)).includes(afterRotation);
  }, 4 * WAIT);
  console.log('the new generation reaches web-a in', Date.now() - rotatedAt, 'ms');
  const leftLabel = `lx-${RUN}`;
  const left = await nativeCreatesChannel(leftLabel);
  await joinChannel(a, left.token, leftLabel);
  await leaveChannel(a, leftLabel);
  await native('rotate_private_channel', { request: { topic: TOPIC, channel_id: left.channelId } });
  const inTheLeftChannel = `in the left channel ${RUN}`;
  await nativePostsInChannel(left.channelId, inTheLeftChannel);
  const version = await profileVersion(aPubkey);
  assert.ok(version, 'native knows the version of the profile');
  // 編集の途中（private channel の列の下書き）。
  const draft = `draft in the channel ${RUN}`;
  await (await openComposer(channelColumn(a, channel.channelId))).setValue(draft);

  // reload（1 つ目の履歴の量）: 同じアカウント・EndpointId・設定・下書きで再開し、初回の dialog を出さない。reload の間の投稿も出る。
  await addHistory('history-a', 25);
  const whileReloading = `posted while reloading ${RUN}`;
  await native('create_post', { request: { topic: TOPIC, content: whileReloading, reply_to: null } });
  await a.refresh();
  await assertWindowed(a, peers);
  assert.equal(await assertConnected(a, peerEndpoints, nativeEndpoint), aEndpoint);
  for (const text of ['Set up your profile', 'What is a community node?']) assert.equal(await findDialog(a, text), null, text);
  // 設定（成人向け表示。runtime が保存し、既定は無効）は有効のまま。
  await openSettings(a, 'safety');
  await eventually('the adult content display stays enabled', () => a.$('[data-testid="adult-content-display-toggle"]').isSelected());
  await a.$('button[aria-label="Close settings"]').click();
  await channelColumn(a, channel.channelId).scrollIntoView();
  assert.equal(await channelColumn(a, channel.channelId).$('textarea[placeholder="Write a post"]').getValue(), draft);
  // reload の間の投稿は、利用者が取り直す操作をしなくても出る（「Show N new posts」は受け取り済みの新着を並べるだけで、取得は
  // しない）。
  await sees(a, whileReloading);
  // 退会した channel は、世代の更新（旧い参加への配布）と reload の後も戻らない。更新した世代は reload の後も読み書きできる。
  // profile の版は reload で変わらない（再送を新しい編集にしない）。
  const labels = await joinedChannelLabels(a);
  assert.ok(labels.includes(channelLabel) && !labels.includes(leftLabel), JSON.stringify(labels));
  assert.ok(!(await pageText(a)).includes(inTheLeftChannel), 'the left channel stays left');
  const afterReload = `after the reload ${RUN}`;
  await nativePostsInChannel(channel.channelId, afterReload);
  await seesInChannel(a, channel.channelId, afterReload);
  const fromAInChannel = `from ${a.label} after the reload ${RUN}`;
  await post(a, fromAInChannel, channelColumn(a, channel.channelId));
  await nativeSeesInChannel(channel.channelId, fromAInChannel);
  assert.equal(await profileVersion(aPubkey), version);
  await resumesOnTheDirectPath(a, aPubkey, 'the reload');

  // 終了（2 つ目の履歴の量）: 別の tab が「このタブで使う」で引き継ぐと、元の tab の runtime は止まって session を閉じ、新しい
  // tab が保存から同じ EndpointId で再開する。
  await addHistory('history-b', 25);
  const first = await a.getWindowHandle();
  const { handle: second } = await a.newWindow(ORIGIN);
  await sees(a, 'kukuri is open in another tab');
  await a.$('button=Use in this tab').click();
  await assertWindowed(a, peers);
  await a.switchToWindow(first);
  await sees(a, 'kukuri is open in another tab');
  assert.ok((await sessionStates(a)).every((state) => state === 'closed'), 'the first tab closes its sessions');
  await a.closeWindow();
  await a.switchToWindow(second);
  assert.equal(await assertConnected(a, peerEndpoints, nativeEndpoint), aEndpoint);
  await resumesOnTheDirectPath(a, aPubkey, 'the takeover');

  // 凍結: 利用者と同じく、非表示 → 凍結 → 復帰 → 表示の順にする（chromedriver の freeze は page を非表示にしてから凍結し、resume
  // の後も非表示のまま戻さない。画面は非表示の間は列を読み直さない）。凍結で旧い session を閉じ、復帰では生きた需要の相手と
  // だけ交渉し直す。交渉は相手ごとに復帰の event（resume と可視）1 回につき 3 回まで（W10 の MAX_ATTEMPTS）。
  // freeze と focus の模擬は Chrome の driver（CDP）にしか無い。
  if (BROWSER === 'chrome') {
    await directPathOpens(a);
    const beforeFreeze = (await sessionStates(a)).length;
    await a.freeze();
    await new Promise((resolve) => setTimeout(resolve, 2000));
    await a.resume();
    await a.sendCommandAndGetResult('Emulation.setFocusEmulationEnabled', { enabled: true });
    await earlierSessionsClose(a, beforeFreeze);
    await resumesOnTheDirectPath(a, aPubkey, 'the freeze');
    const renegotiated = (await sessionStates(a)).length - beforeFreeze;
    console.log('negotiations after the freeze', renegotiated);
    assert.ok(renegotiated <= 6 * peers, `at most 6 negotiations per peer after the freeze, saw ${renegotiated}`);
  } else {
    unconfirmed.push('freeze (the driver cannot freeze a page)');
  }

  // 回線の模擬は safaridriver に無い。
  if (BROWSER === 'safari') {
    unconfirmed.push('network offline (the driver cannot emulate the network)');
  } else {
    // 回線全断（driver の回線の模擬）: offline で session を閉じる。その間、接続の案内はつながっていないことと次の手順を示し
    // （つながっているとは示さない）、自分の投稿は手元に出て、DM は送信待ちと示す。online の後、DM は同じ id で 1 回だけ届き、
    // 届いたと示す。
    // native との会話の列は、会話の route で開き直す（前提で開いた会話の列は一時の列）。
    await a.execute((topic, peer) => {
      location.hash = `#/messages?topic=${encodeURIComponent(topic)}&peerPubkey=${peer}`;
    }, TOPIC, nativePubkey);
    await columnOf(a, 'conversation', nativePubkey).waitForExist({ timeout: WAIT });
    await directPathOpens(a);
    const beforeOffline = (await sessionStates(a)).length;
    // Chrome は chromedriver の命令（BiDi の offline では、online の後に WebRTC の session が開き直らなかった）。
    const contexts = [await a.getWindowHandle()];
    const network = (offline) =>
      BROWSER === 'chrome'
        ? a.setNetworkConditions({ offline, latency: 0, download_throughput: -1, upload_throughput: -1 })
        : a.emulationSetNetworkConditions({ networkConditions: offline ? { type: 'offline' } : null, contexts });
    await network(true);
    await earlierSessionsClose(a, beforeOffline);
    await openSettings(a, 'connectivity');
    await eventually('the guidance without a live connection', async () => {
      const text = await pageText(a);
      return text.includes('There is no live connection') && !/A live connection is available|This topic has a live connection/.test(text);
    });
    await a.$('button[aria-label="Close settings"]').click();
    await post(a, `posted offline ${RUN}`);
    const offlineMessage = `dm while offline ${RUN}`;
    await sendDirectMessage(a, nativePubkey, offlineMessage);
    const pending = await eventually('the pending message', async () => {
      const state = await directMessageState(a, offlineMessage);
      return state?.chip === 'Pending' && state;
    });
    await network(false);
    const copies = async () =>
      (await native('list_direct_message_messages', { request: { pubkey: aPubkey, cursor: null, limit: 50 } })).items.filter(
        (item) => item.text === offlineMessage
      );
    await eventually('native receives the message', async () => {
      const received = await copies();
      assert.ok(received.length <= 1, `the message arrives once: ${JSON.stringify(received)}`);
      return received.length === 1 && received[0].message_id === pending.id;
    });
    await eventually('the delivered message', async () => (await directMessageState(a, offlineMessage))?.chip === 'Delivered');
    await resumesOnTheDirectPath(a, aPubkey, 'going online');
    // 再送は届いた後も重ならない。
    assert.equal((await copies()).length, 1, 'the message is delivered once');
  }

  // r9: AC-4 の段の後に、AC-4 より前に a が作った招待制の channel へ fallback の端 c が token で参加し、b の投稿を読む（reload など
  // の後の owner の channel への新しい参加。#1220 AC-4 r9）。c は直接経路の判定を終えた後に開く（relay の通信が判定に混ざらない）。
  const c = await openClient('web-c', { ice: false });
  await joinChannel(c, ownToken, ownLabel);
  await seesInChannel(c, ownChannelId, fromBInOwnChannel);
}

/**
 * `webrtc-loss`（#1220 AC-4、#1482 J2）: WebRTC の経路だけの喪失（relay は健全）。native の画像が直接経路を通る状態で、画像の
 * 半分ほどを受け取った時点で開いている DataChannel をすべて閉じ、画像が出るまでは新しい session の ICE も成立させない（直接経路が
 * 先に戻ると relay を通らずに完了し、判定が時機に依る）。同じ取得が relay で続き（残りだけが relay を通り、取り直しにならない。
 * #1482 J2）、閉じてから 10 秒以内に表示される。表示した画像は原本と同じ hash で、投稿の card は 1 つ。
 */
async function webrtcLoss() {
  const a = await openClient('web-a', { ice: true });
  await greetNative(a, 'web-a');
  await nativeImageGoesDirect(a, 'before the loss');
  await directPathOpens(a);
  const png = await payloadPng();
  const acrossTheLoss = `image across the webrtc loss ${RUN}`;
  await a.execute((after) => {
    window.__kukuriCut = { after };
  }, png.length / 2);
  const publish = nativeWithImage('create_post', { topic: TOPIC, content: acrossTheLoss, reply_to: null }, png);
  const loss = await relayedWhileLoading(a, acrossTheLoss, publish);
  await a.execute(() => {
    window.__kukuriCut.released = true;
  });
  const cutAt = await a.execute(() => window.__kukuriCut.done);
  assert.ok(cutAt, 'the data channel closes during the transfer');
  const sinceCut = (await a.execute(() => Date.now())) - cutAt;
  console.log('webrtc path loss', { ...loss, sinceCut });
  assert.ok(
    loss.relayed > loss.size / 4 && loss.relayed < loss.size,
    `the same fetch continues through the relay: ${JSON.stringify(loss)}`
  );
  assert.ok(sinceCut <= 10_000, `the image shows within 10 s of the loss: ${sinceCut} ms`);
  assert.deepEqual(await shownImage(a, acrossTheLoss), { cards: 1, sha256: createHash('sha256').update(png).digest('hex') });
}

/** 失敗したときの手がかり（CI だけで落ちたとき用）: 各 client の列の id・画面内か・本文の先頭と、WebRTC の session の状態。 */
async function dumpColumns(browser) {
  const columns = await browser.execute(() =>
    [...document.querySelectorAll('section.shell-column-surface')].map(
      (node) => `[${node.dataset.columnId} visible=${node.dataset.runtimeVisible ?? 'false'}]\n${node.innerText.slice(0, 800)}`
    )
  );
  const sessions = await sessionStates(browser).catch(() => []);
  console.log(`--- ${browser.label} sessions=${JSON.stringify(sessions)}\n${columns.join('\n')}`);
}

/**
 * `direct`（W8 AC-2a・AC-2b の直接経路）: native↔Web と Web↔Web の投稿・返信・反応・topic の切替、画像の経路、双方の
 * EndpointId。DM と、private channel の作成と参加の両方向。
 */
async function direct() {
  const { endpoint_id: nativeEndpoint, pubkey: nativePubkey } = await fixture('/fixture/info');
  // native↔Web。
  const a = await openClient('web-a', { ice: true });
  const withA = await exchangeWithNative(a, 'web-a', 'thumbs-up');
  await exchangeInTopic(a, 'dev', 'web-a');
  const nativeImage = await relayedWhileLoading(a, `native image ${RUN}`, postNativeImage(`native image ${RUN}`));
  assertRoute('native→web direct', nativeImage, true);
  const aEndpoint = await assertConnected(a, [nativeEndpoint], nativeEndpoint);
  // AC-2b: native との DM。
  const aPubkey = withA.webPost.author_pubkey;
  await exchangeDirectMessagesWithNative(a, aPubkey, { pubkey: nativePubkey, post: withA.fromNative }, true);

  // Web↔Web。
  const b = await openClient('web-b', { ice: true });
  await sees(b, withA.fromWeb);
  await exchangeBetweenWeb(b, a, ['heart', 'fire', 'clap']);
  const webImage = `web image ${RUN}`;
  assertRoute('web→web direct', await relayedWhileLoading(b, webImage, postWebImage(a, webImage)), true);
  await assertConnected(b, [nativeEndpoint, aEndpoint], nativeEndpoint);
  // AC-2b: Web が作った private channel に native と別の Web が参加し、投稿が行き来する。Web↔Web の DM。
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
  // native が作った channel に Web が参加する（作成と招待の向きの逆）。
  const nativeChannelLabel = `channel-native-${RUN}`;
  const nativeChannel = await nativeCreatesChannel(nativeChannelLabel);
  await joinChannel(a, nativeChannel.token, nativeChannelLabel);
  await exchangeInChannelWithNative(a, nativeChannel.channelId, 'native-owned', true);
}

/**
 * `fallback`（W8 AC-2a・AC-2b の relay fallback）: ICE の成立しない Web c と、native・直接経路の Web a との投稿・返信・反応・
 * topic の切替、画像の経路、画像が届く前の表示と操作。DM と、private channel の作成と参加の 3 通り。上限つきの timeline
 * （1 ページ 20 件）も、新しい端の c で確かめる。
 */
async function fallback() {
  const { endpoint_id: nativeEndpoint, pubkey: nativePubkey } = await fixture('/fixture/info');
  // 前提: 直接経路の a。a が作った招待制の channel に native が参加し、a が投稿している。
  const a = await openClient('web-a', { ice: true });
  const aPubkey = (await greetNative(a, 'web-a')).webPost.author_pubkey;
  const aEndpoint = await assertConnected(a, [nativeEndpoint], nativeEndpoint);
  const channelLabel = `channel-a-${RUN}`;
  const channelToken = await createChannel(a, channelLabel);
  const channelId = await nativeJoinsChannel(channelToken);
  const fromAInChannel = `web-a in channel ${RUN}`;
  await post(a, fromAInChannel, channelColumn(a, channelId));

  for (let index = 0; index < 25; index++) {
    await native('create_post', { request: { topic: TOPIC, content: `bulk ${RUN} ${index}`, reply_to: null } });
  }
  const c = await openClient('web-c', { ice: false });
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
  const nativeImage = await relayedWhileLoading(c, `native image fallback ${RUN}`, postNativeImage(`native image fallback ${RUN}`));
  assertRoute('native→web fallback', nativeImage, false);
  assert.ok(nativeImage.shownBeforeImage, 'the post and its actions are shown while the image is missing');
  await assertConnected(c, [nativeEndpoint, aEndpoint], nativeEndpoint);
  const webFallback = `web image fallback ${RUN}`;
  assertRoute('web→web fallback', await relayedWhileLoading(c, webFallback, postWebImage(a, webFallback)), false);

  // AC-2b: channel への参加と投稿、native と Web との DM。
  await joinChannel(c, channelToken, channelLabel);
  await seesInChannel(c, channelId, fromAInChannel);
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
  const nativeChannelLabel = `channel-native-c-${RUN}`;
  const nativeChannel = await nativeCreatesChannel(nativeChannelLabel);
  await joinChannel(c, nativeChannel.token, nativeChannelLabel);
  await exchangeInChannelWithNative(c, nativeChannel.channelId, 'native-owned-fallback', false);
  const cPubkey = withC.webPost.author_pubkey;
  await exchangeDirectMessagesWithNative(c, cPubkey, { pubkey: nativePubkey, post: withC.fromNative }, false);
  await exchangeDirectMessagesBetweenWeb(
    { browser: c, pubkey: cPubkey, post: `hello from ${c.label} to ${a.label}` },
    { browser: a, pubkey: aPubkey, post: `hello from ${a.label} to ${c.label}` },
    false
  );
}

/**
 * `link-preview`（W8 AC-2g）: native（投稿者）が record を書いた後に Web が投稿を表示する（表示が先だと、record の無い結果を
 * 60 秒持つ）ように、a が表示していない topic に投稿してから、a をその topic へ切り替える。
 */
async function linkPreview() {
  const a = await openClient('web-a', { ice: true });
  await greetNative(a, 'web-a');
  await nativeShows(topicId('dev'));
  const withPreview = await nativeLinkPost(topicId('dev'), 'with-preview', `preview title ${RUN}`);
  const urlOnly = await nativeLinkPost(topicId('dev'), 'url-only');
  await switchTopic(a, 'dev');
  await seesLinkPreview(a, withPreview);
  await seesOnlyUrl(a, urlOnly);
  // 投稿者の native を止めた後に開いた Web にも、record を読んだ Web（a）から card と画像が出る（AC-2f の中継）。
  await fixture('/fixture/shutdown', {});
  const d = await openClient('web-d', { ice: true });
  await switchTopic(d, 'dev');
  await seesLinkPreview(d, withPreview);
}

/** native の参加中の一覧の、channel の現在の世代と古い世代の数（世代の進みを読む）。一覧に無ければ null。 */
async function nativeEpochs(channelId) {
  const page = await native('list_joined_private_channels', { request: { topic: TOPIC, cursor: null } });
  const view = page.items.find((item) => item.channel_id === channelId);
  return view ? { current: view.current_epoch_id, archived: view.archived_epoch_ids.length } : null;
}

/** channel の列の見出しから、その channel の設定を開く（列が無ければ、作成・参加の dialog の一覧から列を開く）。 */
async function openChannelSettings(browser, channelId, label) {
  const column = channelColumn(browser, channelId);
  if (!(await column.isExisting())) {
    await (await openChannelDialog(browser)).$(`button[aria-label="Open ${label}"]`).click();
    await channelDialogClosed(browser);
  }
  await column.scrollIntoView();
  await column.$(`button[aria-label="Open ${label} channel settings and sharing"]`).click();
  return dialogWith(browser, 'Create share link');
}

/** channel の設定と保留の dialog が閉じ終わるまで Escape を押す。 */
const closeChannelSettings = (browser) =>
  eventually(`${browser.label} closes the channel settings`, async () => {
    if (!(await findDialog(browser, 'Create share link')) && !(await findDialog(browser, 'On hold'))) return true;
    await browser.keys('Escape');
    return false;
  });

/** channel の設定に「新しいアクセスの配布は別の端末で行う」が出ているか（W6 の担当でない端末）。 */
const OTHER_DEVICE = 'happen on another of your devices';
async function handledElsewhere(browser, channelId, label) {
  const shown = (await (await openChannelSettings(browser, channelId, label)).getText()).includes(OTHER_DEVICE);
  await closeChannelSettings(browser);
  return shown;
}

/** channel の設定で共有リンクを作る。作れたら token を、保留のダイアログが出たら null を返す（dialog は閉じる）。 */
async function createShareLink(browser, channelId, label) {
  await (await openChannelSettings(browser, channelId, label)).$('button[aria-label="Create share link"]').click();
  const copy = browser.$('[role=dialog] button[aria-label="Copy link"]');
  // 結果が出る間に dialog は描き直されるので、外れた要素は次の回で見直す。
  const outcome = await eventually(`${browser.label} creates or holds the share link`, async () =>
    ((await findDialog(browser, 'On hold')) && 'held') || ((await copy.isClickable().catch(() => false)) && 'created')
  );
  let token = null;
  if (outcome === 'created') {
    const copied = await browser.execute(() => window.__kukuriCopied.length);
    await copy.click();
    const link = await eventually('the share link', () => browser.execute((index) => window.__kukuriCopied[index], copied));
    token = new URL(link).searchParams.get('token');
  }
  await closeChannelSettings(browser);
  return token;
}

/**
 * `same-account`（#1220 AC-3c1・AC-3c2）: 同じアカウントの Web どうし。native → Web e の移行の後、e が作った招待制の channel C を、
 * e のリンクで同じアカウントを受けた Web g と使う。鍵の更新を担う端末（担当。C を作った e）が居ない間の保留と戻った後の処理、
 * 「この端末で行う」、移行の途中で WebRTC の経路だけを落とした間の移行と担当、経路だけを落とした間の担当でない端末の鍵の更新
 * （AC-3c2）を確かめる（範囲の固定、2026-10-04・10-05）。native は同じアカウントの端末で、C を本人の端末間の同期で受け、その世代の
 * 進みを参加中の一覧で読む。
 */
async function sameAccount() {
  const { pubkey: account } = await fixture('/fixture/info');
  const e = await transferAccount(nativeSource, 'web-e', account);
  await eventually('e is in sync with native', async () => (await accountSyncState(e)) === 'synced');
  await greetNative(e, 'web-e');
  const label = `channel-same-account-${RUN}`;
  const created = await native('preview_channel_access_token', { request: { token: await createChannel(e, label) } });
  const channelId = created.channel_id;
  await eventually('native receives C through the account sync', () => nativeEpochs(channelId));
  await nativeShowsChannel(channelId);

  // 2: e のリンクで g が同じアカウントを受け、同期済みになる。g は C に書け（鍵が届いた）、profile は e↔g の両方向に届く。
  const g = await transferAccount(webSource(e), 'web-g', account);
  await eventually('g is in sync', async () => (await accountSyncState(g)) === 'synced');
  const fromG = `web-g in the channel ${RUN}`;
  await (await openChannelDialog(g)).$(`button[aria-label="Open ${label}"]`).click();
  await channelDialogClosed(g);
  await post(g, fromG, channelColumn(g, channelId));
  await nativeSeesInChannel(channelId, fromG);
  for (const [from, to] of [[e, g], [g, e]]) {
    const name = `${from.label} name`;
    await setOwnName(from, name);
    await eventually(`${to.label} shows the name set on ${from.label}`, async () =>
      (await (await openOwnProfile(to)).getText()).includes(name)
    );
  }

  // 3: 担当の e が居ない間に g で共有リンクを作ると保留になる。e が戻ると、g が作り直す前に世代がちょうど 1 つ進む。
  assert.ok(await handledElsewhere(g, channelId, label), 'g leaves new access to another device');
  assert.ok(!(await handledElsewhere(e, channelId, label)), 'e hands out new access');
  await e.url('about:blank');
  const whileAway = await nativeEpochs(channelId);
  assert.equal(await createShareLink(g, channelId, label), null, 'the share link is on hold while e is away');
  await e.url(ORIGIN);
  const handled = await eventually('the request is handled when e returns', async () => {
    const now = await nativeEpochs(channelId);
    return now.current !== whileAway.current && now;
  });
  assert.equal(handled.archived, whileAway.archived + 1, 'the epoch advances by exactly one');
  assert.ok(await createShareLink(g, channelId, label), 'g creates the share link after e returns');

  // 4: g で「この端末で行う」を押すと、g で共有リンクを作れ、e は別の端末に任せる表示になる。g が居ない間の e は保留になる。
  await (await openChannelSettings(g, channelId, label)).$('button=Do this on this device').click();
  await eventually('g hands out new access', async () => !(await findDialog(g, OTHER_DEVICE)));
  await closeChannelSettings(g);
  assert.ok(await createShareLink(g, channelId, label), 'g creates a share link');
  await eventually('e leaves new access to another device', () => handledElsewhere(e, channelId, label));
  await g.url('about:blank');
  const gAway = await nativeEpochs(channelId);
  assert.equal(await createShareLink(e, channelId, label), null, 'the share link is on hold on e while g is away');
  await g.url(ORIGIN);
  await eventually('the request of e is handled when g returns', async () =>
    (await nativeEpochs(channelId)).current !== gAway.current
  );

  // 5: 移行の途中で h の WebRTC の経路だけを落としても（relay は健全）、relay で続いて 1 度だけ完了する。h はアカウントを 1 つだけ
  // 受け取る。
  const h = await transferAccount(webSource(e), 'web-h', account, { cut: 2048 });
  await eventually('h is in sync', async () => (await accountSyncState(h)) === 'synced');
  await openSettings(h, 'account');
  const rows = await h.$('[data-testid="account-list"]').$$('li').map((row) => row.getText());
  assert.equal(rows.filter((row) => row.includes(account)).length, 1, `h has the account once: ${JSON.stringify(rows)}`);
  await h.keys('Escape');
  // 担当は、経路を戻した後も g のまま。
  assert.ok(!(await handledElsewhere(g, channelId, label)), 'g still hands out new access');
  assert.ok(await handledElsewhere(e, channelId, label), 'e still leaves it to g');

  // 6: WebRTC の経路だけを落とした間（relay は健全）。担当でない e の共有リンクは relay で g の鍵の更新を経て 15 秒の待ちの内に
  // 作れ、その token の世代は native の現在の世代と同じで、1 回の依頼で世代は高々 1 つしか進まない（AC-3c2。依頼の直前に落とす
  // 形）。移行の後に置く: 切られた直後の e は移行の WebRTC の経路を張れず、場面 4 で読み込み直した g の起動とも重ならない。
  const beforeLoss = await nativeEpochs(channelId);
  await e.execute(() => {
    window.__kukuriCut = { after: 1 };
  });
  const token = await createShareLink(e, channelId, label);
  assert.ok(token, 'e creates the share link while its WebRTC paths are down');
  assert.ok(await e.execute(() => window.__kukuriCut.done), 'e loses its WebRTC paths');
  const { epoch_id: tokenEpoch } = await native('preview_channel_access_token', { request: { token } });
  const afterLoss = await eventually('native has the epoch of the share link', async () => {
    const now = await nativeEpochs(channelId);
    return now.current === tokenEpoch && now;
  });
  assert.ok(afterLoss.archived - beforeLoss.archived <= 1, 'one request advances the epoch at most once');
}

/** scenario（`cargo xtask web-e2e` と CI の matrix は、`--list` でこの一覧を読む）。 */
const scenarios = {
  direct,
  fallback,
  settings,
  'link-preview': linkPreview,
  lifecycle,
  'webrtc-loss': webrtcLoss,
  'site-data': siteData,
  transfer,
  'same-account': sameAccount,
};

const [name] = process.argv.slice(2);
if (name === '--list') {
  console.log(JSON.stringify(Object.keys(scenarios)));
} else {
  assert.ok(Object.hasOwn(scenarios, name), `unknown scenario "${name}" (one of ${Object.keys(scenarios).join(', ')})`);
  await nativeShows(TOPIC);
  try {
    await scenarios[name]();
    for (const client of clients) await assertNoCspViolations(client);
  } catch (error) {
    for (const client of clients) await dumpColumns(client).catch(() => undefined);
    // DIAG（一時）
    await import('node:fs/promises').then(({ mkdir }) => mkdir('test-results', { recursive: true }));
    for (const client of clients) await client.saveScreenshot(`test-results/diag-${client.label}.png`).catch(() => undefined);
    if (BROWSER === 'safari') {
      // DIAG（一時）: 各 window の Web Locks の状態。
      const safari = clients[0];
      for (const handle of await safari.getWindowHandles().catch(() => [])) {
        await safari.switchToWindow(handle).catch(() => undefined);
        const locks = await safari.execute(async () => JSON.stringify(await navigator.locks.query())).catch((e) => String(e));
        console.info('DIAG locks', handle, await safari.getUrl().catch(() => ''), locks);
      }
    }
    for (const client of clients) {
      const events = await client
        .execute(() => [
          document.hasFocus(),
          document.visibilityState,
          [...document.querySelectorAll('[role=dialog]')].map((dialog) => [...dialog.querySelectorAll('button')].map((b) => b.textContent)),
          ...(window.__kukuriDiag?.slice(-60) ?? []),
        ])
        .catch((e) => String(e));
      console.info(`DIAG ${client.label} ${JSON.stringify(events)}`);
    }
    throw error;
  } finally {
    for (const client of clients) await client.deleteSession().catch(() => undefined);
  }
  console.log(`web e2e ${name} on ${BROWSER}: PASS${unconfirmed.length ? ` (unconfirmed: ${unconfirmed.join('; ')})` : ''}`);
}

import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { eventually } from './wait.mjs';
import { installPwa, nativeTap } from './pwa-entry.mjs';
import { webInvoke } from './web-invoke.mjs';

async function nativeSnapshot(adb, name) {
  await mkdir('test-results/pwa-input', { recursive: true });
  const file = `/sdcard/kukuri-input-${name}.png`;
  await adb('shell', 'screencap', '-p', file);
  await adb('pull', file, `test-results/pwa-input/${name}.png`);
}

async function configureJapanese(browser, field, adb) {
  await field.click();
  await eventually('native Japanese keyboard opens', () => browser.execute(() => visualViewport.height < innerHeight * .8));
  await nativeSnapshot(adb, 'gboard-initial');
  // GoogleのGboard手順と同じ実UIで日本語QWERTYを追加する（入力eventを注入しない）。
  // SDK GboardのkeyboardはUIAutomatorへvirtual keyを公開しない。
  // 同じAPI35/320px/200%の実画面で確認したtoolbarのgearをtapする。
  const size = (await adb('shell', 'wm', 'size')).match(/Override size: (\d+)x(\d+)/);
  assert.deepEqual(size?.slice(1).map(Number), [840, 1890]);
  await adb('shell', 'input', 'tap', '540', '1200');
  await nativeSnapshot(adb, 'gboard-settings');
  try {
    await nativeTap(adb, attrs => attrs.text === 'Languages', 'Gboard languages');
    await nativeTap(adb, attrs => attrs.text === 'Add keyboard', 'Gboard add keyboard');
  } catch (error) {
    await nativeSnapshot(adb, 'gboard-settings-failed');
    throw error;
  }
  await nativeSnapshot(adb, 'gboard-language-list');
  await nativeTap(adb, attrs => /^Search (?:for a )?language$/.test(attrs.text) || /^Search (?:for a )?language$/.test(attrs['content-desc']), 'search Gboard languages');
  await adb('shell', 'input', 'text', 'Japanese');
  await nativeTap(adb, attrs => attrs.text === '日本語', 'Japanese language result');
  await nativeSnapshot(adb, 'japanese-layout-options');
  await nativeTap(adb, attrs => /^(12 keys|12 キー)$/.test(attrs.text), 'disable Japanese 12-key layout');
  await nativeTap(adb, attrs => attrs.text === 'QWERTY', 'Japanese QWERTY');
  await nativeTap(adb, attrs => attrs.text === 'Done', 'save Japanese keyboard');
  await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  await field.click();
  await adb('shell', 'input', 'tap', '400', '1000');
  await eventually('configured Japanese keyboard opens', () => browser.execute(() => visualViewport.height < innerHeight * .8));
  await nativeSnapshot(adb, 'gboard-configured');

}

async function reviewTalkBack(browser, openSettings, adb) {
  await openSettings(browser, 'appearance');
  try {
    await adb('shell', 'pm', 'grant', 'com.google.android.marvin.talkback', 'android.permission.POST_NOTIFICATIONS');
    await adb('shell', 'settings', 'put', 'secure', 'enabled_accessibility_services', 'com.google.android.marvin.talkback/com.google.android.marvin.talkback.TalkBackService');
    await adb('shell', 'settings', 'put', 'secure', 'accessibility_enabled', '1');
    await eventually('SDK TalkBack service binds', async () => (await adb('shell', 'dumpsys', 'accessibility')).includes('TalkBackService'));
    const accessibility = await adb('shell', 'dumpsys', 'accessibility');
    console.log('PWA SDK accessibility state', accessibility.split('\n').filter(line => /TalkBack|boundServices|bindingServices|touchExploration|FocusedWindow/.test(line)).join('\n'));
    console.log('PWA SDK touch exploration', (await adb('shell', 'settings', 'get', 'secure', 'touch_exploration_enabled')).trim());
    await eventually('SDK TalkBack touch exploration activates', async () => (await adb('shell', 'settings', 'get', 'secure', 'touch_exploration_enabled')).trim() === '1', 20000);
    const focused = async () => {
      const output = await adb('shell', 'am', 'instrument', '-w', 'app.kukuri.focusprobe/app.kukuri.focusprobe.FocusProbe');
      const json = output.match(/^INSTRUMENTATION_RESULT: focus=(.+)$/m)?.[1];
      assert.ok(json, output);
      return JSON.parse(json);
    };
    // 実画面の「Language & theme」へtouch explorationの焦点を置く。
    await adb('shell', 'input', 'tap', '590', '380');
    await browser.executeAsync(done => setTimeout(done, 700));
    await nativeSnapshot(adb, 'talkback-start');
    const focus = [await focused()];
    // 実touch explorationで異なるcontrolを探り、nodeの名前・位置の移動を照合する。
    for (const [step, x, y] of [[1, 220, 550], [2, 590, 550], [3, 450, 1300]]) {
      await adb('shell', 'input', 'tap', String(x), String(y));
      await browser.executeAsync(done => setTimeout(done, 500));
      await nativeSnapshot(adb, `talkback-focus-${step}`);
      focus.push(await focused());
    }
    console.log('PWA SDK TalkBack real focus nodes', JSON.stringify(focus));
    assert.ok(focus.every(node => node.package === 'com.android.chrome'));
    assert.ok(new Set(focus.map(node => `${node.text}/${node.description}/${node.top}/${node.left}`)).size > 1, 'TalkBack moves focus through actual UI nodes');
    assert.ok(focus.some(node => /Language|theme|Settings|Keyboard|Safety/.test(`${node.text} ${node.description}`)));
  } finally {
    await adb('shell', 'settings', 'put', 'secure', 'accessibility_enabled', '0');
    await adb('shell', 'settings', 'delete', 'secure', 'enabled_accessibility_services');
  }
  await browser.$('#shell-settings-drawer .shell-settings-close').click();
}

// getBoundingClientRectはlayout viewport基準。keyboardでpanした分を引いて実画面へ写す。
export function editorTapPoint(element) {
  const { left, top, width, height } = element.getBoundingClientRect();
  return { x: (left + width / 2 - visualViewport.offsetLeft) * devicePixelRatio,
    y: (top + height / 2 - visualViewport.offsetTop) * devicePixelRatio + 106 };
}

async function typeJapanese(browser, field, adb) {
  if (!(await browser.execute(element => document.activeElement === element && visualViewport.height < innerHeight * .8, field))) {
    await field.click();
    const point = await browser.execute(editorTapPoint, field);
    // 同じAPI35/320px/200%画像で確認したstatus barの106pxを含む実field位置。
    await adb('shell', 'input', 'tap', String(Math.round(point.x)), String(Math.round(point.y)));
    await eventually('Japanese editor keyboard is visible', () => browser.execute(() => visualViewport.height < innerHeight * .8));
  }
  await browser.execute(element => {
    window.__pwaImeEvents = [];
    window.__pwaImeComposing = false;
    for (const type of ['compositionstart', 'compositionupdate', 'compositionend', 'keydown']) {
      element.addEventListener(type, event => {
        if (type === 'compositionstart') window.__pwaImeComposing = true;
        if (type === 'compositionend') window.__pwaImeComposing = false;
        window.__pwaImeEvents.push({ type, key: event.key, composing: event.isComposing, data: event.data });
      });
    }
  }, field);
  await adb('shell', 'input', 'keycombination', 'KEYCODE_CTRL_LEFT', 'KEYCODE_A');
  await adb('shell', 'input', 'keyevent', 'KEYCODE_DEL');
  // 実QWERTY画面の N I H O N N。変換eventや文字列をpageへ注入しない。
  for (const [x, y] of [[585, 1570], [630, 1320], [502, 1440], [710, 1320], [585, 1570], [585, 1570]]) {
    await adb('shell', 'input', 'tap', String(x), String(y));
  }
  await nativeSnapshot(adb, 'japanese-composing');
  assert.equal(await browser.execute(() => window.__pwaImeComposing), true, 'Enter is pressed during actual IME composition');
  await adb('shell', 'input', 'keyevent', 'KEYCODE_ENTER');
  assert.equal(await browser.execute(() => window.__pwaImeComposing), false, 'Enter commits the actual IME composition');
  const text = await field.getProperty('value');
  assert.equal(text.trim(), 'にほん');
  const events = await browser.execute(() => window.__pwaImeEvents);
  assert.ok(events.some(event => event.type === 'compositionstart'));
  assert.ok(events.some(event => event.type === 'compositionend'));
  await nativeSnapshot(adb, 'japanese-converted');
  return text.trim();
}

export async function pwaInputFlow({ openClient, greetNative, openSettings, openChannelDialog, dialogWith, nativeCreatesChannel, native,
  nativeLinkPost, sees, enableDeveloperMode, openDirectMessage, fixture, publicColumn, openComposer, replaceValue, browserName, adb, origin, topic, phase, unconfirmed }) {
  const browser = await openClient('pwa-input', { ice: true, beforeConsent: browserName === 'android' ? async browser => {
    const age = browser.$('[data-testid="age-attestation-checkbox"]');
    await age.waitForDisplayed({ timeout: 90_000 });
    await eventually('320px mobile viewport', () => browser.execute(() => Math.abs(innerWidth - 320) <= 1));
    for (const locale of ['ja', 'en']) {
      await browser.$('.app-consent-panel select').selectByAttribute('value', locale);
      await eventually(`initial consent ${locale}`, () => browser.execute(language => document.documentElement.lang === language, locale));
      await age.click();
      assert.equal(await age.isSelected(), true);
      await age.click();
      assert.equal(await age.isSelected(), false);
      assert.equal(await browser.$('.app-consent-actions button').isEnabled(), false);
      await browser.executeAsync(done => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 700))));
      await nativeSnapshot(adb, `initial-consent-${locale}-320-font200`);
    }
    console.log('PWA input initial consent font scale', (await adb('shell', 'settings', 'get', 'system', 'font_scale')).trim());
    assert.equal(await browser.execute(() => document.documentElement.scrollWidth > innerWidth), false);
  } : undefined });
  const dismissOverlay = async dialog => {
    if (browserName !== 'android') return browser.back();
    if (await browser.execute(() => visualViewport.height < innerHeight * .8)) {
      await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
      await eventually('overlay keyboard closes first', () => browser.execute(() => visualViewport.height >= innerHeight * .8));
      assert.equal(await dialog.isDisplayed(), true);
    }
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  };
  try {
  const hello = await greetNative(browser, 'pwa-input');
  if (browserName === 'android') {
    await installPwa(browser, adb);

  }
  if (phase !== 'input') {
    if (browserName !== 'android') {
      unconfirmed.push(`installed Android ${phase}: real OS verification runs only in the emulator`);
      return;
    }
    if (phase === 'accessibility') {
      await reviewTalkBack(browser, openSettings, adb);
      return;
    }
    const ime = await openComposer(publicColumn(browser));
    await ime.waitForDisplayed();
    await configureJapanese(browser, ime, adb);
    // 追加だけではEnglishのままなので、実globe keyで日本語へ切り替える。
    await adb('shell', 'input', 'tap', '250', '1700');
    await nativeSnapshot(adb, 'japanese-keyboard');
    const { pubkey: peer } = await fixture('/fixture/info');
    const list = kind => webInvoke(browser, kind === 'dm' ? 'list_direct_message_messages' : kind === 'reply' ? 'list_thread' : 'list_timeline', {
      request: { cursor: null, limit: 50, ...(kind === 'dm' ? { pubkey: peer } : kind === 'reply'
        ? { topic, thread_id: hello.webPost.object_id } : { topic, scope: { kind: 'public' } }) },
    });
    for (const kind of ['post', 'reply', 'dm']) {
      let field;
      let submit;
      if (kind === 'post') { field = ime; submit = publicColumn(browser).$('button=Post'); }
      else if (kind === 'reply') {
        await browser.$(`article*=${hello.fromWeb}`).$('button[aria-label="Reply"]').click();
        field = await browser.$('textarea[placeholder="Write a reply"]');
        submit = browser.$('button=Reply');
      } else {
        await native('follow_author', { request: { pubkey: hello.webPost.author_pubkey } });
        await openDirectMessage(browser, hello.fromNative, peer);
        const column = browser.$(`section[data-column-id^="column:conversation:"][data-column-id$="${peer}"]`);
        await column.$('button[aria-label^="Message to "]').click();
        field = await column.$('textarea[placeholder="Write a message"]');
        submit = column.$('button=Send');
      }
      await field.waitForDisplayed();
      const before = (await list(kind)).items.map(item => item.object_id ?? item.message_id);
      const text = await typeJapanese(browser, field, adb);
      assert.deepEqual((await list(kind)).items.map(item => item.object_id ?? item.message_id), before, `${kind}: conversion Enter does not send`);
      await submit.click();
      await eventually(`${kind}: explicit button sends the Japanese text`, async () => (await list(kind)).items.some(item =>
        !before.includes(item.object_id ?? item.message_id) && (item.content ?? item.text).trim() === text));
      console.log(`PWA real Japanese ${kind}: composition/Enter non-send/explicit submission PASS`);
    }
    return;
  }
  const field = await openComposer(publicColumn(browser));
  await field.waitForDisplayed();
  const text = 'retained native back draft';
  if (browserName === 'android') {
    assert.equal(process.env.CI, 'true');
    await browser.execute(element => {
      window.__pwaImeEvents = [];
      for (const type of ['compositionstart', 'compositionupdate', 'compositionend', 'keydown']) {
        element.addEventListener(type, event => {
          const events = window.__pwaImeEvents;
          events.push({ type, key: event.key, composing: event.isComposing, data: event.data });
          if (events.length > 24) events.shift();
        });
      }
    }, field);
    await field.click();
    await eventually('native keyboard opens', () => browser.execute(() => visualViewport.height < innerHeight * .8));
    await adb('shell', 'input', 'text', text.replaceAll(' ', '%s'));
    assert.equal(await field.getProperty('value'), text);
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
    await eventually('back closes only the keyboard', () => browser.execute(() => visualViewport.height >= innerHeight * .8));
    assert.equal(await field.getProperty('value'), text);
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  } else {
    await replaceValue(browser, field, text);
    await browser.back();
  }
  await eventually('back collapses only the composer', async () => !(await publicColumn(browser).$('textarea[placeholder="Write a post"]').isExisting()));
  assert.equal(new URL(await browser.getUrl()).origin, origin);
  assert.equal(await (await openComposer(publicColumn(browser))).getProperty('value'), text);
  await openSettings(browser, 'appearance');
  await browser.$('#shell-settings-drawer .shell-settings-close').click();
  await eventually('settings manual close', async () => await browser.$('#shell-settings-drawer').getAttribute('data-open') === 'false');
  if (browserName === 'android') await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  else await browser.back();
  await eventually('manual settings close leaves no disposed route layer', async () => !(await publicColumn(browser).$('textarea[placeholder="Write a post"]').isExisting()));
  assert.equal(await browser.$('#shell-settings-drawer').getAttribute('data-open'), 'false');
  console.log('PWA input manual settings close leaves no history step: PASS');
  assert.equal(await (await openComposer(publicColumn(browser))).getProperty('value'), text);
  await openSettings(browser, 'appearance');
  if (browserName === 'android') await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  else await browser.back();
  await eventually('back closes settings before the composer', async () => await browser.$('#shell-settings-drawer').getAttribute('data-open') === 'false');
  assert.equal(await publicColumn(browser).$('textarea[placeholder="Write a post"]').getProperty('value'), text);
  if (browserName === 'android') await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  else await browser.back();
  await eventually('the next back closes the composer without an extra settings step', async () => !(await publicColumn(browser).$('textarea[placeholder="Write a post"]').isExisting()));
  console.log('PWA input real settings Back then composer Back: PASS');
  assert.equal(await (await openComposer(publicColumn(browser))).getProperty('value'), text);
  if (browserName === 'android') {
    const url = await browser.getUrl();
    await adb('shell', 'settings', 'put', 'system', 'accelerometer_rotation', '0');
    try {
      await adb('shell', 'settings', 'put', 'system', 'user_rotation', '1');
      await eventually('phone landscape viewport', () => browser.execute(() => innerWidth > innerHeight));
      const rotated = publicColumn(browser).$('textarea[placeholder="Write a post"]');
      assert.equal(await rotated.getProperty('value'), text);
      assert.equal(await browser.getUrl(), url);
      await nativeSnapshot(adb, 'composer-landscape-font200');
    } finally {
      await adb('shell', 'settings', 'put', 'system', 'user_rotation', '0');
      await eventually('phone portrait viewport', () => browser.execute(() => innerWidth < innerHeight));
    }
    assert.equal(await publicColumn(browser).$('textarea[placeholder="Write a post"]').getProperty('value'), text);
    console.log('PWA native rotation preserves composer scope and draft: PASS');
  }
  await publicColumn(browser).$('button=Close').click();
  await browser.$('[data-testid="control-center-trigger"]').click();
  await browser.$('#shell-control-center').waitForDisplayed();
  if (browserName === 'android') await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  else await browser.back();
  await eventually('back closes the control center', async () => !(await browser.$('#shell-control-center').isDisplayed()));
  assert.equal(new URL(await browser.getUrl()).origin, origin);
  const channel = await nativeCreatesChannel('PWA input confirmation');
  const joined = async () => (await webInvoke(browser, 'list_joined_private_channels', { request: { topic, cursor: null } })).items;
  assert.equal((await joined()).some(item => item.channel_id === channel.channelId), false);
  const invite = await openChannelDialog(browser);
  await invite.$('textarea[placeholder="Paste a private channel invite, mutual grant, or mutuals+ share"]').setValue(channel.token);
  await eventually('invite confirmation is enabled', async () => {
    const button = browser.$('.ui-dialog-content').$('button=Join');
    return await button.isExisting() && button.isEnabled();
  });
  assert.equal((await joined()).some(item => item.channel_id === channel.channelId), false);
  await dismissOverlay(invite);
  await eventually('invite dialog is closed', async () => !(await browser.$('textarea[placeholder="Paste a private channel invite, mutual grant, or mutuals+ share"]').isExisting()));
  assert.equal((await joined()).some(item => item.channel_id === channel.channelId), false);
  const beforeTransfer = await webInvoke(browser, 'list_accounts');
  const transfer = await native('create_account_transfer_invite');
  await browser.$('[data-testid="account-menu-trigger"]').click();
  await browser.$('[role=menu]').$('button=Add account').click();
  await (await dialogWith(browser, 'Move between devices')).$('button=Move from another device').click();
  const target = browser.$('[data-testid="account-transfer-target"]');
  await target.waitForDisplayed();
  await target.$('input').setValue(transfer.link);
  assert.equal((await webInvoke(browser, 'get_account_transfer_status')).state, 'idle');
  await dismissOverlay(target);
  await eventually('transfer dialog is closed', async () => !(await browser.$('[data-testid="account-transfer-target"]').isExisting()));
  assert.deepEqual(await webInvoke(browser, 'list_accounts'), beforeTransfer);
  console.log('PWA invite/transfer paste and cancellation do not join/connect/import before confirmation: PASS');
  if (browserName === 'android') {
    await openSettings(browser, 'appearance');
    for (const locale of ['ja', 'en']) {
      await browser.$('#shell-settings-drawer select').selectByAttribute('value', locale);
      await eventually(`settings ${locale}`, () => browser.execute(language => document.documentElement.lang === language, locale));
      for (const theme of [0, 1]) {
        const option = (await browser.$$('#shell-settings-drawer [role="radio"]'))[theme];
        await option.click();
        assert.equal(await option.getAttribute('aria-checked'), 'true');
        await eventually('selected theme reaches the rendered document', () => browser.execute(expected => document.documentElement.dataset.theme === expected, theme === 0 ? 'dark' : 'light'));
        await browser.executeAsync(done => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 700))));
        assert.equal(await browser.execute(() => document.documentElement.scrollWidth > innerWidth), false);
        await nativeSnapshot(adb, `settings-${locale}-${theme}-320-font200`);
      }
    }
    await (await browser.$$('#shell-settings-drawer [role="radio"]'))[0].click();
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
    await eventually('back closes the settings drawer', async () => await browser.$('#shell-settings-drawer').getAttribute('data-open') === 'false');
    assert.equal(new URL(await browser.getUrl()).origin, origin);
    await enableDeveloperMode(browser);
    await openSettings(browser, 'release');
    await browser.$('#shell-settings-drawer').$('button=Export').click();
    const report = await browser.$('textarea[aria-label="Diagnostic report preview"]').getProperty('value');
    await eventually('Android downloads the diagnostic text', async () => {
      const saved = await adb('shell', 'cat', '/sdcard/Download/kukuri-diagnostics.txt').catch(() => null);
      return saved === report;
    });
    console.log('PWA native diagnostic download bytes match the explicit export: PASS');
    await browser.$('#shell-settings-drawer .shell-settings-close').click();
    const composer = await openComposer(publicColumn(browser));
    const before = await composer.getProperty('value');
    await publicColumn(browser).$('button=Choose files').click();
    await nativeSnapshot(adb, 'file-picker-before-cancel');
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
    await eventually('file picker cancellation returns to the composer', () => composer.isDisplayed());
    assert.equal(await composer.getProperty('value'), before);
    assert.equal(await publicColumn(browser).$('img[alt^="draft preview"]').isExisting(), false);
    console.log('PWA native file picker cancellation retains the text: PASS');
    const name = 'pwa-input.png';
    const local = `test-results/pwa-input/${name}`;
    await writeFile(local, new Uint8Array(await (await fetch(`${origin}/fixture/payload.png`)).arrayBuffer()));
    await adb('push', local, `/sdcard/Download/${name}`);
    await adb('shell', 'am', 'broadcast', '-a', 'android.intent.action.MEDIA_SCANNER_SCAN_FILE', '-d', `file:///sdcard/Download/${name}`);
    const choose = async () => {
      await publicColumn(browser).$('button=Choose files').click();
      await nativeTap(adb, attrs => attrs.text === 'Files', 'file provider', 3_000).catch(async () => {
        await nativeTap(adb, attrs => attrs['content-desc'] === 'More options', 'photo picker options');
        await nativeTap(adb, attrs => /^Browse/.test(attrs.text), 'browse files from the photo picker');
      });
      await nativeTap(adb, attrs => attrs.text === name, 'image file', 5_000).catch(async () => {
        await nativeTap(adb, attrs => attrs['content-desc'] === 'Show roots', 'file locations');
        await nativeTap(adb, attrs => attrs.text === 'Downloads', 'Downloads folder');
        await nativeTap(adb, attrs => attrs.text === name, 'downloaded image file');
      });
    };
    await browser.execute(() => {
      const read = Blob.prototype.arrayBuffer;
      window.__pwaRestoreFileRead = () => { Blob.prototype.arrayBuffer = read; };
      Blob.prototype.arrayBuffer = function () { return Promise.reject(new DOMException('Injected file read failure', 'NotReadableError')); };
    });
    try {
      await choose();
      await publicColumn(browser).$('form [role="alert"]').waitForDisplayed();
      assert.equal(await composer.getProperty('value'), before);
      assert.equal(await publicColumn(browser).$('img[alt^="draft preview"]').isExisting(), false);
    } finally {
      await browser.execute(() => { window.__pwaRestoreFileRead(); delete window.__pwaRestoreFileRead; });
    }
    await choose();
    await publicColumn(browser).$(`img[alt="draft preview ${name}"]`).waitForDisplayed();
    assert.equal(await browser.execute(() => {
      const form = document.querySelector('.shell-column-composer .composer');
      return form.scrollWidth > form.clientWidth + 1;
    }), false, '320px/200% composer keeps text and attachment controls within its width');
    assert.equal(await composer.getProperty('value'), before);
    await browser.execute(element => element.scrollIntoView({ block: 'center', inline: 'center' }), await publicColumn(browser).$(`img[alt="draft preview ${name}"]`));
    await browser.executeAsync(done => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 700))));
    await nativeSnapshot(adb, 'file-selected-preview');
    await publicColumn(browser).$('.draft-attachment-list').$('button=Remove').click();
    console.log('PWA native file selection/read failure/retry retains text and accepts the explicit image: PASS');
  }
  const external = await nativeLinkPost(topic, 'pwa-mobile-external');
  await sees(browser, external.content);
  const pwaWindow = await browser.getWindowHandle();
  await browser.$(`article*=${external.content}`).$('a.smart-external-link').click();
  await eventually('external HTTP(S) link opens the expected browser destination', async () => {
    for (const handle of await browser.getWindowHandles()) {
      if (handle === pwaWindow) continue;
      await browser.switchToWindow(handle);
      if ((await browser.getUrl()).startsWith(external.url)) return true;
    }
    return false;
  });
  if (browserName === 'android') {
    await nativeSnapshot(adb, 'external-browser');
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  } else await browser.closeWindow();
  await browser.switchToWindow(pwaWindow);
  await eventually('return to the PWA document', () => browser.execute(() => document.visibilityState === 'visible'));
  assert.equal(new URL(await browser.getUrl()).origin, origin);
  assert.equal(await (await openComposer(publicColumn(browser))).getProperty('value'), text);
  await publicColumn(browser).$('button=Close').click();
  console.log('PWA external HTTP(S) URL and return preserve origin/scope/draft: PASS');
  await openSettings(browser, 'safety');
  await browser.$('#shell-settings-drawer').$('button=Open muted users').click();
  await eventually('settings handoff reaches muted connections', async () => (await browser.getUrl()).includes('connectionsView=muted'));
  await browser.executeAsync(done => setTimeout(done, 200));
  assert.ok((await browser.getUrl()).includes('connectionsView=muted'));
  assert.equal(await browser.$('#shell-settings-drawer').getAttribute('data-open'), 'false');
  console.log('PWA input composer/control center back and retained draft: PASS');
  } finally {
    if (browserName === 'android') {
      console.log('PWA real IME events', await browser.execute(() => window.__pwaImeEvents ?? []));
      await nativeSnapshot(adb, 'final-state');
    }
  }
}

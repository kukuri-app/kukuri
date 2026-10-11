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

export async function pwaInputFlow({ openClient, greetNative, openSettings, openChannelDialog, dialogWith, nativeCreatesChannel, native,
  nativeLinkPost, sees, enableDeveloperMode, publicColumn, openComposer, replaceValue, browserName, adb, origin, topic }) {
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
  await greetNative(browser, 'pwa-input');
  if (browserName === 'android') {
    await installPwa(browser, adb);
    console.log('PWA input emulator IMEs', await adb('shell', 'ime', 'list', '-s'));
    console.log('PWA input emulator accessibility', await adb('shell', 'pm', 'list', 'packages', 'com.google.android.marvin.talkback'));

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
    await nativeTap(adb, attrs => attrs.text === 'Search language' || attrs['content-desc'] === 'Search language', 'search Gboard languages');
    await adb('shell', 'input', 'text', 'Japanese');
    await nativeTap(adb, attrs => attrs.text === 'Japanese', 'Japanese language');
    await nativeTap(adb, attrs => attrs.text === 'QWERTY', 'Japanese QWERTY');
    await nativeTap(adb, attrs => attrs.text === 'Done', 'save Japanese keyboard');
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
    await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
    await field.click();
    await nativeSnapshot(adb, 'gboard-configured');
    await adb('shell', 'uiautomator', 'dump', '/sdcard/kukuri-input-keyboard.xml');
    const keyboard = await adb('shell', 'cat', '/sdcard/kukuri-input-keyboard.xml');
    const keys = [...keyboard.matchAll(/<node\s[^>]+/g)].map(item => Object.fromEntries([...item[0].matchAll(/([\w-]+)="([^"]*)"/g)].map(match => [match[1], match[2]])))
      .filter(attrs => attrs.package === 'com.google.android.inputmethod.latin').map(attrs => ({ text: attrs.text, description: attrs['content-desc'], id: attrs['resource-id'], bounds: attrs.bounds })).slice(0, 50);
    console.log('PWA input Japanese Gboard controls', JSON.stringify(keys));
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
  await invite.$('button=Join').waitForEnabled();
  assert.equal((await joined()).some(item => item.channel_id === channel.channelId), false);
  await dismissOverlay(invite);
  await invite.waitForDisplayed({ reverse: true });
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
  await target.waitForDisplayed({ reverse: true });
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
      await nativeTap(adb, attrs => attrs.text === 'Files', 'file provider');
      await nativeTap(adb, attrs => attrs.text === name, 'image file', 5_000).catch(async () => {
        await nativeTap(adb, attrs => attrs['content-desc'] === 'Show roots', 'file locations');
        await nativeTap(adb, attrs => attrs.text === 'Downloads', 'Downloads folder');
        await nativeTap(adb, attrs => attrs.text === name, 'downloaded image file');
      });
    };
    await browser.execute(() => {
      const read = FileReader.prototype.readAsDataURL;
      window.__pwaRestoreFileRead = () => { FileReader.prototype.readAsDataURL = read; };
      FileReader.prototype.readAsDataURL = function () { throw new DOMException('Injected file read failure', 'NotReadableError'); };
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
    assert.equal(await composer.getProperty('value'), before);
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
  if (browserName === 'android') {
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
      await nativeSnapshot(adb, 'talkback-start');
      for (let step = 1; step <= 3; step += 1) {
        await adb('shell', 'input', 'swipe', '200', '700', '600', '700', '150');
        await nativeSnapshot(adb, `talkback-focus-${step}`);
      }
      await adb('shell', 'uiautomator', 'dump', '/sdcard/kukuri-input-accessibility.xml');
      const tree = await adb('shell', 'cat', '/sdcard/kukuri-input-accessibility.xml');
      console.log('PWA SDK TalkBack native labels', JSON.stringify([...tree.matchAll(/(?:text|content-desc)="([^"]+)"/g)].map(match => match[1]).slice(0, 45)));
    } finally {
      await adb('shell', 'settings', 'put', 'secure', 'accessibility_enabled', '0');
      await adb('shell', 'settings', 'delete', 'secure', 'enabled_accessibility_services');
    }
    await browser.$('#shell-settings-drawer .shell-settings-close').click();
  }
  console.log('PWA input composer/control center back and retained draft: PASS');
  } finally {
    if (browserName === 'android') {
      console.log('PWA real IME events', await browser.execute(() => window.__pwaImeEvents ?? []));
      await nativeSnapshot(adb, 'final-state');
    }
  }
}

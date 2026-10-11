import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { eventually } from './wait.mjs';
import { installPwa, nativeTap } from './pwa-entry.mjs';

async function nativeSnapshot(adb, name) {
  await mkdir('test-results/pwa-input', { recursive: true });
  const file = `/sdcard/kukuri-input-${name}.png`;
  await adb('shell', 'screencap', '-p', file);
  await adb('pull', file, `test-results/pwa-input/${name}.png`);
}

export async function pwaInputFlow({ openClient, greetNative, publicColumn, openComposer, replaceValue, browserName, adb, origin }) {
  const browser = await openClient('pwa-input', { ice: true });
  await greetNative(browser, 'pwa-input');
  if (browserName === 'android') {
    await installPwa(browser, adb);
    console.log('PWA input emulator IMEs', await adb('shell', 'ime', 'list', '-s'));
    console.log('PWA input emulator accessibility', await adb('shell', 'pm', 'list', 'packages', 'com.google.android.marvin.talkback'));
  }
  const field = await openComposer(publicColumn(browser));
  const text = 'retained native back draft';
  if (browserName === 'android') {
    assert.equal(process.env.CI, 'true');
    await field.click();
    await eventually('native keyboard opens', () => browser.execute(() => visualViewport.height < innerHeight * .8));
    await nativeSnapshot(adb, 'gboard-initial');
    // GoogleのGboard手順と同じ実UIで日本語QWERTYを追加する（入力eventを注入しない）。
    await nativeTap(adb, attrs => attrs.package === 'com.google.android.inputmethod.latin' && attrs['content-desc'] === 'Settings', 'Gboard settings');
    await nativeSnapshot(adb, 'gboard-settings');
    await nativeTap(adb, attrs => attrs.text === 'Languages', 'Gboard languages');
    await nativeTap(adb, attrs => attrs.text === 'Add keyboard', 'Gboard add keyboard');
    await nativeSnapshot(adb, 'gboard-language-list');
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
  await publicColumn(browser).$('button=Close').click();
  await browser.$('[data-testid="control-center-trigger"]').click();
  await browser.$('#shell-control-center').waitForDisplayed();
  if (browserName === 'android') await adb('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  else await browser.back();
  await eventually('back closes the control center', async () => !(await browser.$('#shell-control-center').isDisplayed()));
  assert.equal(new URL(await browser.getUrl()).origin, origin);
  console.log('PWA input composer/control center back and retained draft: PASS');
}

import assert from 'node:assert/strict';
import { eventually } from './wait.mjs';
import { installPwa } from './pwa-entry.mjs';

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

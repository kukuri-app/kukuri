import assert from 'node:assert/strict';
import { eventually } from './wait.mjs';
import { webInvoke } from './web-invoke.mjs';

// OS操作は隔離されたCIのemulatorだけ。手元の端末でこのhelperを使わない。
export async function nativeTap(adb, matches, what, timeout = 30_000) {
  assert.equal(process.env.CI, 'true', 'PWA OS actions require the isolated CI emulator');
  let lastXml = '';
  const node = await eventually(`PWA Android ${what}`, async () => {
    await adb('shell', 'uiautomator', 'dump', '/sdcard/kukuri-pwa-entry.xml');
    const xml = await adb('shell', 'cat', '/sdcard/kukuri-pwa-entry.xml');
    lastXml = xml;
    for (const item of xml.matchAll(/<node\s[^>]+/g)) {
      const attrs = Object.fromEntries([...item[0].matchAll(/([\w-]+)="([^"]*)"/g)].map(m => [m[1], m[2]]));
      const bounds = attrs.bounds?.match(/^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$/);
      if (bounds && matches(attrs)) return { bounds: bounds.slice(1).map(Number), text: attrs.text };
    }
    return null;
  }, timeout).catch(error => {
    const labels = [...lastXml.matchAll(/(?:text|content-desc)="([^"]+)"/g)].map(match => match[1]).slice(0, 30);
    throw new Error(`${error.message}; CI emulator labels: ${JSON.stringify(labels)}`);
  });
  const [left, top, right, bottom] = node.bounds;
  await adb('shell', 'input', 'tap', String(Math.floor((left + right) / 2)), String(Math.floor((top + bottom) / 2)));
  return node.text;
}

async function accounts(browser) {
  return webInvoke(browser, 'list_accounts');
}

export async function openPwaFromHome(adb) {
  await adb('shell', 'input', 'keyevent', 'KEYCODE_HOME');
  const homeIcon = attrs => /^(kukuri|kukuri web)$/.test(attrs.text);
  await nativeTap(adb, homeIcon, 'home icon', 5_000).catch(async () => {
    const size = [...(await adb('shell', 'wm', 'size')).matchAll(/(\d+)x(\d+)/g)].at(-1);
    assert.ok(size, 'emulator display size');
    const [width, height] = size.slice(1).map(Number);
    await adb('shell', 'input', 'swipe', String(Math.floor(width * .85)), String(Math.floor(height * .5)),
      String(Math.floor(width * .15)), String(Math.floor(height * .5)), '250');
    await nativeTap(adb, homeIcon, 'adjacent home icon');
  });
}

async function nativeState(adb, what) {
  const activity = (await adb('shell', 'dumpsys', 'activity', 'activities')).split('\n')
    .filter(line => /mResumedActivity|topResumedActivity/.test(line));
  console.log(`PWA activity ${what}: ${JSON.stringify(activity)}`);
  await adb('shell', 'uiautomator', 'dump', '/sdcard/kukuri-pwa-entry.xml');
  const xml = await adb('shell', 'cat', '/sdcard/kukuri-pwa-entry.xml');
  const controls = [...xml.matchAll(/<node\s[^>]+/g)].map(item =>
    Object.fromEntries([...item[0].matchAll(/([\w-]+)="([^"]*)"/g)].map(match => [match[1], match[2]])))
    .filter(node => node.text || node['content-desc']).slice(0, 35)
    .map(node => ({ text: node.text, description: node['content-desc'], id: node['resource-id'], clickable: node.clickable }));
  console.log(`PWA native ${what}: ${JSON.stringify(controls)}`);
}

export async function pwaEntry({ openClient, openSettings, browserName, origin, adb, unconfirmed }) {
  const browser = await openClient('pwa-entry', { ice: true });
  const manifest = await browser.execute(async () => {
    const link = document.querySelector('link[rel="manifest"]');
    if (!link) throw new Error('missing web manifest');
    return { url: link.href, value: await (await fetch(link.href)).json() };
  });
  assert.equal(manifest.url, `${origin}/manifest.webmanifest`);
  assert.equal(manifest.value.id, '/');
  assert.equal(manifest.value.start_url, '/');
  assert.equal(manifest.value.scope, '/');
  assert.equal(manifest.value.display, 'standalone');
  await openSettings(browser, 'about');
  await browser.$('section[aria-label="Home screen"]').waitForDisplayed();
  await browser.$('#shell-settings-drawer .shell-settings-close').click();
  if (browserName !== 'android') {
    unconfirmed.push('installed Android PWA: OS actions run only in the Android CI emulator');
    return;
  }
  await installPwa(browser, adb);
}

export async function installPwa(browser, adb) {
  const before = await accounts(browser);
  const tab = await browser.getWindowHandle();
  assert.equal(process.env.CI, 'true', 'installed PWA verification requires the CI emulator');
  try {
    await nativeTap(adb, attrs => attrs['resource-id'] === 'com.android.chrome:id/menu_button', 'Chrome menu');
    const installItem = attrs => /^(Add to home screen|Install app|Install and create shortcut)$/i.test(attrs.text);
    const installMenu = await nativeTap(adb, installItem, 'install menu', 5_000).catch(async () => {
      // 200%文字ではChrome155のmenu下部がviewport外になる。実menuをscrollする。
      const size = [...(await adb('shell', 'wm', 'size')).matchAll(/(\d+)x(\d+)/g)].at(-1);
      assert.ok(size);
      const [width, height] = size.slice(1).map(Number);
      for (let step = 0; step < 3; step++) {
        await adb('shell', 'input', 'swipe', String(Math.floor(width * .7)), String(Math.floor(height * .8)),
          String(Math.floor(width * .7)), String(Math.floor(height * .3)), '400');
        try { return await nativeTap(adb, installItem, 'scrolled install menu', 3_000); }
        catch (error) { if (step === 2) throw error; }
      }
    });
    await nativeState(adb, 'install choices');
    await nativeTap(adb, attrs => /^(Install|Add)$/i.test(attrs.text) &&
      /\/(option_text_install|positive_button|button1)$/.test(attrs['resource-id']), 'install confirmation');
    if (installMenu === 'Install and create shortcut') {
      await nativeTap(adb, attrs => attrs['resource-id'] === 'com.android.chrome:id/positive_button' && /^Install$/i.test(attrs.text), 'app install confirmation');
    }
    await nativeState(adb, 'after Chrome confirmation');
    // shortcutの場合はlauncherがもう一度確認する。WebAPKの場合はこの確認がない。
    await nativeTap(adb, attrs => (/^(Add automatically|Add to home screen)$/i.test(attrs.text) && attrs.clickable === 'true') ||
      /\/(add_button|button1)$/.test(attrs['resource-id']), 'launcher confirmation', 5_000).catch(() => undefined);
    await nativeState(adb, 'after launcher confirmation');
    await openPwaFromHome(adb);
    await nativeState(adb, 'after home icon');
    await eventually('standalone PWA window', async () => {
      for (const handle of await browser.getWindowHandles()) {
        await browser.switchToWindow(handle);
        if (await browser.execute(() => matchMedia('(display-mode: standalone)').matches)) return true;
      }
      return false;
    }, 60_000).catch(async error => {
      await nativeState(adb, 'standalone timeout');
      const windows = [];
      for (const handle of await browser.getWindowHandles()) {
        await browser.switchToWindow(handle);
        windows.push(await browser.execute(() => ({ url: location.href, standalone: matchMedia('(display-mode: standalone)').matches })));
      }
      console.log(`PWA driver windows: ${JSON.stringify(windows)}`);
      throw error;
    });
    await eventually('PWA startup', async () => {
      const takeOver = browser.$('button=Use in this tab');
      if (await takeOver.isDisplayed()) { await takeOver.click(); return true; }
      return browser.$('[data-testid="control-center-trigger"]').isDisplayed();
    }, 90_000);
    await browser.$('[data-testid="control-center-trigger"]').waitForDisplayed({ timeout: 90_000 });
    const after = await accounts(browser);
    assert.equal(after.active_account_id, before.active_account_id);
    assert.equal(after.accounts[0].pubkey, before.accounts[0].pubkey);
    await openPwaFromHome(adb);
    await eventually('PWA document is visible', () => browser.execute(() => document.visibilityState === 'visible'));
    console.log('PWA native foreground', await browser.execute(() => ({ visibility: document.visibilityState, focus: document.hasFocus() })));
    const activity = (await adb('shell', 'dumpsys', 'activity', 'activities')).split('\n').find(line => /mResumedActivity|topResumedActivity/.test(line));
    console.log(`PWA launcher/standalone/account: PASS; ${activity?.trim()}`);
    return tab;
  } finally {
    await adb('shell', 'rm', '-f', '/sdcard/kukuri-pwa-entry.xml').catch(() => undefined);
  }
}

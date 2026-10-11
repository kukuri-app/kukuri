import assert from 'node:assert/strict';
import { eventually } from './wait.mjs';

// OS操作は隔離されたCIのemulatorだけ。手元の端末でこのhelperを使わない。
async function nativeTap(adb, matches, what, timeout = 30_000) {
  assert.equal(process.env.CI, 'true', 'PWA OS actions require the isolated CI emulator');
  let lastXml = '';
  const node = await eventually(`PWA Android ${what}`, async () => {
    await adb('shell', 'uiautomator', 'dump', '/sdcard/kukuri-pwa-entry.xml');
    const xml = await adb('shell', 'cat', '/sdcard/kukuri-pwa-entry.xml');
    lastXml = xml;
    for (const item of xml.matchAll(/<node\s[^>]+/g)) {
      const attrs = Object.fromEntries([...item[0].matchAll(/([\w-]+)="([^"]*)"/g)].map(m => [m[1], m[2]]));
      const bounds = attrs.bounds?.match(/^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$/);
      if (bounds && matches(attrs)) return bounds.slice(1).map(Number);
    }
    return null;
  }, timeout).catch(error => {
    const labels = [...lastXml.matchAll(/(?:text|content-desc)="([^"]+)"/g)].map(match => match[1]).slice(0, 30);
    throw new Error(`${error.message}; CI emulator labels: ${JSON.stringify(labels)}`);
  });
  await adb('shell', 'input', 'tap', String(Math.floor((node[0] + node[2]) / 2)), String(Math.floor((node[1] + node[3]) / 2)));
}

async function accounts(browser) {
  return browser.execute(async () => {
    const module = performance.getEntriesByType('resource').find(entry =>
      entry.name.includes('/kukuri_web_runtime-') && entry.name.endsWith('.js'));
    if (!module) throw new Error('web-runtime module was not loaded');
    return (await import(module.name)).invoke('list_accounts', {});
  });
}

async function nativeState(adb, what) {
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
  const before = await accounts(browser);
  await openSettings(browser, 'about');
  await browser.$('section[aria-label="Home screen"]').waitForDisplayed();
  await browser.$('#shell-settings-drawer .shell-settings-close').click();
  if (browserName !== 'android') {
    unconfirmed.push('installed Android PWA: OS actions run only in the Android CI emulator');
    return;
  }
  assert.equal(process.env.CI, 'true', 'installed PWA verification requires the CI emulator');
  try {
    await nativeTap(adb, attrs => attrs['resource-id'] === 'com.android.chrome:id/menu_button', 'Chrome menu');
    await nativeTap(adb, attrs => /^(Add to home screen|Install app)$/i.test(attrs.text), 'install menu');
    await nativeTap(adb, attrs => /^(Install|Add)$/i.test(attrs.text), 'install confirmation');
    await nativeState(adb, 'after Chrome confirmation');
    // shortcutの場合はlauncherがもう一度確認する。WebAPKの場合はこの確認がない。
    await nativeTap(adb, attrs => (/^(Add automatically|Add to home screen)$/i.test(attrs.text) && attrs.clickable === 'true') ||
      /\/(add_button|button1)$/.test(attrs['resource-id']), 'launcher confirmation', 5_000).catch(() => undefined);
    await nativeState(adb, 'after launcher confirmation');
    await adb('shell', 'input', 'keyevent', 'KEYCODE_HOME');
    const homeIcon = attrs => /^(kukuri|kukuri web)$/.test(attrs.text);
    await nativeTap(adb, homeIcon, 'home icon', 5_000).catch(async () => {
      // factory launcherは追加したshortcutを右隣のpageへ置くことがある。
      const size = (await adb('shell', 'wm', 'size')).match(/(\d+)x(\d+)/);
      assert.ok(size, 'emulator display size');
      const [width, height] = size.slice(1).map(Number);
      await adb('shell', 'input', 'swipe', String(Math.floor(width * .85)), String(Math.floor(height * .5)),
        String(Math.floor(width * .15)), String(Math.floor(height * .5)), '250');
      await nativeTap(adb, homeIcon, 'adjacent home icon');
    });
    await eventually('standalone PWA window', async () => {
      for (const handle of await browser.getWindowHandles()) {
        await browser.switchToWindow(handle);
        if (await browser.execute(() => matchMedia('(display-mode: standalone)').matches)) return true;
      }
      return false;
    }, 60_000);
    await eventually('PWA startup', async () => {
      const takeOver = browser.$('button=Use in this tab');
      if (await takeOver.isDisplayed()) { await takeOver.click(); return true; }
      return browser.$('[data-testid="control-center-trigger"]').isDisplayed();
    }, 90_000);
    await browser.$('[data-testid="control-center-trigger"]').waitForDisplayed({ timeout: 90_000 });
    const after = await accounts(browser);
    assert.equal(after.active_account_id, before.active_account_id);
    assert.equal(after.accounts[0].pubkey, before.accounts[0].pubkey);
    const activity = (await adb('shell', 'dumpsys', 'activity', 'activities')).split('\n').find(line => line.includes('mResumedActivity'));
    console.log(`PWA launcher/standalone/account: PASS; ${activity?.trim()}`);
  } finally {
    await adb('shell', 'rm', '-f', '/sdcard/kukuri-pwa-entry.xml').catch(() => undefined);
  }
}

import { eventually } from './wait.mjs';

export async function waitForUnusedShellCache(browser, timeout) {
  await eventually('unused prior shell cache is reclaimed', () => browser.execute(async () => {
    // reload/close直後は旧clientが最後の回収に残る。既存のhelloで回収し、応答後に観測する。
    const worker = (await navigator.serviceWorker.ready).active;
    const channel = new MessageChannel();
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { channel.port1.close(); reject(new Error('shell retention acknowledgement timed out')); }, 5000);
      channel.port1.onmessage = () => { clearTimeout(timer); channel.port1.close(); resolve(); };
      worker.postMessage({ type: 'hello', version: document.querySelector('meta[name="kukuri-shell-build"]').content }, [channel.port2]);
    });
    return (await caches.keys()).filter(name => name.startsWith('kukuri-shell-')).length === 1;
  }), timeout);
}

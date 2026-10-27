import { spawn } from 'node:child_process';
import { within } from './driver.mjs';

/// 各端末の前面表示を再現する。終了は、その画面のアプリを閉じてから呼ぶ。
export async function startVirtualDisplay() {
  const child = spawn('xvfb-run', ['-a', '-s', '-screen 0 1280x800x24', 'sh', '-c',
    'printf "%s\\n%s\\n" "$DISPLAY" "$XAUTHORITY"; exec openbox --sm-disable'],
  { detached: true, stdio: ['ignore', 'pipe', 'inherit'] });
  const exited = new Promise((resolve) => child.once('close', resolve));
  const signal = (name) => { try { process.kill(-child.pid, name); } catch { /* 終了済み */ } };
  let stopping;
  const close = () => stopping ??= (async () => {
    signal('SIGTERM');
    await within(exited, 3000).catch(() => signal('SIGKILL'));
  })();
  try {
    const env = await within(new Promise((resolve, reject) => {
      let output = '';
      child.once('error', reject);
      child.once('exit', (code) => reject(new Error(`virtual display exited: ${code}`)));
      const collect = (chunk) => {
        output += chunk;
        const lines = output.split('\n');
        if (lines.length >= 3) {
          child.stdout.off('data', collect);
          child.stdout.resume();
          // Waylandがあっても、端末ごとに用意したXvfbの画面を使う。
          resolve({ DISPLAY: lines[0], XAUTHORITY: lines[1], GDK_BACKEND: 'x11' });
        }
      };
      child.stdout.on('data', collect);
    }), 10000);
    return { env, close };
  } catch (error) { await close(); throw error; }
}

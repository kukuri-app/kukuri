// 配布と同じ build の kukuri を同じ PC で複数起動し、各画面の JS を評価する(#1800)。
// Windows は WebView2 の診断用の port(CDP)、Linux は WebKit の remote inspector を使う。
import { spawn } from 'node:child_process';
import { mkdirSync, openSync, rmSync } from 'node:fs';
import { join } from 'node:path';

import { eventually } from '../web-e2e/wait.mjs';

const WINDOWS = process.platform === 'win32';
const STARTUP_MS = 60_000;

/// 1 つの instance を起動する。`env` は instance ごとの追加の環境変数。
export async function launch({ app, root, name, port, env = {} }) {
  const dir = join(root, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, 'data'), { recursive: true });
  const log = openSync(join(dir, 'app.log'), 'w');
  const platformEnv = WINDOWS
    ? {
        WEBVIEW2_USER_DATA_FOLDER: join(dir, 'webview'),
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: [
          `--remote-debugging-port=${port}`,
          '--remote-allow-origins=*',
          '--use-fake-device-for-media-stream',
          '--auto-select-desktop-capture-source=Entire screen',
        ].join(' '),
      }
    : {
        XDG_DATA_HOME: join(dir, 'xdg-data'),
        XDG_CACHE_HOME: join(dir, 'xdg-cache'),
        XDG_CONFIG_HOME: join(dir, 'xdg-config'),
        WEBKIT_INSPECTOR_HTTP_SERVER: `127.0.0.1:${port}`,
      };
  const child = spawn(app, [], {
    env: {
      ...process.env,
      KUKURI_INSTANCE: name,
      KUKURI_APP_DATA_DIR: join(dir, 'data'),
      KUKURI_DISABLE_KEYRING: '1',
      KUKURI_BIND_ADDR: '127.0.0.1:0',
      KUKURI_ADVERTISE_HOST: '127.0.0.1',
      KUKURI_DISCOVERY_MODE: 'static_peer',
      KUKURI_DISCOVERY_SEEDS: '',
      ...platformEnv,
      ...env,
    },
    stdio: ['ignore', log, log],
  });
  const page = await (WINDOWS ? connectCdp(port) : connectWebKit(port));
  return {
    name,
    dir,
    evaluate: page.evaluate,
    /// Tauri の command を画面から呼ぶ(画面と Rust の受け渡しの経路を通す)。
    invoke: (command, args = {}) =>
      page.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`),
    async close() {
      page.close();
      if (WINDOWS) {
        spawn('taskkill', ['/F', '/T', '/PID', String(child.pid)], { stdio: 'ignore' });
      } else {
        child.kill('SIGKILL');
      }
      await new Promise((resolve) => child.once('exit', resolve));
    },
  };
}

/// WebView2(CDP)。最初の app の page に繋ぐ。
async function connectCdp(port) {
  const target = await eventually(
    'the WebView2 debugging port',
    async () => {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`)
        .then((response) => response.json())
        .catch(() => []);
      return targets.find((each) => each.type === 'page' && each.url.startsWith('http://tauri.localhost'));
    },
    STARTUP_MS
  );
  const socket = await open(target.webSocketDebuggerUrl);
  const call = rpc(socket, (message) => message);
  return {
    async evaluate(expression, { gesture = false } = {}) {
      const reply = await call('Runtime.evaluate', {
        expression: `(async () => (${expression}))()`,
        awaitPromise: true,
        returnByValue: true,
        userGesture: gesture,
      });
      return resultOf(reply);
    },
    close: () => socket.close(),
  };
}

/// WebKitGTK の remote inspector。page の target へ包んで送り、非同期の結果は `Runtime.awaitPromise` で待つ。
async function connectWebKit(port) {
  await eventually(
    'the WebKit inspector',
    () => fetch(`http://127.0.0.1:${port}/`).then(() => true, () => false),
    STARTUP_MS
  );
  const socket = await open(`ws://127.0.0.1:${port}/socket/1/1/WebPage`);
  // 読み直しで page の target が作り直されたら、新しい target へ送る。
  let targetId;
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.method === 'Target.targetCreated' && message.params.targetInfo.type === 'page') {
      targetId = message.params.targetInfo.targetId;
    }
  });
  await eventually('the WebKit page target', () => targetId, STARTUP_MS);
  const call = rpc(socket, (message) =>
    message.method === 'Target.dispatchMessageFromTarget' ? JSON.parse(message.params.message) : null
  );
  const send = (method, params) =>
    call(method, params, (id, payload) => ({
      id,
      method: 'Target.sendMessageToTarget',
      params: { targetId, message: JSON.stringify({ id, ...payload }) },
    }));
  return {
    async evaluate(expression, { gesture = false } = {}) {
      const promise = await send('Runtime.evaluate', {
        expression: `(async () => (${expression}))()`,
        emulateUserGesture: gesture,
      });
      if (promise.error || promise.result.wasThrown) return resultOf(promise);
      return resultOf(
        await send('Runtime.awaitPromise', { promiseObjectId: promise.result.result.objectId, returnByValue: true })
      );
    },
    close: () => socket.close(),
  };
}

function open(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.addEventListener('open', () => resolve(socket), { once: true });
    socket.addEventListener('error', () => reject(new Error(`cannot open ${url}`)), { once: true });
  });
}

/// 応答を id で待つ呼出し。`unwrap` は受けた message から応答を取り出し、`wrap` は送る形を作る。
function rpc(socket, unwrap) {
  let next = 0;
  const pending = new Map();
  socket.addEventListener('message', (event) => {
    const reply = unwrap(JSON.parse(event.data));
    if (reply?.id !== undefined && pending.has(reply.id)) {
      pending.get(reply.id)(reply);
      pending.delete(reply.id);
    }
  });
  return (method, params, wrap = (id, payload) => ({ id, ...payload })) =>
    new Promise((resolve) => {
      const id = ++next;
      pending.set(id, resolve);
      socket.send(JSON.stringify(wrap(id, { method, params })));
    });
}

function resultOf(reply) {
  if (reply.error) throw new Error(JSON.stringify(reply.error));
  const { result, exceptionDetails, wasThrown } = reply.result;
  if (exceptionDetails || wasThrown) {
    throw new Error(exceptionDetails?.exception?.description ?? JSON.stringify(result));
  }
  return result.value;
}

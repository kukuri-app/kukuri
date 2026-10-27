// 配布と同じ build の kukuri を同じ PC で複数起動し、各画面の JS を評価する(#1800)。
// Windows は WebView2 の診断用の port(CDP)、Linux は WebKit の remote inspector を使う。
import { execFileSync, spawn } from 'node:child_process';
import { mkdirSync, openSync, rmSync } from 'node:fs';
import { join } from 'node:path';

import { eventually } from '../web-e2e/wait.mjs';
import { audioEnv } from './audio.mjs';

const WINDOWS = process.platform === 'win32';
const STARTUP_MS = 60_000;
/// 利用者の終了と同じ経路で止まるのを待つ上限。
const STOP_MS = 15_000;

/// `promise` を `ms` まで待ち、過ぎたら失敗にする(画面の JS が応答しなくても止まらないため。#1842)。
export const within = (promise, ms) =>
  Promise.race([promise, new Promise((_, reject) => setTimeout(reject, ms, new Error(`no reply in ${ms} ms`)).unref())]);

/// process(負なら process group)へ signal を送る。既に終わっていれば何もしない。
function signal(pid, name) {
  try {
    process.kill(pid, name);
  } catch {
    // 既に終わっている。
  }
}

/// 1 つの instance を起動する。`env` は instance ごとの追加の環境変数。`fresh` が偽なら前の profile のまま起動し直す。
export async function launch({ app, root, name, port, env = {}, fresh = true }) {
  const dir = join(root, name);
  if (fresh) rmSync(dir, { recursive: true, force: true });
  mkdirSync(join(dir, 'data'), { recursive: true });
  const log = openSync(join(dir, 'app.log'), fresh ? 'w' : 'a');
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
      // log は色付けなしで書く(試験が読む)。
      NO_COLOR: '1',
      ...platformEnv,
      ...audioEnv(name),
      ...env,
    },
    stdio: ['ignore', log, log],
    // Linux は WebKit の子の process(描画・通信)ごと止めるため、process group を分ける。
    detached: !WINDOWS,
  });
  const exited = new Promise((resolve) => child.once('exit', resolve));
  const page = await (WINDOWS ? connectCdp(port) : connectWebKit(port));
  return {
    name,
    port,
    pid: child.pid,
    log: join(dir, 'app.log'),
    evaluate: page.evaluate,
    /// 画面の target へ protocol の命令を送る(webview の設定の上書きなど)。
    send: page.send,
    /// key の入力を受けるよう、この instance の window に focus を移す(Linux は window manager の下で xdotool、
    /// Windows は CDP で focus を模擬する)。
    focus() {
      if (!WINDOWS) {
        // WM_CLASS を持つのは toplevel の window だけ(GTK・WebKit の子の window は持たない)。
        execFileSync('xdotool', [
          'search', '--sync', '--onlyvisible', '--pid', String(child.pid), '--classname', '.', 'windowactivate', '--sync',
        ], { env: { ...process.env, ...env }, timeout: 10000 });
      }
    },
    /// Tauri の command を画面から呼ぶ(画面と Rust の受け渡しの経路を通す)。
    invoke: (command, args = {}) =>
      page.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`),
    /// 利用者の終了と同じ経路で止める(Linux は SIGTERM、Windows は閉じたら終了する設定にして window を閉じる)。
    /// 強制で止めると、相手は古い経路の接続が切れる(QUIC の無通信の期限、約 30 秒)まで新しく接続できない(#1816)。
    /// 止まらなければ強制で止める。
    async close() {
      if (WINDOWS) {
        await within(
          page.evaluate(`window.__TAURI_INTERNALS__.invoke('set_window_close_preference', { preference: { behavior: 'quit' } })`),
          STOP_MS
        ).catch(() => undefined);
        spawn('taskkill', ['/PID', String(child.pid)], { stdio: 'ignore' });
      } else {
        signal(child.pid, 'SIGTERM');
      }
      await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, STOP_MS))]);
      page.close();
      if (WINDOWS) {
        spawn('taskkill', ['/F', '/T', '/PID', String(child.pid)], { stdio: 'ignore' });
      } else {
        signal(-child.pid, 'SIGKILL');
      }
      await exited;
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
  const { call } = rpc(socket, (message) => message);
  // 複数の window が並ぶので、どの画面も focus があるものとして key の入力を受けさせる。
  await call('Emulation.setFocusEmulationEnabled', { enabled: true });
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
    send: call,
    close: () => socket.close(),
  };
}

/// WebKitGTK の remote inspector。page の target へ包んで送り、非同期の結果は `Runtime.awaitPromise` で待つ。
async function connectWebKit(port) {
  // inspector の一覧に page が載るのを待ち、その path へつなぐ(載る前につなぐと target の知らせが来ない)。
  const path = await eventually(
    'the WebKit inspector page',
    () =>
      fetch(`http://127.0.0.1:${port}/`)
        .then((response) => response.text())
        .then((html) => /socket\/[^'"]+\/WebPage/.exec(html)?.[0], () => null),
    STARTUP_MS
  );
  const socket = await open(`ws://127.0.0.1:${port}/${path}`);
  // 読み直しで page の target が作り直されたら、新しい target へ送る。消えた target へ送った呼出しは失敗にする。
  let targetId;
  const calls = rpc(socket, (message) =>
    message.method === 'Target.dispatchMessageFromTarget' ? JSON.parse(message.params.message) : message.error ? message : null
  );
  socket.addEventListener('message', (event) => {
    const { method, params } = JSON.parse(event.data);
    if (method === 'Target.targetCreated' && params.targetInfo.type === 'page' && !params.targetInfo.isProvisional) {
      targetId = params.targetInfo.targetId;
    }
    if (method === 'Target.didCommitProvisionalTarget') targetId = params.newTargetId;
    if (method === 'Target.targetDestroyed') calls.abandon(params.targetId);
  });
  await eventually('the WebKit page target', () => targetId, STARTUP_MS);
  const send = (method, params) => {
    const target = targetId;
    return calls.call(
      method,
      params,
      (id, payload) => ({
        id,
        method: 'Target.sendMessageToTarget',
        params: { targetId: target, message: JSON.stringify({ id, ...payload }) },
      }),
      target
    );
  };
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
    send,
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
/// `abandon(tag)` は `tag` を付けて送った呼出しを、応答の無いまま失敗にする(送り先の page が消えたとき)。
function rpc(socket, unwrap) {
  let next = 0;
  const pending = new Map();
  const settle = (id, reply) => {
    pending.get(id)?.resolve(reply);
    pending.delete(id);
  };
  socket.addEventListener('message', (event) => {
    const reply = unwrap(JSON.parse(event.data));
    if (reply?.id !== undefined) settle(reply.id, reply);
  });
  return {
    call: (method, params, wrap = (id, payload) => ({ id, ...payload }), tag = null) =>
      new Promise((resolve) => {
        const id = ++next;
        pending.set(id, { resolve, tag });
        socket.send(JSON.stringify(wrap(id, { method, params })));
      }),
    abandon(tag) {
      for (const [id, entry] of pending) {
        if (entry.tag === tag) settle(id, { id, error: { message: 'the page went away' } });
      }
    },
  };
}

function resultOf(reply) {
  if (reply.error) throw new Error(JSON.stringify(reply.error));
  const { result, exceptionDetails, wasThrown } = reply.result;
  if (exceptionDetails || wasThrown) {
    throw new Error(exceptionDetails?.exception?.description ?? JSON.stringify(result));
  }
  return result.value;
}

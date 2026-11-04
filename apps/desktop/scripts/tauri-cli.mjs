import { spawn } from 'node:child_process';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export const DEFAULT_DEV_HOST = '127.0.0.1';
export const DEFAULT_DEV_PORT = 5173;
const PORT_SCAN_ATTEMPTS = 20;
const require = createRequire(import.meta.url);
const tauriCliEntrypoint = path.resolve(
  path.dirname(require.resolve('@tauri-apps/cli/package.json')),
  'tauri.js'
);

export function buildBeforeDevCommand(host, port) {
  return `npx pnpm@10.16.1 vite --host ${host} --port ${port} --strictPort`;
}

export function buildDevUrl(host, port) {
  return `http://${host}:${port}`;
}

export function resolveDevHost(env = process.env) {
  return env.KUKURI_TAURI_DEV_HOST ?? env.TAURI_DEV_HOST ?? DEFAULT_DEV_HOST;
}

function normalizeInstance(value) {
  const trimmed = value?.trim();
  if (!trimmed) {
    return null;
  }
  return trimmed.replace(/[^A-Za-z0-9._-]+/g, '-');
}

export function parsePreferredDevPort(value) {
  if (value === undefined) {
    return DEFAULT_DEV_PORT;
  }
  const parsed = Number.parseInt(value, 10);
  if (!Number.isInteger(parsed) || parsed <= 0 || parsed > 65535) {
    return DEFAULT_DEV_PORT;
  }
  return parsed;
}

export async function isPortAvailable(host, port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once('error', () => {
      resolve(false);
    });
    server.listen({ host, port, exclusive: true }, () => {
      server.close(() => {
        resolve(true);
      });
    });
  });
}

export async function findAvailablePort(host, preferredPort, maxAttempts = PORT_SCAN_ATTEMPTS) {
  for (let offset = 0; offset < maxAttempts; offset += 1) {
    const nextPort = preferredPort + offset;
    if (nextPort > 65535) {
      break;
    }
    if (await isPortAvailable(host, nextPort)) {
      return nextPort;
    }
  }
  throw new Error(`no available dev port found from ${preferredPort} within ${maxAttempts} attempts`);
}

export function resolveCargoTargetDir(env = process.env, cwd = process.cwd()) {
  if (env.CARGO_TARGET_DIR) {
    return env.CARGO_TARGET_DIR;
  }
  const instance = normalizeInstance(env.KUKURI_INSTANCE);
  if (!instance) {
    return null;
  }
  return path.join(cwd, 'src-tauri', 'target', 'dev-instances', instance);
}

// 開発中のアプリは OS の keyring を使う。`.cargo/config.toml` の既定（無効）は試験のためのもので、
// 環境にある値を上書きしない。
export function resolveDisableKeyring(env = process.env) {
  return env.KUKURI_DISABLE_KEYRING ?? '0';
}

async function createDevConfig(host, port) {
  const tempDir = await mkdtemp(path.join(os.tmpdir(), 'kukuri-tauri-dev-'));
  const configPath = path.join(tempDir, 'tauri.dev.auto.conf.json');
  const config = {
    build: {
      beforeDevCommand: buildBeforeDevCommand(host, port),
      devUrl: buildDevUrl(host, port),
    },
  };
  await writeFile(configPath, JSON.stringify(config, null, 2));
  return { configPath, tempDir };
}

async function runTauri(args, env = process.env) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [tauriCliEntrypoint, ...args], {
      cwd: process.cwd(),
      env,
      stdio: 'inherit',
    });
    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (signal) {
        reject(new Error(`tauri exited with signal ${signal}`));
        return;
      }
      resolve(code ?? 1);
    });
  });
}

export async function main(argv = process.argv.slice(2), env = process.env) {
  if (argv[0] !== 'dev') {
    const code = await runTauri(argv, env);
    process.exitCode = code;
    return;
  }

  const host = resolveDevHost(env);
  const preferredPort = parsePreferredDevPort(env.KUKURI_TAURI_DEV_PORT);
  const port = await findAvailablePort(host, preferredPort);
  const cargoTargetDir = resolveCargoTargetDir(env, process.cwd());
  const nextEnv = {
    ...env,
    KUKURI_TAURI_DEV_HOST: host,
    KUKURI_TAURI_DEV_PORT: String(port),
    TAURI_DEV_HOST: host,
    KUKURI_DISABLE_KEYRING: resolveDisableKeyring(env),
    ...(cargoTargetDir ? { CARGO_TARGET_DIR: cargoTargetDir } : {}),
  };

  if (port !== preferredPort) {
    process.stdout.write(
      `[kukuri.desktop] dev port ${preferredPort} is busy; using ${port}\n`
    );
  }

  if (cargoTargetDir && cargoTargetDir !== env.CARGO_TARGET_DIR) {
    process.stdout.write(`[kukuri.desktop] using cargo target dir ${cargoTargetDir}\n`);
  }

  if (argv.includes('--config')) {
    const code = await runTauri(argv, nextEnv);
    process.exitCode = code;
    return;
  }

  const { configPath, tempDir } = await createDevConfig(host, port);
  try {
    const code = await runTauri(['dev', '--config', configPath, ...argv.slice(1)], nextEnv);
    process.exitCode = code;
  } finally {
    await rm(tempDir, { force: true, recursive: true });
  }
}

const entrypoint = process.argv[1]
  ? pathToFileURL(path.resolve(process.argv[1])).href
  : null;

if (entrypoint === import.meta.url) {
  main().catch((error) => {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`${message}\n`);
    process.exitCode = 1;
  });
}

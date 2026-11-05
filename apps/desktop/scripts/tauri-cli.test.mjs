// @vitest-environment node

import net from 'node:net';
import path from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import {
  buildBeforeDevCommand,
  buildDevUrl,
  findAvailablePort,
  parsePreferredDevPort,
  resolveCargoTargetDir,
  resolveDevHost,
  resolveDisableKeyring,
} from './tauri-cli.mjs';

const servers = [];

async function listenOnEphemeralPort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen({ host: '127.0.0.1', port: 0 }, () => {
      resolve();
    });
  });
  servers.push(server);
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('failed to resolve test server address');
  }
  return address.port;
}

afterEach(async () => {
  await Promise.all(
    servers.splice(0).map(
      (server) =>
        new Promise((resolve, reject) => {
          server.close((error) => {
            if (error) {
              reject(error);
              return;
            }
            resolve();
          });
        })
    )
  );
});

describe('tauri cli wrapper', () => {
  it('uses loopback host by default', () => {
    expect(resolveDevHost({})).toBe('127.0.0.1');
  });

  it('normalizes an invalid preferred port back to the default', () => {
    expect(parsePreferredDevPort('not-a-number')).toBe(5173);
    expect(parsePreferredDevPort('70000')).toBe(5173);
  });

  it('builds matching dev server command and URL', () => {
    expect(buildBeforeDevCommand('127.0.0.1', 5199)).toBe(
      'npx pnpm@10.16.1 vite --host 127.0.0.1 --port 5199 --strictPort'
    );
    expect(buildDevUrl('127.0.0.1', 5199)).toBe('http://127.0.0.1:5199');
  });

  it('falls back to another port when the preferred port is already bound', async () => {
    const occupiedPort = await listenOnEphemeralPort();
    const nextPort = await findAvailablePort('127.0.0.1', occupiedPort, 20);
    expect(nextPort).toBeGreaterThan(occupiedPort);
  });

  it('derives a per-instance cargo target dir for tauri dev', () => {
    expect(resolveCargoTargetDir({ KUKURI_INSTANCE: 'desktop-b' }, '/repo/apps/desktop')).toBe(
      path.join('/repo/apps/desktop', 'src-tauri', 'target', 'dev-instances', 'desktop-b')
    );
  });

  it('sanitizes the instance value when deriving the cargo target dir', () => {
    expect(
      resolveCargoTargetDir({ KUKURI_INSTANCE: 'desktop b/test' }, '/repo/apps/desktop')
    ).toBe(
      path.join('/repo/apps/desktop', 'src-tauri', 'target', 'dev-instances', 'desktop-b-test')
    );
  });

  it('preserves an explicit cargo target dir override', () => {
    expect(
      resolveCargoTargetDir(
        {
          CARGO_TARGET_DIR: '/custom/target',
          KUKURI_INSTANCE: 'desktop-b',
        },
        '/repo/apps/desktop'
      )
    ).toBe('/custom/target');
  });

  it('keeps the OS keyring in the dev app unless the environment disables it', () => {
    expect(resolveDisableKeyring({})).toBe('0');
    expect(resolveDisableKeyring({ KUKURI_DISABLE_KEYRING: '1' })).toBe('1');
  });
});

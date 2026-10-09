import { afterEach, describe, expect, test, vi } from 'vitest';

import {
  desktopDistributionFromEnvironment,
  usesSelfManagedUpdater,
} from './distribution';

afterEach(() => {
  vi.unstubAllEnvs();
  vi.resetModules();
});

describe('desktop distribution', () => {
  test('defaults unknown and missing values to the direct distribution', () => {
    expect(desktopDistributionFromEnvironment()).toBe('direct');
    expect(desktopDistributionFromEnvironment('preview')).toBe('direct');
    expect(desktopDistributionFromEnvironment(undefined, 'windows')).toBe('direct');
    expect(usesSelfManagedUpdater('direct')).toBe(true);
  });

  test('makes the Microsoft Store the only update owner for Store builds', () => {
    expect(desktopDistributionFromEnvironment('microsoft-store')).toBe('microsoft-store');
    expect(desktopDistributionFromEnvironment('microsoft-store', 'windows')).toBe('microsoft-store');
    expect(usesSelfManagedUpdater('microsoft-store')).toBe(false);
  });

  // #1199 AC-3: Android は Google Play だけで配る（#1193 D3）。
  test('makes Google Play the only update owner for Android builds', () => {
    expect(desktopDistributionFromEnvironment(undefined, 'android')).toBe('google-play');
    expect(desktopDistributionFromEnvironment('microsoft-store', 'android')).toBe('google-play');
    expect(usesSelfManagedUpdater('google-play')).toBe(false);
  });

  // Tauri CLI は Android の frontend の build（beforeBuildCommand）に `TAURI_ENV_PLATFORM=android` を渡す。
  test('reads the platform that the Tauri CLI passes to the frontend build', async () => {
    vi.stubEnv('TAURI_ENV_PLATFORM', 'android');
    // 先頭の import が読んだ module を捨て、env を変えた後に読み直す。
    vi.resetModules();
    const { DESKTOP_DISTRIBUTION } = await import('./distribution');
    const { RELEASE_CHANNEL } = await import('./releaseReadiness');
    expect(DESKTOP_DISTRIBUTION).toBe('google-play');
    expect(RELEASE_CHANNEL).toBe('google-play');
  });
});

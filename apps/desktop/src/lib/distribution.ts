export type DesktopDistribution = 'direct' | 'microsoft-store' | 'google-play';

/// Android は Google Play だけで配る（#1193 D3）。backend の `app_update.rs` も同じ build の対象で更新を拒否する。
export function desktopDistributionFromEnvironment(
  value?: string,
  platform?: string
): DesktopDistribution {
  if (platform === 'android') return 'google-play';
  return value === 'microsoft-store' ? 'microsoft-store' : 'direct';
}

export const DESKTOP_DISTRIBUTION = desktopDistributionFromEnvironment(
  import.meta.env.VITE_KUKURI_DISTRIBUTION,
  import.meta.env.TAURI_ENV_PLATFORM
);

export function usesSelfManagedUpdater(
  distribution: DesktopDistribution = DESKTOP_DISTRIBUTION
): boolean {
  return distribution === 'direct';
}

import type { CommunityNodeConfig, CommunityNodeNodeStatus } from './types';
import {
  eligibleCommunityIndexNodes,
  type CommunityIndexManifestEntry,
  type CommunityIndexNodePreference,
} from './communityIndex';

export type CommunityNodeAvailabilityReason =
  | 'ready' | 'checking' | 'noNodes' | 'statusUnavailable' | 'consentRequired'
  | 'reconsentRequired' | 'connecting' | 'retrying' | 'inviteRequired' | 'admissionDenied'
  | 'manifestError' | 'manifestAbsent' | 'indexNotProvided' | 'authRequired' | 'connectionFailed';

export type CommunityNodeAvailability = {
  reason: CommunityNodeAvailabilityReason;
  baseUrl: string | null;
  manual: boolean;
  retryAfter: number | null;
  recovery: 'status' | 'metadata' | 'manifest' | 'consent' | 'settings' | null;
};

export function hasActiveCommunityNodeConsent(status: CommunityNodeNodeStatus): boolean {
  return Boolean(status.local_consent?.records.length && status.local_consent.withdrawn_at == null);
}

// 「全Node未同意」は検索機能や到達性と独立。未取得/壊れたlocal stateは未知として保留する。
export function firstUnconsentedCommunityNode(
  config: CommunityNodeConfig,
  statuses: readonly CommunityNodeNodeStatus[],
  loaded: boolean
): string | null {
  if (!loaded || config.nodes.length === 0) return null;
  const byUrl = new Map(statuses.map((status) => [status.base_url, status]));
  for (const node of config.nodes) {
    const status = byUrl.get(node.base_url);
    if (!status || !Array.isArray(status.local_consent?.records) ||
      hasActiveCommunityNodeConsent(status)) return null;
  }
  return config.nodes[0].base_url;
}

// #1420: 同意済みで、規約の更新により再同意を待つNode。一覧の順。
export function reconsentPendingCommunityNodes(
  config: CommunityNodeConfig,
  statuses: readonly CommunityNodeNodeStatus[]
): string[] {
  const byUrl = new Map(statuses.map((status) => [status.base_url, status]));
  return config.nodes.map((node) => node.base_url).filter((baseUrl) => {
    const status = byUrl.get(baseUrl);
    return Boolean(status?.consent_update_pending && hasActiveCommunityNodeConsent(status));
  });
}

export function communityIndexAvailability({
  config, statuses, manifests, preference, configLoaded, statusesLoaded, statusError,
}: {
  config: CommunityNodeConfig;
  statuses: readonly CommunityNodeNodeStatus[];
  manifests: Readonly<Record<string, CommunityIndexManifestEntry>>;
  preference: CommunityIndexNodePreference;
  configLoaded: boolean;
  statusesLoaded: boolean;
  statusError: boolean;
}): CommunityNodeAvailability {
  const eligible = eligibleCommunityIndexNodes(config, statuses, manifests);
  const manual = preference.mode === 'manual';
  const baseUrl = manual ? preference.baseUrl : eligible[0] ?? config.nodes[0]?.base_url ?? null;
  const status = statuses.find((item) => item.base_url === baseUrl);
  const result = (
    reason: CommunityNodeAvailabilityReason, recovery: CommunityNodeAvailability['recovery']
  ): CommunityNodeAvailability => ({ reason, recovery, baseUrl, manual, retryAfter: status?.retry_after ?? null });
  if (statusError) return result('statusUnavailable', 'status');
  if (!configLoaded) return result('checking', null);
  if (!baseUrl || config.nodes.length === 0) return result('noNodes', 'settings');
  if (!statusesLoaded || !status) return result('checking', 'status');
  if (eligible.includes(baseUrl)) return result('ready', null);
  if (status.consent_update_pending) return result('reconsentRequired', 'consent');
  if (status.local_consent && !hasActiveCommunityNodeConsent(status)) {
    return result('consentRequired', 'consent');
  }
  if (status.admission_rejection || status.session_phase === 'awaiting_admission') {
    return result(status.admission_rejection?.code.startsWith('INVITE_')
      ? 'inviteRequired' : 'admissionDenied', 'settings');
  }
  if (status.session_phase === 'retrying') return result('retrying', 'metadata');
  if (['connecting', 'authenticating', 'accepting', 'refreshing'].includes(status.session_phase ?? '')) {
    return result('connecting', null);
  }
  if (status.last_error) return result('connectionFailed', 'metadata');
  if (!status.auth_state.authenticated) return result('authRequired', 'metadata');
  if (!status.consent_state?.all_required_accepted) return result('reconsentRequired', 'consent');
  const manifest = manifests[baseUrl];
  if (!manifest || manifest.status === 'loading') return result('checking', 'manifest');
  if (manifest.status === 'error') return result('manifestError', 'manifest');
  if (manifest.status === 'absent') return result('manifestAbsent', 'manifest');
  return result('indexNotProvided', 'settings');
}

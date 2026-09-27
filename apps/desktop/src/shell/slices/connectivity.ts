import type {
  CommunityNodeConfig,
  CommunityNodeManifest,
  CommunityNodeNodeStatus,
  CommunityNodePolicyDocument,
  DiscoveryConfig,
  SyncStatus,
} from '@/lib/api';
import type { CommunityIndexNodePreference } from '@/lib/api/communityIndex';

/// 接続まわり(discovery / community node / sync 状態)(WP-H6 PR3 のドメインスライス)。

export type CommunityNodeDraftNode = {
  id: string;
  base_url: string;
  /// #1056: この node の content advisory を採用するか。未指定は保存済みの値(新規は採用)。
  content_advisory_enabled?: boolean;
};

// public manifest endpoint (#356) からの取得状態。base_url ごとに保持する。
export type CommunityNodeManifestEntry =
  | { status: 'loading' }
  | { status: 'ok'; manifest: CommunityNodeManifest }
  | { status: 'absent' }
  | { status: 'error'; error: string };

// #857: 認証不要の公開 policy カタログ(GET /v1/policies)の取得状態。base_url ごとに保持する。
export type CommunityNodePoliciesEntry =
  | { status: 'loading' }
  | { status: 'ok'; policies: CommunityNodePolicyDocument[] }
  | { status: 'error'; error: string };

export type SyncStatusRead = {
  loaded: boolean;
  refreshing: boolean;
  error: boolean;
};

export const INITIAL_SYNC_STATUS_READ: SyncStatusRead = {
  loaded: false, refreshing: false, error: false,
};

export type ConnectivitySliceState = {
  peerTicket: string;
  localPeerTicket: string | null;
  discoveryConfig: DiscoveryConfig;
  discoverySeedInput: string;
  discoveryEditorDirty: boolean;
  discoveryError: string | null;
  communityNodeConfig: CommunityNodeConfig;
  communityNodeConfigLoaded: boolean;
  communityNodeConfigError: string | null;
  communityNodeStatuses: CommunityNodeNodeStatus[];
  communityNodeStatusesLoaded: boolean;
  communityNodeStatusError: string | null;
  communityNodeOnboardingShownFor: string[];
  communityNodeManifests: Record<string, CommunityNodeManifestEntry>;
  communityNodePolicies: Record<string, CommunityNodePoliciesEntry>;
  communityNodeInput: CommunityNodeDraftNode[];
  communityNodeEditorDirty: boolean;
  communityNodeError: string | null;
  communityIndexNodeBaseUrl: string | null;
  communityIndexNodePreference: CommunityIndexNodePreference;
  syncStatus: SyncStatus;
  syncStatusRead: SyncStatusRead;
};

export const DEFAULT_DISCOVERY_CONFIG: DiscoveryConfig = {
  mode: 'seeded_dht',
  connect_mode: 'direct_only',
  env_locked: false,
  seed_peers: [],
};

export const DEFAULT_COMMUNITY_NODE_CONFIG: CommunityNodeConfig = {
  nodes: [],
};

export const DEFAULT_SYNC_STATUS: SyncStatus = {
  connected: false,
  delivery_state: 'Offline',
  peer_count: 0,
  pending_events: 0,
  status_detail: '',
  last_error: null,
  configured_peer_count: 0,
  subscribed_topics: [],
  active_path: 'direct_p2p',
  fallback_peer_count: 0,
  topic_diagnostics: [],
  local_author_pubkey: '',
  discovery: {
    mode: 'seeded_dht',
    connect_mode: 'direct_only',
    active_path: 'direct_p2p',
    env_locked: false,
    configured_seed_peer_count: 0,
    bootstrap_seed_peer_count: 0,
    connected_peer_count: 0,
    docs_assist_peer_count: 0,
    blob_assist_peer_count: 0,
    local_endpoint_id: '',
    last_discovery_error: null,
  },
  gossip_disabled_topics: [],
  gossip_disabled_channels: [],
};

// 差分の event を適用する(#1221 R2-D)。`delta.topic_diagnostics` は変わった topic だけを持つ。
export function applySyncStatusDelta(
  current: SyncStatus,
  delta: SyncStatus,
  removedTopics: string[]
): SyncStatus {
  const replaced = new Set([...removedTopics, ...delta.topic_diagnostics.map((topic) => topic.topic)]);
  return {
    ...delta,
    topic_diagnostics: [
      ...current.topic_diagnostics.filter((topic) => !replaced.has(topic.topic)),
      ...delta.topic_diagnostics,
    ].sort((left, right) => left.topic.localeCompare(right.topic)),
  };
}

// 読み直した状態へ、読む間に差分で変わった topic と件数を重ねる。
export function mergePulledSyncStatus(
  pulled: SyncStatus,
  baseline: SyncStatus,
  current: SyncStatus
): SyncStatus {
  if (current === baseline) return pulled;
  const before = new Map(baseline.topic_diagnostics.map((topic) => [topic.topic, topic]));
  const kept = new Set(current.topic_diagnostics.map((topic) => topic.topic));
  return applySyncStatusDelta(
    pulled,
    { ...current, topic_diagnostics: current.topic_diagnostics.filter((topic) => before.get(topic.topic) !== topic) },
    [...before.keys()].filter((topic) => !kept.has(topic))
  );
}

export function createInitialConnectivitySlice(): ConnectivitySliceState {
  return {
    peerTicket: '',
    localPeerTicket: null,
    discoveryConfig: DEFAULT_DISCOVERY_CONFIG,
    discoverySeedInput: '',
    discoveryEditorDirty: false,
    discoveryError: null,
    communityNodeConfig: DEFAULT_COMMUNITY_NODE_CONFIG,
    communityNodeConfigLoaded: false,
    communityNodeConfigError: null,
    communityNodeStatuses: [],
    communityNodeStatusesLoaded: false,
    communityNodeStatusError: null,
    communityNodeOnboardingShownFor: [],
    communityNodeManifests: {},
    communityNodePolicies: {},
    communityNodeInput: [],
    communityNodeEditorDirty: false,
    communityNodeError: null,
    communityIndexNodeBaseUrl: null,
    communityIndexNodePreference: { mode: 'auto' },
    syncStatus: DEFAULT_SYNC_STATUS,
    syncStatusRead: INITIAL_SYNC_STATUS_READ,
  };
}

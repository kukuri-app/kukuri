import { useMemo } from 'react';
import { connectivityGuidance, discoveryGuidance } from '@/shell/connectivityGuidance';
import { diagnosticErrorLabel, diagnosticStatusDetail, diagnosticValueLabel } from '@/shell/diagnosticLabels';
import { eligibleDistanceOptoutNodes } from '@/lib/api/communityIndex';

import { buildCommunityNodeDependencyView } from '@/components/settings/communityNodeDependency';
import type {
  AppearancePanelView,
  CommunityNodePanelView,
  ConnectivityPanelView,
  DiscoveryPanelView,
  ReactionsPanelView,
} from '@/components/settings/types';
import type { TopicSyncStatus } from '@/lib/api';
import type { SupportedLocale } from '@/i18n';
import type { DesktopTheme } from '@/lib/theme';
import {
  communityNodeAuthLabel,
  communityNodeConnectivityUrlsLabel,
  communityNodeConsentLabel,
  communityNodeConsentView,
  communityNodeNextStepLabel,
  communityNodeRetryAfterLabel,
  communityNodeSessionPhaseLabel,
  communityNodeSessionActivationLabel,
  formatCount,
  formatLastReceivedLabel,
} from '@/shell/presentation';
import type { DesktopShellState } from '@/shell/store';

type UseSettingsViewModelsArgs = {
  bookmarkedReactionAssets: DesktopShellState['bookmarkedReactionAssets'];
  communityNodeConfig: DesktopShellState['communityNodeConfig'];
  communityNodeEditorDirty: DesktopShellState['communityNodeEditorDirty'];
  communityNodeError: DesktopShellState['communityNodeError'];
  communityNodeInput: DesktopShellState['communityNodeInput'];
  communityNodeManifests: DesktopShellState['communityNodeManifests'];
  communityNodePolicies: DesktopShellState['communityNodePolicies'];
  communityNodeStatuses: DesktopShellState['communityNodeStatuses'];
  discoveryConfig: DesktopShellState['discoveryConfig'];
  discoveryEditorDirty: DesktopShellState['discoveryEditorDirty'];
  discoveryError: DesktopShellState['discoveryError'];
  discoverySeedInput: DesktopShellState['discoverySeedInput'];
  error: DesktopShellState['error'];
  locale: SupportedLocale;
  localPeerTicket: DesktopShellState['localPeerTicket'];
  ownedReactionAssets: DesktopShellState['ownedReactionAssets'];
  peerTicket: DesktopShellState['peerTicket'];
  reactionPanelState: DesktopShellState['reactionPanelState'];
  syncStatus: DesktopShellState['syncStatus'];
  syncStatusRead: DesktopShellState['syncStatusRead'];
  t: (key: string, options?: Record<string, unknown>) => string;
  theme: DesktopTheme;
  topicDiagnostics: Record<string, TopicSyncStatus>;
  trackedTopics: DesktopShellState['trackedTopics'];
};

const connectivityStatusDetailKeys: Record<string, string> = {
  'No peers configured': 'settings:connectivity.statusDetails.noPeersConfigured',
  'No topics subscribed locally': 'settings:connectivity.statusDetails.noTopicsSubscribedLocally',
  'Waiting for configured peers to connect':
    'settings:connectivity.statusDetails.waitingForConfiguredPeers',
  'Connected to a subset of configured peers':
    'settings:connectivity.statusDetails.connectedToSubset',
  'Connected to all configured peers': 'settings:connectivity.statusDetails.connectedToAll',
  'No peers configured for this topic':
    'settings:connectivity.statusDetails.topicNoPeersConfigured',
  'Waiting for configured peers to join this topic':
    'settings:connectivity.statusDetails.topicWaitingForConfiguredPeers',
  'Connected to a subset of configured peers for this topic':
    'settings:connectivity.statusDetails.topicConnectedToSubset',
  'Connected to all configured peers for this topic':
    'settings:connectivity.statusDetails.topicConnectedToAll',
};

function localizeConnectivityStatusDetail(
  detail: string | null | undefined,
  t: UseSettingsViewModelsArgs['t']
) {
  if (!detail) {
    return t('settings:connectivity.summaryDetailFallback');
  }
  const translationKey = connectivityStatusDetailKeys[detail];
  return translationKey ? t(translationKey) : diagnosticStatusDetail(detail, t);
}

// settings section(connectivity / appearance / discovery / community-node /
// reactions の 5 panel)の projection(Q3 T5 の分割先)。
export function useSettingsViewModels({
  bookmarkedReactionAssets,
  communityNodeConfig,
  communityNodeEditorDirty,
  communityNodeError,
  communityNodeInput,
  communityNodeManifests,
  communityNodePolicies,
  communityNodeStatuses,
  discoveryConfig,
  discoveryEditorDirty,
  discoveryError,
  discoverySeedInput,
  error,
  locale,
  localPeerTicket,
  ownedReactionAssets,
  peerTicket,
  reactionPanelState,
  syncStatus,
  syncStatusRead,
  t,
  theme,
  topicDiagnostics,
  trackedTopics,
}: UseSettingsViewModelsArgs) {
  const communityNodeStatusByBaseUrl = useMemo(
    () =>
      Object.fromEntries(communityNodeStatuses.map((status) => [status.base_url, status])) as Record<
        string,
        (typeof communityNodeStatuses)[number]
      >,
    [communityNodeStatuses]
  );

  const connectivityPanelView = useMemo<ConnectivityPanelView>(
    () => ({
      status: syncStatusRead.refreshing && !syncStatusRead.loaded ? 'loading' : 'ready',
      guidance: connectivityGuidance(syncStatus, syncStatusRead, t),
      summaryLabel: connectivityGuidance(syncStatus, syncStatusRead, t).label,
      panelError: error,
      metrics: [
        {
          label: t('settings:connectivity.metrics.connected'),
          value: syncStatus.connected ? t('common:states.yes') : t('common:states.no'),
          tone: syncStatus.connected ? 'accent' : 'warning',
        },
        {
          label: t('settings:connectivity.metrics.peers'),
          value: formatCount(syncStatus.peer_count),
        },
        {
          label: t('settings:connectivity.metrics.pending'),
          value: formatCount(syncStatus.pending_events),
          tone: syncStatus.pending_events > 0 ? 'warning' : 'default',
        },
      ],
      diagnostics: [
        {
          label: t('settings:connectivity.diagnostics.configuredPeers'),
          value: formatCount(syncStatus.configured_peer_count),
        },
        {
          label: t('settings:connectivity.diagnostics.connectionDetail'),
          value: localizeConnectivityStatusDetail(syncStatus.status_detail, t),
        },
        {
          label: t('settings:connectivity.diagnostics.effectivePeers'),
          value: '',
          monospace: true,
          peers: { kind: 'connected' },
        },
        {
          label: t('settings:connectivity.diagnostics.lastError'),
          value: diagnosticErrorLabel(syncStatus.last_error, t) ?? t('common:fallbacks.none'),
          tone: syncStatus.last_error ? 'danger' : 'default',
        },
      ],
      localPeerTicket: localPeerTicket ?? '',
      peerTicketInput: peerTicket,
      topics: trackedTopics.map((topic) => {
        const diagnostic = topicDiagnostics[topic];
        return {
          topic,
          guidance: connectivityGuidance(syncStatus, syncStatusRead, t, { id: topic, diagnostic }),
          summary: connectivityGuidance(syncStatus, syncStatusRead, t, { id: topic, diagnostic }).label,
          lastReceivedLabel: formatLastReceivedLabel(diagnostic?.last_received_at, locale),
          expectedPeerCount: diagnostic?.configured_peer_count ?? null,
          missingPeerCount: diagnostic?.missing_peer_count ?? null,
          statusDetail:
            localizeConnectivityStatusDetail(diagnostic?.status_detail, t),
          lastError: diagnosticErrorLabel(diagnostic?.last_error, t),
        };
      }),
    }),
    [
      error,
      localPeerTicket,
      locale,
      peerTicket,
      syncStatus,
      syncStatusRead,
      t,
      topicDiagnostics,
      trackedTopics,
    ]
  );

  const appearancePanelView = useMemo<AppearancePanelView>(
    () => ({
      selectedTheme: theme,
      selectedLocale: locale,
      options: [
        {
          value: 'dark',
          label: t('settings:appearance.themeOptions.dark.label'),
          description: t('settings:appearance.themeOptions.dark.description'),
        },
        {
          value: 'light',
          label: t('settings:appearance.themeOptions.light.label'),
          description: t('settings:appearance.themeOptions.light.description'),
        },
      ],
    }),
    [locale, t, theme]
  );

  const discoveryPanelView = useMemo<DiscoveryPanelView>(
    () => ({
      status: syncStatusRead.refreshing && !syncStatusRead.loaded ? 'loading' : 'ready',
      guidance: discoveryGuidance(syncStatus, syncStatusRead, t),
      summaryLabel: discoveryGuidance(syncStatus, syncStatusRead, t).label,
      panelError: null,
      metrics: [
        { label: t('settings:discovery.metrics.mode'), value: diagnosticValueLabel('discoveryMode', syncStatus.discovery.mode, t) },
        {
          label: t('settings:discovery.metrics.connect'),
          value: diagnosticValueLabel('connectMode', syncStatus.discovery.connect_mode, t),
          tone: syncStatus.discovery.connect_mode === 'direct_or_relay' ? 'accent' : 'default',
        },
        {
          label: t('settings:discovery.metrics.envLock'),
          value: discoveryConfig.env_locked ? t('common:states.yes') : t('common:states.no'),
          tone: discoveryConfig.env_locked ? 'warning' : 'default',
        },
      ],
      diagnostics: [
        {
          label: t('settings:discovery.diagnostics.localEndpointId'),
          value: syncStatus.discovery.local_endpoint_id || t('common:fallbacks.unknown'),
          monospace: true,
        },
        {
          label: t('settings:discovery.diagnostics.connectedPeers'),
          value: '',
          monospace: true,
          peers: { kind: 'connected' },
        },
        {
          label: t('settings:discovery.diagnostics.docsAssistPeers'),
          value: '',
          monospace: true,
          peers: { kind: 'docs_assist' },
        },
        {
          label: t('settings:discovery.diagnostics.blobAssistPeers'),
          value: '',
          monospace: true,
          peers: { kind: 'blob_assist' },
        },
        {
          label: t('settings:discovery.diagnostics.manualTicketPeers'),
          value: '',
          monospace: true,
          peers: { kind: 'manual_ticket' },
        },
        {
          label: t('settings:discovery.diagnostics.communityBootstrapPeers'),
          value: '',
          monospace: true,
          peers: { kind: 'bootstrap_seed' },
        },
        {
          label: t('settings:discovery.diagnostics.configuredSeedIds'),
          value: '',
          monospace: true,
          peers: { kind: 'configured_seed' },
        },
        {
          label: t('settings:discovery.diagnostics.discoveryError'),
          value: diagnosticErrorLabel(discoveryError ?? syncStatus.discovery.last_discovery_error, t) ?? t('common:fallbacks.none'),
          tone:
            discoveryError || syncStatus.discovery.last_discovery_error ? 'danger' : 'default',
        },
      ],
      seedPeersInput: discoverySeedInput,
      seedPeersMessage: discoveryConfig.env_locked
        ? t('settings:discovery.messages.viewLocked')
        : discoveryEditorDirty
          ? t('settings:discovery.messages.unsaved')
          : t('settings:discovery.messages.saved'),
      seedPeersMessageTone: discoveryConfig.env_locked ? ('default' as const) : ('default' as const),
      envLocked: discoveryConfig.env_locked,
      publicBlobDiscovery: discoveryConfig.public_blob_discovery,
    }),
    [
      discoveryConfig.env_locked,
      discoveryConfig.public_blob_discovery,
      discoveryEditorDirty,
      discoveryError,
      discoverySeedInput,
      syncStatus,
      syncStatusRead,
      t,
    ]
  );

  const communityNodePanelView = useMemo<CommunityNodePanelView>(
    () => ({
      status: 'ready' as const,
      summaryLabel: t('settings:communityNode.summary', { count: communityNodeInput.length }),
      panelError: communityNodeError,
      editorMessage: communityNodeEditorDirty
        ? t('settings:communityNode.editorMessage.unsaved')
        : t('settings:communityNode.editorMessage.saved'),
      editorMessageTone: 'default' as const,
      nodes: communityNodeInput.map((node) => {
        // 距離利用停止は索引か信頼のいずれかを提供中の適格ノードでだけ操作できる(#705)。
        const distanceOptoutEligibleBaseUrls = eligibleDistanceOptoutNodes(
          communityNodeConfig,
          communityNodeStatuses,
          communityNodeManifests
        );
        const saved =
          communityNodeConfig.nodes.find(
            (candidate) => candidate.base_url === node.base_url
          ) != null;
        const status = communityNodeStatusByBaseUrl[node.base_url];
        const manifestEntry = communityNodeManifests[node.base_url];
        return {
          id: node.id,
          baseUrl: node.base_url,
          nodeId: manifestEntry?.status === 'ok' ? manifestEntry.manifest.node_id : null,
          nodeName: manifestEntry?.status === 'ok' ? manifestEntry.manifest.node_name : null,
          saved,
          distanceOptoutEligible: distanceOptoutEligibleBaseUrls.includes(node.base_url),
          contentAdvisoryEnabled: node.content_advisory_enabled !== false,
          diagnostics: [
            {
              label: t('settings:communityNode.diagnostics.auth'),
              value: communityNodeAuthLabel(status),
            },
            {
              label: t('settings:communityNode.diagnostics.consent'),
              value: communityNodeConsentLabel(status),
            },
            {
              label: t('settings:communityNode.diagnostics.connectivityUrls'),
              value: communityNodeConnectivityUrlsLabel(status),
              monospace: true,
            },
            {
              label: t('settings:communityNode.diagnostics.sessionPhase'),
              value: communityNodeSessionPhaseLabel(status),
            },
            {
              label: t('settings:communityNode.diagnostics.retryAfter'),
              value: communityNodeRetryAfterLabel(status),
            },
            {
              label: t('settings:communityNode.diagnostics.sessionActivation'),
              value: communityNodeSessionActivationLabel(status),
            },
            {
              label: t('settings:communityNode.diagnostics.nextStep'),
              value: communityNodeNextStepLabel(status),
            },
            {
              label: t('settings:communityNode.diagnostics.lastError'),
              value: diagnosticErrorLabel(status?.last_error, t) ?? t('common:fallbacks.none'),
              tone: status?.last_error ? 'danger' : 'default',
            },
          ],
          dependency: buildCommunityNodeDependencyView(
            communityNodeManifests[node.base_url],
            t
          ),
          consent: communityNodeConsentView(status, communityNodePolicies[node.base_url]),
          inviteCodeSaved: status?.invite_code_saved ?? false,
          admissionRejectionCode: status?.admission_rejection?.code ?? null,
          lastError: diagnosticErrorLabel(status?.last_error, t),
        };
      }),
    }),
    [
      communityNodeConfig,
      communityNodeEditorDirty,
      communityNodeError,
      communityNodeInput,
      communityNodeManifests,
      communityNodePolicies,
      communityNodeStatusByBaseUrl,
      communityNodeStatuses,
      t,
    ]
  );

  const reactionsPanelView = useMemo<ReactionsPanelView>(
    () => ({
      status: reactionPanelState.status,
      summaryLabel: t('settings:reactions.summary', {
        owned: ownedReactionAssets.length,
        saved: bookmarkedReactionAssets.length,
      }),
      panelError: reactionPanelState.error,
      ownedAssets: ownedReactionAssets,
      bookmarkedAssets: bookmarkedReactionAssets,
    }),
    [
      bookmarkedReactionAssets,
      ownedReactionAssets,
      reactionPanelState.error,
      reactionPanelState.status,
      t,
    ]
  );

  return {
    communityNodeStatusByBaseUrl,
    connectivityPanelView,
    appearancePanelView,
    discoveryPanelView,
    communityNodePanelView,
    reactionsPanelView,
  };
}

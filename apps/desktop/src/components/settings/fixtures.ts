import type {
  AppearancePanelView,
  CommunityNodePanelView,
  ConnectivityPanelView,
  DiscoveryPanelView,
} from './types';
import { buildCommunityNodeDependencyView } from './communityNodeDependency';
import i18n from '@/i18n';
import { diagnosticErrorLabel, diagnosticValueLabel } from '@/shell/diagnosticLabels';
import { formatLocalizedDateTime, formatLocalizedTime, getResolvedLocale } from '@/i18n/format';

const fixtureTranslate = (key: string, options?: Record<string, unknown>): string =>
  i18n.t(key, options);

export function createAppearancePanelFixture(): AppearancePanelView {
  return {
    selectedTheme: 'dark',
    selectedLocale: getResolvedLocale(i18n.resolvedLanguage),
    options: [
      {
        value: 'dark',
        label: i18n.t('settings:appearance.themeOptions.dark.label'),
        description: i18n.t('settings:appearance.themeOptions.dark.description'),
      },
      {
        value: 'light',
        label: i18n.t('settings:appearance.themeOptions.light.label'),
        description: i18n.t('settings:appearance.themeOptions.light.description'),
      },
    ],
  };
}

export const appearancePanelFixture = createAppearancePanelFixture();

export function createConnectivityPanelFixture(): ConnectivityPanelView {
  return {
    status: 'ready',
    summaryLabel: i18n.t('common:states.connected'),
    panelError: null,
    metrics: [
      { label: i18n.t('settings:connectivity.metrics.connected'), value: i18n.t('common:states.yes'), tone: 'accent' },
      { label: i18n.t('settings:connectivity.metrics.peers'), value: '2' },
      { label: i18n.t('settings:connectivity.metrics.pending'), value: '0' },
    ],
    diagnostics: [
      {
        label: i18n.t('settings:connectivity.diagnostics.configuredPeers'),
        value: 'peer-a, peer-b',
        monospace: true,
      },
      {
        label: i18n.t('settings:connectivity.diagnostics.connectionDetail'),
        value: 'Connected to all configured peers',
      },
      {
        label: i18n.t('settings:connectivity.diagnostics.effectivePeers'),
        value: 'peer-a, relay-peer',
        monospace: true,
      },
      { label: i18n.t('settings:connectivity.diagnostics.lastError'), value: i18n.t('common:fallbacks.none') },
    ],
    localPeerTicket: 'peer1@127.0.0.1:7777',
    peerTicketInput: '',
    topics: [
      {
        topic: 'kukuri:topic:demo',
        summary: i18n.t('settings:connectivity.summary', {
          status: i18n.t('common:states.joined'),
          count: 2,
        }),
        lastReceivedLabel: formatLocalizedTime('2026-03-28T12:45:11Z'),
        expectedPeerCount: 2,
        missingPeerCount: 0,
        statusDetail: 'Connected to all configured peers for this topic',
        connectedPeersLabel: 'peer-a, peer-b',
        relayAssistedPeersLabel: 'relay-peer',
        configuredPeersLabel: 'peer-a, peer-b',
        missingPeersLabel: i18n.t('common:fallbacks.none'),
        lastError: null,
      },
      {
        topic: 'kukuri:topic:relay',
        summary: i18n.t('settings:connectivity.summary', {
          status: 'recovering',
          count: 0,
        }),
        lastReceivedLabel: i18n.t('common:fallbacks.noEvents'),
        expectedPeerCount: 0,
        missingPeerCount: 0,
        statusDetail:
          'docs-assisted recovery is in progress via 1 peer(s); live topic delivery is unavailable',
        connectedPeersLabel: i18n.t('common:fallbacks.none'),
        relayAssistedPeersLabel: 'relay-peer',
        configuredPeersLabel: i18n.t('common:fallbacks.none'),
        missingPeersLabel: i18n.t('common:fallbacks.none'),
        lastError: diagnosticErrorLabel('topic join pending: timed out waiting for initial topic join', fixtureTranslate),
      },
    ],
  };
}

export const connectivityPanelFixture = createConnectivityPanelFixture();

export function createDiscoveryPanelFixture(): DiscoveryPanelView {
  return {
    status: 'ready',
    summaryLabel: diagnosticValueLabel('discoveryMode', 'seeded_dht', fixtureTranslate),
    panelError: null,
    metrics: [
      { label: i18n.t('settings:discovery.metrics.mode'), value: diagnosticValueLabel('discoveryMode', 'seeded_dht', fixtureTranslate) },
      { label: i18n.t('settings:discovery.metrics.connect'), value: diagnosticValueLabel('connectMode', 'direct_or_relay', fixtureTranslate), tone: 'accent' },
      { label: i18n.t('settings:discovery.metrics.envLock'), value: i18n.t('common:states.no') },
    ],
    diagnostics: [
      { label: i18n.t('settings:discovery.diagnostics.localEndpointId'), value: 'local-endpoint-a', monospace: true },
      { label: i18n.t('settings:discovery.diagnostics.connectedPeers'), value: 'peer-a', monospace: true },
      { label: i18n.t('settings:discovery.diagnostics.relayAssistedPeers'), value: 'relay-peer', monospace: true },
      { label: i18n.t('settings:discovery.diagnostics.manualTicketPeers'), value: 'peer-ticket-1', monospace: true },
      { label: i18n.t('settings:discovery.diagnostics.communityBootstrapPeers'), value: 'bootstrap-peer-1', monospace: true },
      { label: i18n.t('settings:discovery.diagnostics.configuredSeedIds'), value: 'seed-peer-1', monospace: true },
      { label: i18n.t('settings:discovery.diagnostics.discoveryError'), value: i18n.t('common:fallbacks.none') },
    ],
    seedPeersInput: 'seed-peer-1\nseed-peer-2@127.0.0.1:7777',
    seedPeersMessage: 'Editing stays enabled because discovery is not env-locked.',
    seedPeersMessageTone: 'default',
    envLocked: false,
  };
}

export const discoveryPanelFixture = createDiscoveryPanelFixture();

export function createCommunityNodePanelFixture(): CommunityNodePanelView {
  return {
    status: 'ready',
    summaryLabel: i18n.t('settings:communityNode.summary', { count: 2 }),
    panelError: null,
    editorMessage: 'Save nodes before authenticating.',
    editorMessageTone: 'default',
    nodes: [
      {
        id: 'node-api',
        baseUrl: 'https://api.kukuri.app',
        saved: true,
        diagnostics: [
          { label: i18n.t('settings:communityNode.diagnostics.auth'), value: `${i18n.t('common:states.yes')} (1711324800000)` },
          { label: i18n.t('settings:communityNode.diagnostics.consent'), value: i18n.t('common:states.accepted') },
          {
            label: i18n.t('settings:communityNode.diagnostics.sessionPhase'),
            value: i18n.t('settings:communityNode.sessionPhases.ready'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.retryAfter'),
            value: i18n.t('common:fallbacks.none'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.connectivityUrls'),
            value: 'https://api.kukuri.app',
            monospace: true,
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.sessionActivation'),
            value: i18n.t('settings:communityNode.values.activeOnCurrentSession'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.nextStep'),
            value: i18n.t('settings:communityNode.values.connectivityUrlsActiveOnCurrentSession'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.lastError'),
            value: i18n.t('common:fallbacks.none'),
          },
        ],
        dependency: buildCommunityNodeDependencyView(
          {
            status: 'ok',
            manifest: {
              node_id: '',
              node_name: 'api.kukuri.app',
              node_role: 'default-onboarding-node',
              server_name: 'api.kukuri.app',
              manifest_version: 'v1',
              capability_scope: {
                available_enabled: ['auth_consent', 'bootstrap_assist', 'iroh_relay'],
                planned_enabled: ['community_index', 'moderation'],
              },
              authority_scope: {
                applies_to: ['this_node'],
                does_not_apply_to: [
                  'kukuri_network_as_a_whole',
                  'user_identity',
                  'user_profile_canonical_source',
                  'user_social_graph_canonical_source',
                ],
              },
              p2p_boundary: {
                identity_authority: false,
                profile_canonical_store: false,
                social_graph_canonical_store: false,
                content_truth_source: false,
                network_wide_authority: false,
              },
              abuse_contact: 'abuse@api.kukuri.app',
              report_endpoint: 'https://api.kukuri.app/v1/report',
              terms_url: 'https://api.kukuri.app/terms',
              privacy_url: 'https://api.kukuri.app/privacy',
              moderation_policy_url: 'https://api.kukuri.app/moderation-policy',
            },
          },
          fixtureTranslate
        ),
        consent: {
          loaded: true,
          loading: false,
          loadError: null,
          withdrawn: false,
          hasLocalConsent: true,
          allRequiredAccepted: true,
          hasPendingUpdate: false,
          policies: [
            {
              policySlug: 'terms_of_service',
              title: 'Terms of Service',
              body: 'You must follow the community node terms of service.',
              policyVersion: 1,
              policyKind: 'terms',
              referenceTranslation: false,
              fallback: false,
              required: true,
              acceptedAtLabel: formatLocalizedDateTime('2026-03-28T13:00:00Z'),
              updated: false,
              previouslyAcceptedVersion: 1,
            },
          ],
        },
        contentAdvisoryEnabled: true,
      distanceOptoutEligible: true,
        inviteCodeSaved: false,
        admissionRejectionCode: null,
        lastError: null,
      },
      {
        id: 'node-example',
        baseUrl: 'https://community.example.com',
        saved: true,
        diagnostics: [
          { label: i18n.t('settings:communityNode.diagnostics.auth'), value: i18n.t('common:states.no') },
          { label: i18n.t('settings:communityNode.diagnostics.consent'), value: i18n.t('common:states.unknown') },
          {
            label: i18n.t('settings:communityNode.diagnostics.sessionPhase'),
            value: i18n.t('settings:communityNode.sessionPhases.retrying'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.retryAfter'),
            value: formatLocalizedTime('2026-03-28T13:00:00Z'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.connectivityUrls'),
            value: i18n.t('settings:communityNode.values.notResolved'),
            monospace: true,
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.sessionActivation'),
            value: i18n.t('settings:communityNode.values.notAuthenticated'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.nextStep'),
            value: i18n.t('settings:communityNode.values.authenticateThisNode'),
          },
          {
            label: i18n.t('settings:communityNode.diagnostics.lastError'),
            value: i18n.t('common:errors.failedToRefreshCommunityNode'),
            tone: 'danger',
          },
        ],
        dependency: buildCommunityNodeDependencyView({ status: 'absent' }, fixtureTranslate),
        consent: {
          loaded: false,
          loading: false,
          loadError: null,
          withdrawn: false,
          hasLocalConsent: false,
          allRequiredAccepted: false,
          hasPendingUpdate: false,
          policies: [],
        },
        distanceOptoutEligible: true,
        inviteCodeSaved: false,
        admissionRejectionCode: null,
        lastError: i18n.t('common:errors.failedToRefreshCommunityNode'),
      },
    ],
  };
}

export const communityNodePanelFixture = createCommunityNodePanelFixture();

import {
  type BlobMediaPayload,
  type CommunityNodeIndexQueryRequest,
  type DesktopApi,
  type IndexQueryResponse,
  type IndexingRequestView,
  type IndexingStatusResponse,
  type SubmitIndexingRequestResponse,
} from '@/lib/api';

import { cloneSyncStatus } from '../desktopMockModel';
import { type MockRuntime } from '../mockRuntime';

type ConnectivityMock = Pick<
  DesktopApi,
  | 'getSyncStatus'
  | 'getDiscoveryConfig'
  | 'getCommunityNodeConfig'
  | 'getCommunityNodeStatuses'
  | 'setCommunityNodeConfig'
  | 'clearCommunityNodeConfig'
  | 'authenticateCommunityNode'
  | 'setCommunityNodeInviteCode'
  | 'clearCommunityNodeToken'
  | 'fetchCommunityNodePolicies'
  | 'acceptCommunityNodeConsents'
  | 'withdrawCommunityNodeConsents'
  | 'refreshCommunityNodeMetadata'
  | 'fetchCommunityNodeManifest'
  | 'lookupCommunityNodeContentAdvisories'
  | 'readCommunityNodeTrustUser'
  | 'readCommunityNodeRelationUser'
  | 'listCommunityNodeRelationNeighbors'
  | 'evaluateAuthorTrustGates'
  | 'setAuthorTrustDisplayException'
  | 'listAuthorTrustDisplayExceptions'
  | 'getCommunityNodeObservationSharing'
  | 'enableCommunityNodeObservationSharing'
  | 'disableCommunityNodeObservationSharing'
  | 'getCommunityNodeRelationOptout'
  | 'setCommunityNodeRelationOptout'
  | 'clearCommunityNodeRelationOptout'
  | 'searchCommunityNodeIndex'
  | 'discoverCommunityNodeIndex'
  | 'recommendCommunityNodeIndex'
  | 'submitCommunityNodeIndexingRequest'
  | 'revokeCommunityNodeIndexingRequest'
  | 'readCommunityNodeIndexingStatus'
  | 'submitCommunityNodeReport'
  | 'submitCommunityNodeTesterFeedback'
  | 'importPeerTicket'
  | 'setDiscoverySeeds'
  | 'unsubscribeTopic'
  | 'setTopicGossipEnabled'
  | 'setChannelGossipEnabled'
  | 'getLocalPeerTicket'
  | 'getBlobMediaPayload'
  | 'getBlobPreviewUrl'
  | 'getContentDisplaySettings'
  | 'setAdultContentDisplayEnabled'
>;

function observationSharingStatus(baseUrl: string, enabled: boolean) {
  return {
    base_url: baseUrl,
    offered: true,
    policy: null,
    enabled,
    needs_reconsent: false,
    revocation_pending: false,
    pending_count: 0,
  };
}

export function createConnectivityMock(runtime: MockRuntime): ConnectivityMock {
  let adultContentDisplayEnabled = false;
  const {
    syncStatus,
    postsByTopic,
    liveSessionsByTopic,
    gameRoomsByTopic,
    joinedChannelsByTopic,
    metaverseAssetPayloads,
    mockConsentItems,
  } = runtime;

  function queryIndex(request: CommunityNodeIndexQueryRequest): IndexQueryResponse {
    const query = request.query?.trim().toLocaleLowerCase() ?? '';
    const entries = Object.entries(postsByTopic)
      .flatMap(([topic, posts]) =>
        posts.map((post) => ({
          scope_kind: post.channel_id ? ('private_channel' as const) : ('public_topic' as const),
          scope_id: post.channel_id ?? topic,
          object_id: post.object_id,
          author_pubkey: post.author_pubkey,
          text: post.content,
          created_at: post.created_at,
          // #1054: mock の投稿には CN の content advisory は付かない（表示側は C3 で扱う）。
          content_advisories: [],
        }))
      )
      .filter(
        (entry) =>
          (!request.scope_kind ||
            (entry.scope_kind === request.scope_kind && entry.scope_id === request.scope_id)) &&
          (!query || entry.text.toLocaleLowerCase().includes(query))
      )
      .sort((left, right) => right.created_at - left.created_at)
      .slice(0, request.limit ?? 20);
    return { entries };
  }

  const relationOptoutNodes = new Set<string>();
  const observationSharingNodes = new Set<string>();
  const trustDisplayExceptions = new Set<string>();
  // #975: node 別の索引申請簿。submit した申請を status 読取りが返す(mock 内 memory のみ)。
  const indexingRequestsByNode = new Map<string, IndexingRequestView[]>();
  // 索引対象は seed に依存せず固定する(Storybook の確認面用)。browser seed の general は対象外のままにし、
  // 申請 → pending → 承認前の流れを既存 spec で保つ。
  const supportedPublicTopics = new Set<string>(['kukuri:topic:demo']);

  return {
    async getSyncStatus() {
      return cloneSyncStatus(syncStatus);
    },
    async getDiscoveryConfig() {
      return runtime.discoveryConfig;
    },
    async getCommunityNodeConfig() {
      return runtime.communityNodeConfig;
    },
    async getCommunityNodeStatuses() {
      return runtime.communityNodeStatuses;
    },
    async setCommunityNodeConfig(nodes, trustNodePriority) {
      const previousNodes = new Map(runtime.communityNodeConfig.nodes.map((node) => [node.base_url, node]));
      const previousStatuses = new Map(runtime.communityNodeStatuses.map((status) => [status.base_url, status]));
      runtime.communityNodeConfig = {
        trust_node_priority: (
          trustNodePriority ??
          runtime.communityNodeConfig.trust_node_priority ??
          []
        ).filter((baseUrl) => nodes.some((node) => node.base_url === baseUrl)),
        nodes: nodes.map((node) => {
          const previous = previousNodes.get(node.base_url);
          const contentAdvisoryEnabled =
            node.content_advisory_enabled ?? previous?.content_advisory_enabled ?? true;
          return previous
            ? { ...previous, content_advisory_enabled: contentAdvisoryEnabled }
            : {
                base_url: node.base_url,
                resolved_urls: null,
                content_advisory_enabled: contentAdvisoryEnabled,
              };
        }),
      };
      runtime.communityNodeStatuses = nodes.map((node) => previousStatuses.get(node.base_url) ?? ({
        base_url: node.base_url,
        auth_state: { authenticated: false, expires_at: null },
        consent_state: null,
        local_consent: { records: [], withdrawn_at: null },
        consent_update_pending: false,
        resolved_urls: null,
        last_error: null,
        invite_code_saved: false,
        admission_rejection: null,
        session_phase: 'idle',
        retry_after: null,
        restart_required: false,
      }));
      return runtime.communityNodeConfig;
    },
    async clearCommunityNodeConfig() {
      runtime.communityNodeConfig = { nodes: [] };
      runtime.communityNodeStatuses = [];
    },
    async authenticateCommunityNode(baseUrl) {
      runtime.communityNodeStatuses = runtime.communityNodeStatuses.map((status) =>
        status.base_url === baseUrl
          ? {
              ...status,
              auth_state: { authenticated: true, expires_at: Date.now() },
              consent_state: { all_required_accepted: false, items: mockConsentItems(false) },
              session_phase: 'authenticating',
            }
          : status
      );
      return runtime.communityNodeStatuses.find((status) => status.base_url === baseUrl)!;
    },
    async setCommunityNodeInviteCode(baseUrl, inviteCode) {
      runtime.communityNodeStatuses = runtime.communityNodeStatuses.map((status) =>
        status.base_url === baseUrl
          ? {
              ...status,
              invite_code_saved: Boolean(inviteCode?.trim()),
              admission_rejection: null,
              auth_state: { authenticated: true, expires_at: Date.now() },
              session_phase: 'authenticating',
            }
          : status
      );
      return runtime.communityNodeStatuses.find((status) => status.base_url === baseUrl)!;
    },
    async clearCommunityNodeToken(baseUrl) {
      runtime.communityNodeStatuses = runtime.communityNodeStatuses.map((status) =>
        status.base_url === baseUrl
          ? {
              ...status,
              auth_state: { authenticated: false, expires_at: null },
              consent_state: null,
              session_phase: 'idle',
            }
          : status
      );
      return runtime.communityNodeStatuses.find((status) => status.base_url === baseUrl)!;
    },
    // #857: 認証不要の公開 policy カタログ。consent items と同じ slug / 版を返す。
    async fetchCommunityNodePolicies() {
      return {
        policies: mockConsentItems(false).map((item) => ({
          policy_slug: item.policy_slug,
          policy_version: item.policy_version,
          title: item.title,
          body_markdown: item.body ?? '',
          required: item.required,
          is_current: true,
          reference_translation: false,
          fallback: false,
          material_change: false,
          requires_reconsent: false,
        })),
      };
    },
    async acceptCommunityNodeConsents(baseUrl, documents, language) {
      const resolvedUrls = { public_base_url: baseUrl, connectivity_urls: [baseUrl] };
      syncStatus.discovery.connect_mode = 'direct_or_relay';
      const acceptedAt = Math.floor(Date.now() / 1000);
      runtime.communityNodeStatuses = runtime.communityNodeStatuses.map((status) =>
        status.base_url === baseUrl
          ? {
              ...status,
              auth_state: { authenticated: true, expires_at: Date.now() },
              consent_state: { all_required_accepted: true, items: mockConsentItems(true) },
              local_consent: {
                records: documents.map((document) => ({
                  policy_slug: document.policy_slug,
                  policy_version: document.policy_version,
                  policy_snapshot_revision: document.policy_snapshot_revision ?? null,
                  accepted_at: acceptedAt,
                  language,
                  app_version: 'mock',
                })),
                withdrawn_at: null,
              },
              consent_update_pending: false,
              resolved_urls: resolvedUrls,
              session_phase: 'ready',
              retry_after: null,
              restart_required: false,
            }
          : status
      );
      runtime.communityNodeConfig = {
        nodes: runtime.communityNodeConfig.nodes.map((node) =>
          node.base_url === baseUrl ? { ...node, resolved_urls: resolvedUrls } : node
        ),
      };
      return runtime.communityNodeStatuses.find((status) => status.base_url === baseUrl)!;
    },
    // #857: 撤回は記録を履歴として残しつつ接続を停止する。
    async withdrawCommunityNodeConsents(baseUrl) {
      runtime.communityNodeStatuses = runtime.communityNodeStatuses.map((status) =>
        status.base_url === baseUrl
          ? {
              ...status,
              auth_state: { authenticated: false, expires_at: null },
              consent_state: null,
              local_consent: {
                records: status.local_consent?.records ?? [],
                withdrawn_at: Math.floor(Date.now() / 1000),
              },
              consent_update_pending: false,
              session_phase: 'idle',
            }
          : status
      );
      return runtime.communityNodeStatuses.find((status) => status.base_url === baseUrl)!;
    },
    async refreshCommunityNodeMetadata(baseUrl) {
      syncStatus.discovery.connect_mode = 'direct_or_relay';
      const resolvedUrls = { public_base_url: baseUrl, connectivity_urls: [baseUrl] };
      runtime.communityNodeStatuses = runtime.communityNodeStatuses.map((status) =>
        status.base_url === baseUrl
          ? {
              ...status,
              resolved_urls: resolvedUrls,
              session_phase: 'ready',
              retry_after: null,
              restart_required: false,
            }
          : status
      );
      runtime.communityNodeConfig = {
        nodes: runtime.communityNodeConfig.nodes.map((node) =>
          node.base_url === baseUrl ? { ...node, resolved_urls: resolvedUrls } : node
        ),
      };
      return runtime.communityNodeStatuses.find((status) => status.base_url === baseUrl)!;
    },
    async fetchCommunityNodeManifest(baseUrl) {
      return {
        status: 'ok',
        manifest: {
          node_id: '',
          node_name: baseUrl,
          node_role: 'community-node',
          server_name: baseUrl,
          manifest_version: 'v1',
          capability_scope: {
            // この mock は trust / relation 読み取りも提供するため、公開ノード情報でも
            // community_local_trust を提供中として宣言する(#705 の適格判定と一致させる)。
            available_enabled: [
              'auth_consent',
              'bootstrap_assist',
              'iroh_relay',
              'community_index',
              'community_local_trust',
            ],
            planned_enabled: ['moderation'],
          },
          authority_scope: {
            applies_to: [
              'this_node',
              'communities_indexed_by_this_node',
              'trust_signals_issued_by_this_node',
            ],
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
          abuse_contact: `abuse@${baseUrl.replace(/^https?:\/\//, '')}`,
          report_endpoint: `${baseUrl}/v1/report`,
          terms_url: `${baseUrl}/terms`,
          privacy_url: `${baseUrl}/privacy`,
          moderation_policy_url: `${baseUrl}/moderation-policy`,
        },
      };
    },
    // #1056: 既定の mock は advisory を返さない(fixture は runtime.contentAdvisories で差し込む)。
    async lookupCommunityNodeContentAdvisories(request) {
      const requested = new Set([...request.post_ids, ...request.blob_hashes]);
      const nodes = runtime.communityNodeConfig.nodes.filter(
        (node) => node.content_advisory_enabled !== false
      );
      return {
        nodes: nodes.map((node) => ({
          base_url: node.base_url,
          node_id: runtime.contentAdvisoryIssuerNodeId,
          advisories: runtime.contentAdvisories.filter(
            (advisory) =>
              requested.has(advisory.subject_id) &&
              advisory.issuer_node_id === runtime.contentAdvisoryIssuerNodeId
          ),
          error: null,
        })),
      };
    },
    async readCommunityNodeTrustUser(request) {
      return {
        viewer_pubkey: 'mock-viewer',
        target_id: request.target_pubkey,
        absolute: 0,
        relative: 0,
        trust: 0,
        w_abs_applied: 0.5,
        computed_at: new Date(0).toISOString(),
        basis: [],
      };
    },
    async readCommunityNodeRelationUser(request) {
      return {
        viewer_pubkey: 'mock-viewer',
        target_pubkey: request.target_pubkey,
        score: 0.5,
        basis: [
          {
            feature: 'shared_topics',
            value: 1,
            weight: 1,
            contribution: 0.5,
          },
        ],
      };
    },
    async listCommunityNodeRelationNeighbors() {
      const neighbors = Array.from(
        new Set(Object.values(postsByTopic).flatMap((posts) => posts.map((post) => post.author_pubkey)))
      );
      return { viewer_pubkey: 'mock-viewer', neighbors };
    },
    async evaluateAuthorTrustGates(request) {
      return {
        gates: request.author_pubkeys.map((author_pubkey) => ({
          author_pubkey,
          hidden: false,
          node_base_url: null,
          reasons: [],
          expires_at: null,
          always_visible: trustDisplayExceptions.has(author_pubkey),
        })),
      };
    },
    async setAuthorTrustDisplayException(authorPubkey, alwaysVisible) {
      if (alwaysVisible) trustDisplayExceptions.add(authorPubkey);
      else trustDisplayExceptions.delete(authorPubkey);
      return {
        author_pubkey: authorPubkey,
        hidden: false,
        node_base_url: null,
        reasons: [],
        expires_at: null,
        always_visible: alwaysVisible,
      };
    },
    async listAuthorTrustDisplayExceptions() {
      return [...trustDisplayExceptions];
    },
    async getCommunityNodeObservationSharing(baseUrl) {
      return observationSharingStatus(baseUrl, observationSharingNodes.has(baseUrl));
    },
    async enableCommunityNodeObservationSharing(request) {
      observationSharingNodes.add(request.base_url);
      return observationSharingStatus(request.base_url, true);
    },
    async disableCommunityNodeObservationSharing(baseUrl) {
      observationSharingNodes.delete(baseUrl);
      return observationSharingStatus(baseUrl, false);
    },
    async getCommunityNodeRelationOptout(baseUrl) {
      return {
        pubkey: 'mock-viewer',
        opted_out: relationOptoutNodes.has(baseUrl),
        opted_out_at: relationOptoutNodes.has(baseUrl) ? new Date(0).toISOString() : null,
        min_proximity: 0.25,
      };
    },
    async setCommunityNodeRelationOptout(baseUrl) {
      relationOptoutNodes.add(baseUrl);
      return {
        pubkey: 'mock-viewer',
        opted_out: true,
        opted_out_at: new Date(0).toISOString(),
        min_proximity: 0.25,
      };
    },
    async clearCommunityNodeRelationOptout(baseUrl) {
      relationOptoutNodes.delete(baseUrl);
      return {
        pubkey: 'mock-viewer',
        opted_out: false,
        opted_out_at: null,
        min_proximity: 0.25,
      };
    },
    async searchCommunityNodeIndex(request) {
      return queryIndex(request);
    },
    async discoverCommunityNodeIndex(request) {
      return queryIndex(request);
    },
    async recommendCommunityNodeIndex(request) {
      return queryIndex(request);
    },
    async submitCommunityNodeIndexingRequest(request) {
      const targetId = request.channel_id ?? request.topic_id;
      const requests = indexingRequestsByNode.get(request.base_url) ?? [];
      const existing = requests.find(
        (entry) => entry.scope_kind === request.scope_kind && entry.target_id === targetId
      );
      if (existing) {
        return { request_id: existing.request_id, status: existing.status };
      }
      const created: IndexingRequestView = {
        request_id: `mock-indexing-${request.scope_kind}-${targetId}`,
        scope_kind: request.scope_kind,
        target_id: targetId,
        status: 'pending',
        created_at: Date.now(),
        decided_at: null,
      };
      indexingRequestsByNode.set(request.base_url, [created, ...requests]);
      return {
        request_id: created.request_id,
        status: created.status,
      } satisfies SubmitIndexingRequestResponse;
    },
    async revokeCommunityNodeIndexingRequest(request) {
      const targetId = request.channel_id ?? request.topic_id;
      const requests = indexingRequestsByNode.get(request.base_url) ?? [];
      indexingRequestsByNode.set(request.base_url, requests.filter(
        (entry) => entry.scope_kind !== request.scope_kind || entry.target_id !== targetId
      ));
    },
    async readCommunityNodeIndexingStatus(request) {
      const requests = indexingRequestsByNode.get(request.base_url) ?? [];
      if (!request.scope_kind) {
        return { requests, target: null } satisfies IndexingStatusResponse;
      }
      const scopeId =
        request.scope_kind === 'private_channel' ? (request.channel_id ?? '') : (request.topic_id ?? '');
      const supported =
        request.scope_kind === 'public_topic'
          ? supportedPublicTopics.has(scopeId) ||
            requests.some(
              (entry) =>
                entry.scope_kind === 'public_topic' &&
                entry.target_id === scopeId &&
                entry.status === 'approved'
            )
          : false;
      return {
        requests,
        target: { scope_kind: request.scope_kind, scope_id: scopeId, supported },
      } satisfies IndexingStatusResponse;
    },
    async submitCommunityNodeReport(request) {
      return {
        status: 'submitted',
        reference_id: `mock-${request.subject_kind}-${request.subject_id}`,
        disputed_risk_signal_id: request.appeal?.risk_signal_id ?? null,
      };
    },
    async submitCommunityNodeTesterFeedback() {
      return {
        reference_id: `mock-tester-feedback-${Date.now()}`,
      };
    },
    async importPeerTicket() {},
    async setDiscoverySeeds(seedEntries) {
      runtime.discoveryConfig = {
        ...runtime.discoveryConfig,
        seed_peers: seedEntries.map((entry) => {
          const [endpointId, addrHint] = entry.split('@', 2);
          return {
            endpoint_id: endpointId,
            addr_hint: addrHint ?? null,
          };
        }),
      };
      syncStatus.discovery.configured_seed_peer_ids = runtime.discoveryConfig.seed_peers.map(
        (peer) => peer.endpoint_id
      );
      return runtime.discoveryConfig;
    },
    async unsubscribeTopic(topic) {
      delete postsByTopic[topic];
      delete liveSessionsByTopic[topic];
      delete gameRoomsByTopic[topic];
      delete joinedChannelsByTopic[topic];
      syncStatus.subscribed_topics = syncStatus.subscribed_topics.filter((value) => value !== topic);
      syncStatus.topic_diagnostics = syncStatus.topic_diagnostics.filter(
        (value) => value.topic !== topic
      );
    },
    async setTopicGossipEnabled(topic, enabled) {
      syncStatus.gossip_disabled_topics = syncStatus.gossip_disabled_topics.filter(
        (value) => value !== topic
      );
      if (!enabled) {
        syncStatus.gossip_disabled_topics.push(topic);
      }
    },
    async setChannelGossipEnabled(topic, channelId, enabled) {
      const key = `${topic}::${channelId}`;
      syncStatus.gossip_disabled_channels = syncStatus.gossip_disabled_channels.filter(
        (value) => value !== key
      );
      if (!enabled) {
        syncStatus.gossip_disabled_channels.push(key);
      }
    },
    async getLocalPeerTicket() {
      return 'peer1@127.0.0.1:7777';
    },
    async getBlobMediaPayload(hash, mime): Promise<BlobMediaPayload | null> {
      if (metaverseAssetPayloads[hash]) {
        return metaverseAssetPayloads[hash];
      }
      return {
        bytes_base64: mime.startsWith('video/') ? 'ZmFrZS12aWRlbw==' : 'ZmFrZS1pbWFnZQ==',
        mime,
      };
    },
    async getBlobPreviewUrl() {
      return null;
    },
    // #858: mock は成人向け表示設定を in-memory で保持する(既定 OFF)。
    async getContentDisplaySettings() {
      return { adult_content_enabled: adultContentDisplayEnabled };
    },
    async setAdultContentDisplayEnabled(enabled) {
      adultContentDisplayEnabled = enabled;
      return { adult_content_enabled: adultContentDisplayEnabled };
    },
  };
}

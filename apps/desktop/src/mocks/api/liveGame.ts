import {
  type DesktopApi,
  type DomeConnectionProposalView,
  type DomeConnectionTopologyView,
  type DomeConnectionView,
  type DomeHostingStateV1,
  type DomeHostingView,
  type DomeTransitionAdmissionTicketV1,
  type GameScoreView,
  type MetaverseAssetRef,
  type MetaverseRoomEventV1,
  type MetaverseRoomEventView,
  type TimelineScope,
} from '@/lib/api';
import { createDefaultMetaverseRoomState } from '@/components/extended/metaverse/DomeSceneModel';

import {
  filterChannelScopedItems,
  withGameRoomDefaults,
  withLiveSessionDefaults,
} from '../desktopMockModel';
import { type MockRuntime } from '../mockRuntime';

type LiveGameMock = Pick<
  DesktopApi,
  | 'listSessionCandidates'
  | 'setSessionDisplay'
  | 'setScopeDisplay'
  | 'listLiveSessions'
  | 'createLiveSession'
  | 'endLiveSession'
  | 'joinLiveSession'
  | 'leaveLiveSession'
  | 'listGameRooms'
  | 'createGameRoom'
  | 'createMetaverseRoom'
  | 'updateGameRoom'
  | 'listPendingDomeDeletions'
  | 'deleteDome'
  | 'updateMetaverseRoom'
  | 'getDomeHosting'
  | 'startOwnerDomeHosting'
  | 'delegateDomeHosting'
  | 'closeDomeHosting'
  | 'submitDomeSessionInput'
  | 'prepareDomeTransition'
  | 'previewDomeTransitionAccess'
  | 'commitDomeTransition'
  | 'abortDomeTransition'
  | 'commitDomeLayout'
  | 'resyncDomeSnapshots'
  | 'moveDome'
  | 'listDomeConnectionTopology'
  | 'createDomeConnectionProposal'
  | 'acceptDomeConnectionProposal'
  | 'withdrawDomeConnectionProposal'
  | 'revokeDomeConnection'
  | 'publishMetaverseRoomEvent'
  | 'listMetaverseRoomEvents'
  | 'importMetaverseRoomAsset'
>;

export function createLiveGameMock(runtime: MockRuntime): LiveGameMock {
  const {
    liveSessionsByTopic,
    gameRoomsByTopic,
    syncStatus,
    metaverseRoomEventsByRoom,
    metaverseAssetPayloads,
    mutedAuthorPubkeys,
  } = runtime;
  const connectionProposals = new Map<string, DomeConnectionProposalView>();
  const connectionViews = new Map<string, DomeConnectionView>();
  const hostingViews = new Map<string, DomeHostingView>();
  const transitionTickets = new Map<string, DomeTransitionAdmissionTicketV1>();

  const updateRoomHostingState = (instanceId: string, state: DomeHostingStateV1) => {
    for (const topic of Object.keys(gameRoomsByTopic)) {
      gameRoomsByTopic[topic] = (gameRoomsByTopic[topic] ?? []).map((room) =>
        room.metaverse?.instance_id === instanceId
          ? { ...room, dome_hosting: state, updated_at: Date.now() }
          : room
      );
    }
  };

  const contextKey = (context: Parameters<DesktopApi['listDomeConnectionTopology']>[0]) =>
    context.kind === 'topic'
      ? `topic:${context.topic_id}`
      : `channel:${context.topic_id}:${context.channel_id}`;

  const topologyView = (
    context: Parameters<DesktopApi['listDomeConnectionTopology']>[0]
  ): DomeConnectionTopologyView => {
    const rooms = (gameRoomsByTopic[context.topic_id] ?? []).filter(
      (room) =>
        room.metaverse?.spatial_context.kind === context.kind &&
        (context.kind === 'topic' ||
          (room.metaverse.spatial_context.kind === 'channel' &&
            room.metaverse.spatial_context.channel_id === context.channel_id))
    );
    const connections = [...connectionViews.values()].filter(
      (connection) => contextKey(connection.record.agreement.spatial_context) === contextKey(context)
    );
    const active = connections.filter((connection) => connection.record.status === 'active');
    const connectedIds = new Set(
      active.flatMap((connection) => [
        connection.record.agreement.proposer.instance_id,
        connection.record.agreement.receiver.instance_id,
      ])
    );
    const components = rooms.map((room) => ({
      root_instance_id: room.room_id,
      instance_ids: [room.room_id],
      connection_ids: [] as string[],
      coordinates_cm: { [room.room_id]: [0, 0, 0] as [number, number, number] },
    }));
    if (active.length === 1) {
      const agreement = active[0].record.agreement;
      const proposerRoom = rooms.find((room) => room.room_id === agreement.proposer.instance_id);
      const receiverRoom = rooms.find((room) => room.room_id === agreement.receiver.instance_id);
      if (proposerRoom && receiverRoom) {
        const offset: Record<string, [number, number, number]> = {
          north: [0, 0, -5700],
          east: [5700, 0, 0],
          south: [0, 0, 5700],
          west: [-5700, 0, 0],
        };
        const root = [proposerRoom.room_id, receiverRoom.room_id].sort()[0];
        const proposerAtRoot = root === proposerRoom.room_id;
        const direction = proposerAtRoot
          ? agreement.proposer.direction
          : agreement.receiver.direction;
        const neighbor = proposerAtRoot ? receiverRoom.room_id : proposerRoom.room_id;
        components.splice(
          0,
          components.length,
          {
            root_instance_id: root,
            instance_ids: [root, neighbor].sort(),
            connection_ids: [agreement.connection_id],
            coordinates_cm: {
              [root]: [0, 0, 0],
              [neighbor]: offset[direction],
            },
          },
          ...rooms
            .filter((room) => !connectedIds.has(room.room_id))
            .map((room) => ({
              root_instance_id: room.room_id,
              instance_ids: [room.room_id],
              connection_ids: [] as string[],
              coordinates_cm: { [room.room_id]: [0, 0, 0] as [number, number, number] },
            }))
        );
      }
    }
    return {
      proposals: [...connectionProposals.values()].filter(
        (proposal) => contextKey(proposal.proposal.spatial_context) === contextKey(context)
      ),
      connections,
      resolution: {
        topology: {
          spatial_context: context,
          components: components.sort((a, b) => a.root_instance_id.localeCompare(b.root_instance_id)),
          active_connection_ids: active.map((connection) => connection.record.agreement.connection_id),
          topology_digest: `mock-${contextKey(context)}-${active.length}`,
        },
        rejected_connections: [],
      },
    };
  };

  return {
    async listSessionCandidates() { return []; },
    async setSessionDisplay() {},
    async setScopeDisplay() {},
    async listLiveSessions(topic, scope: TimelineScope = { kind: 'public' }) {
      const muted = mutedAuthorPubkeys();
      return filterChannelScopedItems(
        liveSessionsByTopic[topic] ?? [],
        scope
      ).filter((session) => !muted.has(session.host_pubkey));
    },
    async createLiveSession(topic, title, description, channelRef = { kind: 'public' }) {
      runtime.sequence += 1;
      const sessionId = `live-${runtime.sequence}`;
      const channelId = channelRef.kind === 'private_channel' ? channelRef.channel_id : null;
      liveSessionsByTopic[topic] = [
        withLiveSessionDefaults({
          session_id: sessionId,
          host_pubkey: syncStatus.local_author_pubkey,
          title,
          description,
          status: 'Live',
          started_at: Date.now(),
          ended_at: null,
          viewer_count: 0,
          joined_by_me: false,
          channel_id: channelId,
          audience_label: channelId ? 'Private channel' : 'Public',
        }),
        ...(liveSessionsByTopic[topic] ?? []),
      ];
      return sessionId;
    },
    async endLiveSession(topic, sessionId) {
      liveSessionsByTopic[topic] = (liveSessionsByTopic[topic] ?? []).map((session) =>
        session.session_id === sessionId
          ? { ...session, status: 'Ended', ended_at: Date.now(), joined_by_me: false }
          : session
      );
    },
    async joinLiveSession(topic, sessionId) {
      liveSessionsByTopic[topic] = (liveSessionsByTopic[topic] ?? []).map((session) =>
        session.session_id === sessionId
          ? { ...session, joined_by_me: true, viewer_count: session.viewer_count + 1 }
          : session
      );
    },
    async leaveLiveSession(topic, sessionId) {
      liveSessionsByTopic[topic] = (liveSessionsByTopic[topic] ?? []).map((session) =>
        session.session_id === sessionId
          ? {
              ...session,
              joined_by_me: false,
              viewer_count: Math.max(0, session.viewer_count - 1),
            }
          : session
      );
    },
    async listGameRooms(topic, scope: TimelineScope = { kind: 'public' }) {
      const muted = mutedAuthorPubkeys();
      return filterChannelScopedItems(
        gameRoomsByTopic[topic] ?? [],
        scope
      ).filter((room) => !muted.has(room.host_pubkey));
    },
    async createGameRoom(topic, title, description, participants, channelRef = { kind: 'public' }) {
      runtime.sequence += 1;
      const roomId = `game-${runtime.sequence}`;
      const channelId = channelRef.kind === 'private_channel' ? channelRef.channel_id : null;
      const scores: GameScoreView[] = participants.map((label, index) => ({
        participant_id: `participant-${index + 1}`,
        label,
        score: 0,
      }));
      gameRoomsByTopic[topic] = [
        withGameRoomDefaults({
          room_id: roomId,
          host_pubkey: syncStatus.local_author_pubkey,
          title,
          description,
          status: 'Waiting',
          phase_label: null,
          scores,
          updated_at: Date.now(),
          channel_id: channelId,
          audience_label: channelId ? 'Private channel' : 'Public',
        }),
        ...(gameRoomsByTopic[topic] ?? []),
      ];
      return roomId;
    },
    async createMetaverseRoom(
      topic,
      title,
      description,
      maxPeers = null,
      channelRef = { kind: 'public' }
    ) {
      runtime.sequence += 1;
      const roomId = `meta-${runtime.sequence}`;
      const channelId = channelRef.kind === 'private_channel' ? channelRef.channel_id : null;
      const now = Date.now();
      gameRoomsByTopic[topic] = [
        withGameRoomDefaults({
          room_id: roomId,
          host_pubkey: syncStatus.local_author_pubkey,
          title,
          description,
          status: 'Waiting',
          phase_label: 'fixed-dome-v1',
          scores: [],
          room_kind: 'metaverse_room',
          metaverse: createDefaultMetaverseRoomState(maxPeers, {
            roomId,
            topicId: topic,
            channelId,
            ownerPubkey: syncStatus.local_author_pubkey,
          }),
          manifest_blob_hash: `mock-${roomId}`,
          updated_at: now,
          channel_id: channelId,
          audience_label: channelId ? 'Private channel' : 'Public',
        }),
        ...(gameRoomsByTopic[topic] ?? []),
      ];
      return roomId;
    },
    async updateGameRoom(topic, roomId, status, phaseLabel, scores) {
      gameRoomsByTopic[topic] = (gameRoomsByTopic[topic] ?? []).map((room) =>
        room.room_id === roomId
          ? {
              ...room,
              status,
              phase_label: phaseLabel,
              scores: scores.map((score) => ({ ...score })),
              updated_at: Date.now(),
            }
          : room
      );
    },
    async listPendingDomeDeletions() { return []; },
    async deleteDome(context, instanceId, generation) {
      const rooms = gameRoomsByTopic[context.topic_id] ?? [];
      const room = rooms.find((r) => r.room_id === instanceId);
      if (!room?.metaverse || room.host_pubkey !== syncStatus.local_author_pubkey || room.metaverse.instance_generation !== generation
        || JSON.stringify(room.metaverse.spatial_context) !== JSON.stringify(context)) throw new Error('DOME_DELETE_STALE_INSTANCE');
      gameRoomsByTopic[context.topic_id] = rooms.filter((r) => r !== room);
      hostingViews.delete(instanceId);
      return { deleted: true, cleanup_pending: false };
    },
    async updateMetaverseRoom(
      topic,
      roomId,
      status,
      customization
    ) {
      const now = Date.now();
      gameRoomsByTopic[topic] = (gameRoomsByTopic[topic] ?? []).map((room) =>
        room.room_id === roomId && room.metaverse
          ? withGameRoomDefaults({
              ...room,
              status,
              metaverse: {
                ...room.metaverse,
                dome: { ...room.metaverse.dome, customization },
              },
              updated_at: now,
              manifest_blob_hash: `mock-${roomId}-${now}`,
            })
          : room
      );
    },
    async getDomeHosting(spatialContext, instanceId) {
      const existing = hostingViews.get(instanceId);
      if (existing) return existing;
      return {
        instance_id: instanceId,
        state: {
          kind: 'closed',
          host: null,
          lease_id: null,
          lease_epoch: null,
          lease_expires_at: null,
          session_id: null,
          reason: 'not_hosted',
          last_heartbeat_at: null,
        },
        lease: null,
        signed_lease_json: null,
        signed_activation_json: null,
        signed_close_json: null,
        instance_manifest_json: JSON.stringify({ instance_id: instanceId, spatial_context: spatialContext }),
        preset_manifest_json: '{}',
        participants: 0,
        sleeping: true,
        resource_budget: {
          dome: {
            max_persistent_props: 64,
            max_texture_bytes: 268435456,
            max_texture_dimension: 8192,
            max_model_bytes: 268435456,
            max_model_triangles: 2000000,
            max_colliders: 128,
            max_rigid_bodies: 256,
            max_snapshot_hz: 10,
          },
          player: {
            max_guest_props: 16,
            max_guest_prop_bytes: 67108864,
            max_avatar_asset_bytes: 33554432,
            max_prop_spawns_per_minute: 12,
            max_interactions_per_second: 30,
            max_input_bytes_per_second: 262144,
            max_proposals_per_ten_minutes_per_slot: 8,
            max_impulse_centimeters: 5000,
            max_audio_frames_per_second: 50,
            max_audio_bytes_per_second: 32768,
          },
          host: {
            max_participants: 64,
            max_simulated_rigid_bodies: 512,
            max_snapshot_bytes_per_second: 4194304,
            max_session_asset_bytes: 536870912,
          },
          client: {
            max_rendered_avatars: 32,
            max_texture_memory_bytes: 536870912,
            max_rendered_triangles: 3000000,
            max_interpolated_bodies: 256,
            max_neighbor_domes: 4,
            cache_capacity_bytes: 1073741824,
            max_concurrent_audio_streams: 16,
            max_audio_jitter_frames: 8,
          },
        },
        resource_metrics: {
          rejected_total: 0,
          rejection_counts: [],
          participant_high_water: 0,
          rigid_body_high_water: 0,
          snapshot_bytes: 0,
          snapshot_throttled: 0,
        },
      };
    },
    async startOwnerDomeHosting(spatialContext, instanceId, endpointId, leaseDurationMillis) {
      const now = Date.now();
      const view: DomeHostingView = {
        ...(await this.getDomeHosting(spatialContext, instanceId)),
        state: {
          kind: 'owner_hosted',
          host: { kind: 'owner_device', endpoint_id: endpointId, host_pubkey: syncStatus.local_author_pubkey },
          lease_id: `mock-lease-${instanceId}`,
          lease_epoch: 1,
          lease_expires_at: now + leaseDurationMillis,
          session_id: `mock-session-${instanceId}`,
          reason: null,
          last_heartbeat_at: now,
        },
        participants: 0,
        sleeping: true,
      };
      hostingViews.set(instanceId, view);
      updateRoomHostingState(instanceId, view.state);
      return view;
    },
    async delegateDomeHosting(spatialContext, instanceId, nodeId, baseUrl, leaseDurationMillis) {
      const now = Date.now();
      const view: DomeHostingView = {
        ...(await this.getDomeHosting(spatialContext, instanceId)),
        state: {
          kind: 'community_node_hosted',
          host: { kind: 'community_node', node_id: nodeId, api_base_url: baseUrl },
          lease_id: `mock-lease-${instanceId}`,
          lease_epoch: 2,
          lease_expires_at: now + leaseDurationMillis,
          session_id: `mock-cn-session-${instanceId}`,
          reason: null,
          last_heartbeat_at: now,
        },
        participants: 0,
        sleeping: true,
      };
      hostingViews.set(instanceId, view);
      updateRoomHostingState(instanceId, view.state);
      return view;
    },
    async closeDomeHosting(spatialContext, instanceId) {
      const view = await this.getDomeHosting(spatialContext, instanceId);
      const closed: DomeHostingView = {
        ...view,
        state: { ...view.state, kind: 'closed', session_id: null, reason: 'owner_closed' },
      };
      hostingViews.set(instanceId, closed);
      updateRoomHostingState(instanceId, closed.state);
      return closed;
    },
    async submitDomeSessionInput(_spatialContext, instanceId, sequence, input) {
      const position = input.type === 'move' ? input.position : [0, 0, 0] as [number, number, number];
      const rotation = input.type === 'move' ? input.rotation : [0, 0, 0] as [number, number, number];
      return {
        instance_id: instanceId,
        instance_generation: 1,
        lease_epoch: 1,
        session_id: `mock-session-${instanceId}`,
        host_pubkey: syncStatus.local_author_pubkey,
        sequence,
        simulated_at: Date.now(),
        sleeping: false,
        bodies: [{
          entity_id: `avatar:${syncStatus.local_author_pubkey}`,
          kind: 'avatar',
          position,
          rotation,
          linear_velocity: [0, 0, 0],
          animation: input.type === 'move' ? input.animation : 'idle',
          grabbed_by: null,
          expires_at: null,
        }],
      };
    },
    async previewDomeTransitionAccess() {
      return { status: 'allowed' };
    },
    async prepareDomeTransition(request) {
      const hosting = await this.getDomeHosting(request.spatial_context, request.target_instance_id);
      if (!hosting.state.session_id || hosting.state.lease_epoch == null) {
        throw new Error('target Dome is not hosted');
      }
      const ticket: DomeTransitionAdmissionTicketV1 = {
        request,
        target_lease_epoch: hosting.state.lease_epoch,
        target_session_id: hosting.state.session_id,
        expires_at: Date.now() + 15_000,
      };
      transitionTickets.set(request.transition_id, ticket);
      return ticket;
    },
    async commitDomeTransition(ticket) {
      if (!transitionTickets.has(ticket.request.transition_id)) {
        throw new Error('transition ticket is not reserved');
      }
      transitionTickets.delete(ticket.request.transition_id);
    },
    async abortDomeTransition(ticket) {
      transitionTickets.delete(ticket.request.transition_id);
    },
    async commitDomeLayout(spatialContext, instanceId, operationId) {
      const rooms = gameRoomsByTopic[spatialContext.topic_id] ?? [];
      const room = rooms.find((candidate) => candidate.room_id === instanceId);
      if (!room?.metaverse) throw new Error('Dome instance not found');
      room.metaverse.preset_ref = {
        ...room.metaverse.preset_ref,
        revision: room.metaverse.preset_ref.revision + 1,
        manifest_blob_hash: `mock-layout-${operationId}`,
      };
      const hosting = await this.getDomeHosting(spatialContext, instanceId);
      return {
        outcome: 'committed',
        operation_id: operationId,
        revision: room.metaverse.preset_ref.revision,
        manifest_blob_hash: room.metaverse.preset_ref.manifest_blob_hash,
        signed_commit_json: '{}',
        hosting,
      };
    },
    async resyncDomeSnapshots() {
      return [];
    },
    async moveDome(sourceTopic, moveId, sourceInstanceId, targetContext) {
      const source = (gameRoomsByTopic[sourceTopic] ?? []).find(
        (room) => room.room_id === sourceInstanceId && room.metaverse
      );
      if (!source?.metaverse) throw new Error('source Dome instance not found');
      const targetTopic = targetContext.topic_id;
      const targetRoomId = `moved-${sourceInstanceId}`;
      gameRoomsByTopic[sourceTopic] = (gameRoomsByTopic[sourceTopic] ?? []).filter(
        (room) => room.room_id !== sourceInstanceId
      );
      gameRoomsByTopic[targetTopic] = [
        withGameRoomDefaults({
          ...source,
          room_id: targetRoomId,
          channel_id: targetContext.kind === 'channel' ? targetContext.channel_id : null,
          metaverse: {
            ...source.metaverse,
            instance_id: targetRoomId,
            spatial_context: targetContext,
            session_id: targetRoomId,
            relationship_detach: null,
            replacement_instance_id: null,
          },
        }),
        ...(gameRoomsByTopic[targetTopic] ?? []),
      ];
      return {
        move_id: moveId,
        owner_pubkey: syncStatus.local_author_pubkey,
        source_instance_id: sourceInstanceId,
        source_context: source.metaverse.spatial_context,
        source_generation: source.metaverse.instance_generation,
        target_instance_id: targetRoomId,
        target_context: targetContext,
        target_generation: 1,
        preset_ref: source.metaverse.preset_ref,
        phase: 'completed',
        failure_reason: null,
        updated_at: Date.now(),
      };
    },
    async listDomeConnectionTopology(spatialContext) {
      return topologyView(spatialContext);
    },
    async createDomeConnectionProposal(
      proposalId,
      spatialContext,
      proposerInstanceId,
      receiverInstanceId,
      proposerDirection
    ) {
      const rooms = gameRoomsByTopic[spatialContext.topic_id] ?? [];
      const proposerRoom = rooms.find((room) => room.room_id === proposerInstanceId);
      const receiverRoom = rooms.find((room) => room.room_id === receiverInstanceId);
      if (!proposerRoom?.metaverse || !receiverRoom?.metaverse) {
        throw new Error('Dome Connection endpoint not found');
      }
      const opposite = { north: 'south', east: 'west', south: 'north', west: 'east' } as const;
      const proposal: DomeConnectionProposalView = {
        proposal: {
          proposal_id: proposalId,
          spatial_context: spatialContext,
          proposer: {
            instance_id: proposerInstanceId,
            instance_generation: proposerRoom.metaverse.instance_generation,
            owner_pubkey: proposerRoom.host_pubkey,
            direction: proposerDirection,
          },
          receiver: {
            instance_id: receiverInstanceId,
            instance_generation: receiverRoom.metaverse.instance_generation,
            owner_pubkey: receiverRoom.host_pubkey,
            direction: opposite[proposerDirection],
          },
          sequence: connectionProposals.size + 1,
          created_at: Date.now(),
        },
        selection: null,
        status: 'proposed',
        terminal_reason: null,
        connection_id: `connection-${proposalId}`,
      };
      connectionProposals.set(proposalId, proposal);
      return proposal;
    },
    async acceptDomeConnectionProposal(spatialContext, proposalId) {
      const proposal = connectionProposals.get(proposalId);
      if (!proposal || contextKey(proposal.proposal.spatial_context) !== contextKey(spatialContext)) {
        throw new Error('Dome Connection proposal not found');
      }
      const connection: DomeConnectionView = {
        record: {
          agreement: {
            connection_id: proposal.connection_id,
            proposal_id: proposalId,
            spatial_context: spatialContext,
            proposer: proposal.proposal.proposer,
            receiver: proposal.proposal.receiver,
            activation_generation: proposal.proposal.sequence,
          },
          receiver_slot_generation: 1,
          observed_active_connection_ids: [],
          status: 'active',
          lifecycle_generation: 1,
          lifecycle_actor: null,
          lifecycle_reason: null,
          lifecycle_deadline_at: null,
        },
      };
      connectionViews.set(proposal.connection_id, connection);
      connectionProposals.set(proposalId, {
        ...proposal,
        selection: {
          selection_id: `selection-${proposalId}-1`,
          proposal_id: proposalId,
          spatial_context: spatialContext,
          receiver: proposal.proposal.receiver,
          slot_generation: 1,
          observed_active_connection_ids: [],
          selected_at: Date.now(),
        },
        status: 'accepted',
      });
      return connection;
    },
    async withdrawDomeConnectionProposal(spatialContext, proposalId) {
      const proposal = connectionProposals.get(proposalId);
      if (!proposal || contextKey(proposal.proposal.spatial_context) !== contextKey(spatialContext)) {
        throw new Error('Dome Connection proposal not found');
      }
      const withdrawn: DomeConnectionProposalView = {
        ...proposal,
        status: 'discarded',
        terminal_reason: 'proposer_withdrew',
      };
      connectionProposals.set(proposalId, withdrawn);
      return withdrawn;
    },
    async revokeDomeConnection(spatialContext, connectionId) {
      const connection = connectionViews.get(connectionId);
      if (!connection || contextKey(connection.record.agreement.spatial_context) !== contextKey(spatialContext)) {
        throw new Error('Dome Connection not found');
      }
      const revoked: DomeConnectionView = {
        record: {
          ...connection.record,
          status: 'revoked',
          lifecycle_generation: connection.record.lifecycle_generation + 2,
          lifecycle_actor: syncStatus.local_author_pubkey,
          lifecycle_reason: 'owner_revoked',
          lifecycle_deadline_at: null,
        },
      };
      connectionViews.set(connectionId, revoked);
      return revoked;
    },
    async publishMetaverseRoomEvent(topic, roomId, peerId, seq, event) {
      const now = Date.now();
      const envelopeId = `mock-metaverse-event-${now}-${seq}`;
      const view: MetaverseRoomEventView = {
        envelope_id: envelopeId,
        content: {
          event_id: envelopeId,
          topic_id: topic,
          channel_id: null,
          room_id: roomId,
          spatial_context: (gameRoomsByTopic[topic] ?? []).find((room) => room.room_id === roomId)?.metaverse?.spatial_context ?? { kind: 'topic', topic_id: topic },
          instance_generation: (gameRoomsByTopic[topic] ?? []).find((room) => room.room_id === roomId)?.metaverse?.instance_generation ?? 1,
          session_id: roomId,
          peer_id: peerId,
          seq,
          sent_at: now,
          event: event as MetaverseRoomEventV1,
        },
        envelope: {
          id: envelopeId,
          kind: 'metaverse-room-event',
          pubkey: syncStatus.local_author_pubkey,
        },
        received_at: now,
        source_peer: 'mock-local',
      };
      const key = `${topic}::${roomId}`;
      metaverseRoomEventsByRoom[key] = [...(metaverseRoomEventsByRoom[key] ?? []), view].slice(-512);
      if (event.type === 'chat_message') {
        gameRoomsByTopic[topic] = (gameRoomsByTopic[topic] ?? []).map((room) => {
          if (room.room_id !== roomId || !room.metaverse) {
            return room;
          }
          const chatHistory = [
            ...(room.metaverse.chat_history ?? []).filter(
              (message) => message.message_id !== event.message.message_id
            ),
            event.message,
          ].slice(-100);
          return withGameRoomDefaults({
            ...room,
            metaverse: {
              ...room.metaverse,
              chat_history: chatHistory,
            },
            updated_at: now,
            manifest_blob_hash: `mock-${roomId}-${now}`,
          });
        });
      }
      return view;
    },
    async listMetaverseRoomEvents(topic, roomId, afterEnvelopeId = null, limit = null) {
      const key = `${topic}::${roomId}`;
      const events = metaverseRoomEventsByRoom[key] ?? [];
      const start = afterEnvelopeId
        ? events.findIndex((event) => event.envelope_id === afterEnvelopeId) + 1
        : 0;
      const page = events.slice(Math.max(0, start));
      return typeof limit === 'number' && page.length > limit ? page.slice(page.length - limit) : page;
    },
    async importMetaverseRoomAsset(_topic, roomId, kind, mimeType, name, dataBase64) {
      const hash = `mock-metaverse-asset-${roomId}-${Object.keys(metaverseAssetPayloads).length + 1}`;
      metaverseAssetPayloads[hash] = {
        bytes_base64: dataBase64,
        mime: mimeType,
      };
      return {
        kind,
        blob_hash: hash,
        mime_type: mimeType,
        size_bytes: Math.ceil((dataBase64.length * 3) / 4),
        name,
      } satisfies MetaverseAssetRef;
    },
  };
}

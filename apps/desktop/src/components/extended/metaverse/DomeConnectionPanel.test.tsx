import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, test, vi } from 'vitest';

import i18n from '@/i18n';
import type { DomeConnectionTopologyView, GameRoomView } from '@/lib/api';
import { createDefaultMetaverseRoomState } from './DomeSceneModel';
import { DomeConnectionPanel } from './DomeConnectionPanel';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import { connectionFixtureRooms, connectionFixtureTopology } from './DomeConnectionFixtures';

afterEach(async () => {
  await i18n.changeLanguage('en');
});

const context = { kind: 'topic' as const, topic_id: 'kukuri:topic:connections-ui' };

function room(id: string, owner: string, title: string): GameRoomView {
  return {
    room_id: id,
    host_pubkey: owner,
    title,
    description: '',
    status: 'Waiting',
    phase_label: 'fixed-dome-v1',
    scores: [],
    room_kind: 'metaverse_room',
    metaverse: createDefaultMetaverseRoomState(8, {
      roomId: id,
      topicId: context.topic_id,
      ownerPubkey: owner,
    }),
    manifest_blob_hash: `manifest-${id}`,
    updated_at: 1,
    channel_id: null,
    audience_label: 'Public',
  };
}

const owner = 'a'.repeat(64);
const receiverOwner = 'b'.repeat(64);
const localRoom = room('dome-a', owner, 'Home Dome');
const receiverRoom = room('dome-b', receiverOwner, 'Neighbor Dome');

function emptyTopology(): DomeConnectionTopologyView {
  return {
    proposals: [],
    connections: [],
    resolution: {
      topology: {
        spatial_context: context,
        components: [localRoom, receiverRoom].map((item) => ({
          root_instance_id: item.room_id,
          instance_ids: [item.room_id],
          connection_ids: [],
          coordinates_cm: { [item.room_id]: [0, 0, 0] },
        })),
        active_connection_ids: [],
        topology_digest: 'empty-topology',
      },
      rejected_connections: [],
    },
  };
}

function actions(topology = emptyTopology()) {
  return {
    listPendingDeletions: vi.fn().mockResolvedValue([]),
    deleteRoom: vi.fn().mockResolvedValue({ deleted: true, cleanup_pending: false }),
    createRoom: vi.fn(),
    publishRoomEvent: vi.fn(),
    listRoomEvents: vi.fn(),
    getAuthorSocialView: vi.fn(),
    importRoomAsset: vi.fn(),
    getBlobPreviewUrl: vi.fn(),
    updateRoom: vi.fn(),
    getHosting: vi.fn(),
    startOwnerHosting: vi.fn(),
    delegateHosting: vi.fn(),
    closeHosting: vi.fn(),
    submitSessionInput: vi.fn(),
    prepareTransition: vi.fn(),
    previewTransitionAccess: vi.fn().mockResolvedValue({ status: 'allowed' }),
    commitTransition: vi.fn(),
    abortTransition: vi.fn(),
    commitLayout: vi.fn(),
    resyncSnapshots: vi.fn(),
    moveRoom: vi.fn(),
    listConnections: vi.fn().mockResolvedValue(topology),
    createConnectionProposal: vi.fn().mockResolvedValue(topology.proposals[0]),
    acceptConnectionProposal: vi.fn(),
    withdrawConnectionProposal: vi.fn(),
    revokeConnection: vi.fn(),
    refresh: vi.fn(),
  } satisfies MetaverseRoomActions;
}

describe('DomeConnectionPanel', () => {
  test('a recreated Dome can propose into a slot with an old accepted record', async () => {
    const topology = connectionFixtureTopology(); topology.connections[0].record.status = 'accepted';
    const room = structuredClone(connectionFixtureRooms[0]); room.metaverse!.instance_generation = 2;
    const roomActions = actions(topology); const user = userEvent.setup();
    render(<DomeConnectionPanel room={room} rooms={[room, ...connectionFixtureRooms.slice(1)]} actions={roomActions}
      localAuthorPubkey={room.host_pubkey} locale='en' />);
    await user.click(screen.getByRole('button', { name: /^East/ }));
    await user.selectOptions(await screen.findByRole('combobox'), 'd');
    await user.click(screen.getByRole('button', { name: 'Propose Connection' }));
    expect(roomActions.createConnectionProposal).toHaveBeenCalledWith(expect.any(String), room.metaverse!.spatial_context, 'a', 'd', 'east');
  });
  test('a draining record overrides an older ready passage result', async () => {
    const topology = connectionFixtureTopology(); topology.connections[0].record.status = 'draining';
    const room = connectionFixtureRooms[0]; const user = userEvent.setup();
    render(<DomeConnectionPanel room={room} rooms={connectionFixtureRooms} actions={actions(topology)}
      localAuthorPubkey={room.host_pubkey} locale='en' boundaries={{ east: 'ready' }} />);
    await user.click(screen.getByRole('button', { name: /^East/ }));
    expect(await screen.findByText('Passage: Disconnecting; passage is disabled')).toBeInTheDocument();
    expect(screen.queryByText('Passage: Ready to pass')).not.toBeInTheDocument();
  });
  test('keyboard direction selection preserves the candidate and sends no writes', async () => {
    const user = userEvent.setup(); const roomActions = actions();
    render(<DomeConnectionPanel actions={roomActions} room={localRoom} rooms={[localRoom, receiverRoom]} localAuthorPubkey={owner} locale='en' />);
    await user.click(await screen.findByRole('button', { name: /^North/ }));
    await user.keyboard('{ArrowRight}');
    expect(screen.getByRole('button', { name: /^East/ })).toHaveFocus();
    await user.selectOptions(screen.getByRole('combobox'), receiverRoom.room_id);
    await user.click(screen.getByRole('button', { name: /^West/ }));
    await user.click(screen.getByRole('button', { name: /^East/ }));
    expect(screen.getByRole('combobox')).toHaveValue(receiverRoom.room_id);
    expect(roomActions.createConnectionProposal).not.toHaveBeenCalled();
  });

  test('retries a proposal with the same ID and invalidates a removed candidate', async () => {
    const user = userEvent.setup(); const roomActions = actions();
    roomActions.createConnectionProposal.mockRejectedValue(new Error('result unknown'));
    const props = { actions: roomActions, room: localRoom, rooms: [localRoom, receiverRoom], localAuthorPubkey: owner, locale: 'en' as const };
    const { rerender } = render(<DomeConnectionPanel {...props} />);
    await user.selectOptions(await screen.findByRole('combobox'), receiverRoom.room_id);
    await user.click(screen.getByRole('button', { name: 'Propose Connection' }));
    await screen.findByText('result unknown');
    await user.click(screen.getByRole('button', { name: 'Propose Connection' }));
    expect(roomActions.createConnectionProposal.mock.calls[0]).toEqual(roomActions.createConnectionProposal.mock.calls[1]);
    rerender(<DomeConnectionPanel {...props} rooms={[localRoom]} />);
    expect(screen.queryByRole('button', { name: 'Propose Connection' })).not.toBeInTheDocument();
  });

  test('disconnect is explicit and non-owners cannot mutate connections', async () => {
    const topology = connectionFixtureTopology(); const roomActions = actions(topology);
    const user = userEvent.setup(); const room = connectionFixtureRooms[0];
    const props = { room, rooms: connectionFixtureRooms, actions: roomActions, localAuthorPubkey: room.host_pubkey, locale: 'en' as const };
    const { rerender } = render(<DomeConnectionPanel {...props} />);
    await user.click(screen.getByRole('button', { name: /^East/ }));
    await user.click(await screen.findByRole('button', { name: 'Disconnect' }));
    expect(roomActions.revokeConnection).toHaveBeenCalledExactlyOnceWith(room.metaverse!.spatial_context, 'a-b');
    rerender(<DomeConnectionPanel {...props} localAuthorPubkey='visitor' />);
    await user.click(screen.getByRole('button', { name: /^East/ }));
    expect(screen.queryByRole('button', { name: 'Disconnect' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Propose Connection' })).not.toBeInTheDocument();
  });

  test('only the correct endpoint can accept or withdraw a proposal', async () => {
    const topology = connectionFixtureTopology(); const agreement = topology.connections[0].record.agreement;
    topology.connections = []; topology.resolution.topology.active_connection_ids = [];
    topology.proposals = [{ proposal: { proposal_id: 'incoming', spatial_context: agreement.spatial_context,
      proposer: agreement.proposer, receiver: agreement.receiver, sequence: 1, created_at: 1 },
      selection: null, status: 'proposed', terminal_reason: null, connection_id: 'a-b' }];
    const roomActions = actions(topology); const user = userEvent.setup();
    const { rerender } = render(<DomeConnectionPanel actions={roomActions} room={connectionFixtureRooms[1]} rooms={connectionFixtureRooms} localAuthorPubkey={connectionFixtureRooms[1].host_pubkey} locale='en' />);
    await user.click(screen.getByRole('button', { name: /^West/ }));
    await user.click(await screen.findByRole('button', { name: 'Accept' }));
    expect(roomActions.acceptConnectionProposal).toHaveBeenCalledExactlyOnceWith(agreement.spatial_context, 'incoming');
    expect(screen.queryByRole('button', { name: 'Withdraw' })).not.toBeInTheDocument();
    rerender(<DomeConnectionPanel actions={roomActions} room={connectionFixtureRooms[0]} rooms={connectionFixtureRooms} localAuthorPubkey={connectionFixtureRooms[0].host_pubkey} locale='en' />);
    await user.click(screen.getByRole('button', { name: /^East/ }));
    await user.click(await screen.findByRole('button', { name: 'Withdraw' }));
    expect(roomActions.withdrawConnectionProposal).toHaveBeenCalledExactlyOnceWith(agreement.spatial_context, 'incoming');
  });

  test('does not call an unreceived topology an open slot', async () => {
    const roomActions = actions();
    roomActions.listConnections.mockRejectedValue(new Error('read unavailable'));
    render(<DomeConnectionPanel actions={roomActions} room={localRoom} rooms={[localRoom]}
      localAuthorPubkey={owner} locale='en' />);
    await screen.findByText('read unavailable');
    expect(screen.queryAllByText('Open')).toHaveLength(0);
    expect(roomActions.createConnectionProposal).not.toHaveBeenCalled();
  });

  test('keeps a refresh failure visible after a successful proposal', async () => {
    const user = userEvent.setup();
    const roomActions = actions();
    render(<DomeConnectionPanel actions={roomActions} room={localRoom} rooms={[localRoom, receiverRoom]}
      localAuthorPubkey={owner} locale='en' />);
    await waitFor(() => expect(roomActions.listConnections).toHaveBeenCalled());
    await user.selectOptions(screen.getAllByRole('combobox')[0], receiverRoom.room_id);
    roomActions.listConnections.mockRejectedValue(new Error('refresh unavailable'));
    await user.click(screen.getAllByRole('button', { name: 'Propose Connection' })[0]);
    await waitFor(() => expect(roomActions.createConnectionProposal).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('refresh unavailable')).toBeInTheDocument();
  });

  test('renders four fixed direction slots and creates an opposite-slot proposal', async () => {
    const user = userEvent.setup();
    const roomActions = actions();
    render(
      <DomeConnectionPanel
        actions={roomActions}
        room={localRoom}
        rooms={[localRoom, receiverRoom]}
        localAuthorPubkey={owner}
        locale='en'
      />
    );

    await waitFor(() => expect(roomActions.listConnections).toHaveBeenCalledWith(context));
    for (const direction of ['North', 'East', 'South', 'West']) {
      expect(screen.getByRole('button', { name: new RegExp(`^${direction}`) })).toBeInTheDocument();
    }
    await user.click(screen.getByRole('button', { name: /^East/ }));
    await user.selectOptions(screen.getByRole('combobox'), receiverRoom.room_id);
    await user.click(screen.getByRole('button', { name: 'Propose Connection' }));

    expect(roomActions.createConnectionProposal).toHaveBeenCalledWith(
      expect.stringMatching(/^dome-proposal-/),
      context,
      localRoom.room_id,
      receiverRoom.room_id,
      'east'
    );
  });
});

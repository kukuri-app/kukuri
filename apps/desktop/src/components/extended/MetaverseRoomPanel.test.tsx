import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, test, vi } from 'vitest';
import { useState, type ReactNode } from 'react';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type {
  DesktopApi,
  DomePhysicsSnapshotV1,
  GameRoomView,
  MetaverseRoomEventView,
  SharedRoomObjectV1,
  SyncStatus,
} from '@/lib/api';
import { MetaverseRoomPanel } from './MetaverseRoomPanel';
import { createDefaultMetaverseRoomState } from './metaverse/DomeSceneModel';
import { createMetaverseRoomActions } from '@/shell/actions/metaverse';
import { ColumnFullscreenContext } from '@/shell/ColumnPresentationContext';

vi.mock('./MetaverseScene', () => ({
  MetaverseScene: (props: {
    room: GameRoomView;
    localPeerId: string;
    sharedObject: SharedRoomObjectV1;
    latestChatByPeer: Record<string, { body: string }>;
    connectionState: 'live' | 'stale' | 'recovering' | 'offline';
    onLocalTransform: (transform: {
      roomId: string;
      peerId: string;
      seq: number;
      position: [number, number, number];
      rotation: [number, number, number];
      animation: 'idle' | 'walk' | 'sprint' | 'jump' | 'sitting';
      sentAt: number;
    }) => void;
    onAvatarAssetStatus: (status: 'loading' | 'sample-vrm' | 'blob-vrm' | 'fallback-primitive') => void;
    hud: ReactNode;
  }) => (
    <div aria-label='Metaverse room viewport'>
      <button
        type='button'
        onClick={() =>
          props.onLocalTransform({
            roomId: props.room.room_id,
            peerId: props.localPeerId,
            seq: 12,
            position: [10, 0, 20],
            rotation: [0, 90, 0],
            animation: 'sprint',
            sentAt: 42,
          })
        }
      >
        Emit sprint transform
      </button>
      <button type='button' onClick={() => props.onAvatarAssetStatus('fallback-primitive')}>
        Mark animation fallback
      </button>
      <span>{props.sharedObject.object_id}</span>
      <span>Shared object position: {props.sharedObject.position.join(',')}</span>
      <span>Scene connection: {props.connectionState}</span>
      {Object.entries(props.latestChatByPeer).map(([peerId, bubble]) => (
        <span key={peerId}>{`Bubble ${peerId}: ${bubble.body}`}</span>
      ))}
      {props.hud}
    </div>
  ),
}));

const room: GameRoomView = {
  room_id: 'metaverse-room-1',
  host_pubkey: 'f'.repeat(64),
  title: 'Atrium',
  description: 'Small social space',
  status: 'Waiting',
  phase_label: 'fixed-dome-v1',
  scores: [],
  room_kind: 'metaverse_room',
  metaverse: createDefaultMetaverseRoomState(8),
  dome_hosting: { kind: 'owner_hosted' },
  manifest_blob_hash: 'mock-metaverse-room-1',
  updated_at: 1,
  channel_id: null,
  audience_label: 'Public',
};

function presenceJoinEvent(peerId: string, seq: number): MetaverseRoomEventView {
  return {
    envelope_id: `event-${seq}`,
    content: {
      event_id: `event-${seq}`,
      topic_id: 'kukuri:topic:demo',
      channel_id: null,
      room_id: room.room_id,
      spatial_context: { kind: 'topic', topic_id: 'kukuri:topic:demo' },
      instance_generation: 1,
      session_id: room.room_id,
      peer_id: peerId,
      seq,
      sent_at: seq,
      event: {
        type: 'presence_join',
        presence: {
          room_id: room.room_id,
          peer_id: peerId,
          display_name: null,
          avatar_asset_ref: null,
          joined_at: seq,
          last_seen_at: seq,
        },
      },
    },
    envelope: {},
    received_at: seq,
    source_peer: peerId,
  };
}

function physicsSnapshot(animations: Array<[string, string | null]>): DomePhysicsSnapshotV1 {
  return {
    instance_id: room.metaverse!.instance_id,
    instance_generation: 1,
    lease_epoch: 1,
    session_id: room.room_id,
    host_pubkey: room.host_pubkey,
    sequence: 7,
    simulated_at: 7,
    sleeping: false,
    bodies: [
      {
        entity_id: `avatar:${room.host_pubkey}`,
        kind: 'avatar',
        position: [0, 0, 0],
        rotation: [0, 0, 0],
        linear_velocity: [0, 0, 0],
        animation: 'idle',
        grabbed_by: null,
        expires_at: null,
      },
      ...animations.map(([entityId, animation], index) => ({
        entity_id: entityId,
        kind: 'avatar' as const,
        position: [index, 0, index] as [number, number, number],
        rotation: [0, 90, 0] as [number, number, number],
        linear_velocity: [0, 0, 0] as [number, number, number],
        animation,
        grabbed_by: null,
        expires_at: null,
      })),
    ],
  };
}

function presenceLeaveEvent(peerId: string, seq: number): MetaverseRoomEventView {
  return {
    envelope_id: `event-${seq}`,
    content: {
      event_id: `event-${seq}`,
      topic_id: 'kukuri:topic:demo',
      channel_id: null,
      room_id: room.room_id,
      spatial_context: { kind: 'topic', topic_id: 'kukuri:topic:demo' },
      instance_generation: 1,
      session_id: room.room_id,
      peer_id: peerId,
      seq,
      sent_at: seq,
      event: {
        type: 'presence_leave',
        room_id: room.room_id,
        peer_id: peerId,
        left_at: seq,
      },
    },
    envelope: {},
    received_at: seq,
    source_peer: peerId,
  };
}

type RenderPanelOptions = {
  rooms?: GameRoomView[];
  syncStatus?: SyncStatus;
  onRefresh?: () => Promise<void>;
};

function createSyncStatus(): SyncStatus {
  return {
    connected: true,
    delivery_state: 'Live',
    peer_count: 1,
    pending_events: 0,
    status_detail: 'connected',
    configured_peer_count: 0,
    subscribed_topics: ['kukuri:topic:demo'],
    active_path: 'direct_p2p',
    fallback_peer_count: 0,
    topic_diagnostics: [],
    local_author_pubkey: 'f'.repeat(64),
    discovery: {
      mode: 'seeded_dht' as const,
      connect_mode: 'direct_only' as const,
      active_path: 'direct_p2p',
      env_locked: false,
      configured_seed_peer_count: 0,
      bootstrap_seed_peer_count: 0,
      connected_peer_count: 0,
      docs_assist_peer_count: 0,
      blob_assist_peer_count: 0,
      local_endpoint_id: 'local-endpoint-a',
    },
    gossip_disabled_topics: [],
    gossip_disabled_channels: [],
  };
}

function panelElement(api: DesktopApi, options: RenderPanelOptions = {}) {
  const effectiveSyncStatus = options.syncStatus ?? createSyncStatus();
  const onRefresh = options.onRefresh ?? vi.fn();
  return (
    <MetaverseRoomPanel
      actions={createMetaverseRoomActions({
        api,
        activeTopic: 'kukuri:topic:demo',
        activeComposeChannel: { kind: 'public' },
        onRefresh,
      })}
      activeTopic='kukuri:topic:demo'
      rooms={options.rooms ?? [room]}
      syncStatus={effectiveSyncStatus}
      locale='en'
      localProfile={{
        pubkey: effectiveSyncStatus.local_author_pubkey,
        name: 'host',
        display_name: 'Host Author',
        about: null,
        picture_asset: null,
        updated_at: 1,
      }}
    />
  );
}

function renderPanel(api: DesktopApi, options: RenderPanelOptions = {}) {
  return render(panelElement(api, options));
}

async function openCategory(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(screen.getByRole('button', { name: 'Menu (Tab)' }));
  await user.click(screen.getByRole('button', { name }));
}

describe('MetaverseRoomPanel animation sharing', () => {
  test('fullscreen tools preserve the admitted session and draft without repeating presence or host actions', async () => {
    const api = createDesktopMockApi();
    const publish = vi.spyOn(api, 'publishMetaverseRoomEvent');
    const admission = vi.spyOn(api, 'submitDomeSessionInput');
    const startHost = vi.spyOn(api, 'startOwnerDomeHosting');
    const delegateHost = vi.spyOn(api, 'delegateDomeHosting');
    const closeHost = vi.spyOn(api, 'closeDomeHosting');
    const element = panelElement(api);
    const view = render(<ColumnFullscreenContext.Provider value={false}>{element}</ColumnFullscreenContext.Provider>);
    const scene = await screen.findByLabelText('Metaverse room viewport');
    await userEvent.click(screen.getByRole('button', { name: 'Open room chat' }));
    const input = screen.getByRole('textbox', { name: 'Room chat message' });
    await userEvent.type(input, 'unsent 1023');
    const beforeEvents = publish.mock.calls.length;
    const beforeAdmission = admission.mock.calls.length;
    view.rerender(<ColumnFullscreenContext.Provider value>{element}</ColumnFullscreenContext.Provider>);
    await userEvent.click(screen.getByRole('button', { name: 'Dome tools' }));
    await userEvent.click(screen.getAllByRole('button', { name: 'Close Dome tools' })[1]);
    view.rerender(<ColumnFullscreenContext.Provider value={false}>{element}</ColumnFullscreenContext.Provider>);
    expect(screen.getByLabelText('Metaverse room viewport')).toBe(scene);
    expect(input).toHaveValue('unsent 1023');
    expect(publish.mock.calls.length).toBe(beforeEvents);
    expect(admission.mock.calls.length).toBe(beforeAdmission);
    expect(startHost).not.toHaveBeenCalled();
    expect(delegateHost).not.toHaveBeenCalled();
    expect(closeHost).not.toHaveBeenCalled();
  });
  test('requires a new authoritative admission when the same room id has a new generation', async () => {
    const api = createDesktopMockApi();
    const input = vi.spyOn(api, 'submitDomeSessionInput');
    const view = renderPanel(api);
    await screen.findByLabelText('Metaverse room viewport');
    input.mockImplementation(() => new Promise(() => undefined));
    view.rerender(panelElement(api, { rooms: [{ ...room, metaverse: { ...room.metaverse!, instance_generation: 2 } }] }));
    expect(screen.queryByLabelText('Metaverse room viewport')).not.toBeInTheDocument();
  });
  test('does not render a room before authoritative admission completes', async () => {
    const baseApi = createDesktopMockApi();
    const publishMetaverseRoomEvent = vi.fn(baseApi.publishMetaverseRoomEvent);
    let resolveAdmission!: (snapshot: DomePhysicsSnapshotV1) => void;
    const api: DesktopApi = {
      ...baseApi,
      publishMetaverseRoomEvent,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
      submitDomeSessionInput: vi.fn(() => new Promise<DomePhysicsSnapshotV1>((resolve) => {
        resolveAdmission = resolve;
      })),
    };

    renderPanel(api);

    await waitFor(() => expect(api.submitDomeSessionInput).toHaveBeenCalled());
    expect(screen.queryByLabelText('Metaverse room viewport')).not.toBeInTheDocument();
    expect(publishMetaverseRoomEvent).not.toHaveBeenCalled();
    resolveAdmission(await baseApi.submitDomeSessionInput(
      room.metaverse!.spatial_context,
      room.metaverse!.instance_id,
      Date.now(),
      { type: 'join', avatar_collider: null }
    ));
    await waitFor(() => expect(screen.getByLabelText('Metaverse room viewport')).toBeInTheDocument());
  });

  test('automatically joins the preferred available room', async () => {
    const baseApi = createDesktopMockApi();
    const publishMetaverseRoomEvent = vi.fn(baseApi.publishMetaverseRoomEvent);
    const api: DesktopApi = {
      ...baseApi,
      publishMetaverseRoomEvent,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await waitFor(() => expect(screen.getByLabelText('Metaverse room viewport')).toBeInTheDocument());
    await waitFor(() => {
      expect(
        publishMetaverseRoomEvent.mock.calls.some(([, , , , event]) => event.type === 'presence_join')
      ).toBe(true);
    });
  });

  test('leaves a joined room and publishes presence leave', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const publishMetaverseRoomEvent = vi.fn(baseApi.publishMetaverseRoomEvent);
    const api: DesktopApi = {
      ...baseApi,
      publishMetaverseRoomEvent,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await waitFor(() => {
      expect(screen.getByLabelText('Metaverse room viewport')).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: 'Leave room' }));

    expect(screen.queryByLabelText('Metaverse room viewport')).not.toBeInTheDocument();
    await waitFor(() => {
      expect(
        publishMetaverseRoomEvent.mock.calls.some(
          ([, roomId, , , event]) => roomId === room.room_id && event.type === 'presence_leave'
        )
      ).toBe(true);
    });
  });

  test('room HUD can be collapsed without a resize mode', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const api: DesktopApi = {
      ...baseApi,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));

    expect(screen.getByText(/Topic: demo/)).not.toBeVisible();
    await openCategory(user, 'Diagnostics');
    expect(screen.getByText(/Topic: demo/)).toBeInTheDocument();

    expect(document.querySelector('.metaverse-room-hud')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Expand room HUD' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Shrink room HUD' })).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Close' }));
    expect(document.querySelector('.metaverse-room-hud')).not.toBeVisible();
    await openCategory(user, 'Diagnostics');
    expect(document.querySelector('.metaverse-room-hud')).toBeInTheDocument();
  });

  test('room chat can be closed and reopened', async () => {
    const user = userEvent.setup();
    const api = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));

    if (!screen.queryByLabelText('ROOM Chat')) await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    expect(screen.getByLabelText('ROOM Chat')).toBeInTheDocument();
    if (!screen.queryByLabelText('ROOM Chat')) await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    await user.click(screen.getByRole('button', { name: 'Hide room chat' }));
    expect(screen.queryByLabelText('ROOM Chat')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    if (!screen.queryByLabelText('ROOM Chat')) await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    expect(screen.getByLabelText('ROOM Chat')).toBeInTheDocument();
  });

  test('enter opens room chat and focuses the message input while playing', async () => {
    const user = userEvent.setup();
    const api = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    if (!screen.queryByLabelText('ROOM Chat')) await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    await user.click(screen.getByRole('button', { name: 'Hide room chat' }));
    expect(screen.queryByLabelText('ROOM Chat')).not.toBeInTheDocument();

    const scene = screen.getByLabelText('Metaverse room viewport');
    const stage = scene.closest<HTMLElement>('.metaverse-room-stage');
    expect(stage).not.toBeNull();
    stage?.focus();
    await waitFor(() => expect(stage).toHaveFocus());
    await user.keyboard('{Enter}');

    const input = await screen.findByPlaceholderText('Say something in the room');
    await waitFor(() => {
      expect(input).toHaveFocus();
    });
  });

  test('enter inside an editable control does not open room chat', async () => {
    const user = userEvent.setup();
    const api = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api, { rooms: [{ ...room, host_pubkey: 'e'.repeat(64) }] });
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    if (!screen.queryByLabelText('ROOM Chat')) await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    await user.click(screen.getByRole('button', { name: 'Hide room chat' }));
    await user.click(screen.getByRole('button', { name: 'Create metaverse room' }));
    await user.click(screen.getByPlaceholderText('Atrium'));
    await user.keyboard('{Enter}');

    expect(screen.queryByLabelText('ROOM Chat')).not.toBeInTheDocument();
  });

  test('keeps create room controls collapsed until opened', async () => {
    const user = userEvent.setup();
    const api = createDesktopMockApi();

    renderPanel(api, { rooms: [] });

    expect(screen.queryByPlaceholderText('Atrium')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Create metaverse room' }));
    expect(screen.getByPlaceholderText('Atrium')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Small social space')).toBeInTheDocument();
  });

  test('moves the owner Dome and refreshes discovery after completion', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const moveDome = vi.fn().mockResolvedValue(null as never);
    const onRefresh = vi.fn().mockResolvedValue(undefined);

    renderPanel({ ...baseApi, moveDome }, { onRefresh });
    await user.click(screen.getByRole('button', { name: 'Move Dome' }));
    await user.type(screen.getByPlaceholderText('kukuri:topic:...'), 'kukuri:topic:target');
    await user.click(screen.getByRole('button', { name: 'Start move' }));

    await waitFor(() => {
      expect(moveDome).toHaveBeenCalledWith(
        'kukuri:topic:demo',
        expect.stringMatching(/^dome-move-/),
        room.room_id,
        { kind: 'topic', topic_id: 'kukuri:topic:target' }
      );
      expect(onRefresh).toHaveBeenCalledTimes(1);
    });
  });

  test('validates and submits the normalized create-room payload', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const createMetaverseRoom = vi.fn().mockResolvedValue('created-room');
    const api: DesktopApi = {
      ...baseApi,
      createMetaverseRoom,
    };

    renderPanel(api, { rooms: [] });
    await user.click(screen.getByRole('button', { name: 'Create metaverse room' }));
    await user.click(screen.getAllByRole('button', { name: 'Create metaverse room' })[1]);

    expect(screen.getByText('Room title is required')).toBeInTheDocument();
    expect(createMetaverseRoom).not.toHaveBeenCalled();

    await user.type(screen.getByPlaceholderText('Atrium'), '  Contract room  ');
    await user.type(screen.getByPlaceholderText('Small social space'), '  Contract space  ');
    const maxPeersInput = screen.getByLabelText('Max peers');
    await user.clear(maxPeersInput);
    await user.type(maxPeersInput, '12');
    await user.click(screen.getAllByRole('button', { name: 'Create metaverse room' })[1]);

    expect(createMetaverseRoom).toHaveBeenCalledWith(
      'kukuri:topic:demo',
      'Contract room',
      'Contract space',
      12,
      { kind: 'public' }
    );
  });

  test.each([
    [new Error('concrete create failure'), 'concrete create failure'],
    [null, 'Failed to create metaverse room'],
  ])('preserves create-room error priority for %p', async (failure, expectedMessage) => {
    const user = userEvent.setup();
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      createMetaverseRoom: vi.fn().mockRejectedValue(failure),
    };

    renderPanel(api, { rooms: [] });
    await user.click(screen.getByRole('button', { name: 'Create metaverse room' }));
    await user.type(screen.getByPlaceholderText('Atrium'), 'Contract room');
    await user.click(screen.getAllByRole('button', { name: 'Create metaverse room' })[1]);

    expect(await screen.findByText(expectedMessage)).toBeInTheDocument();
  });

  test('opens management after creation and waits for explicit hosting before admission', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const createdRoom = {
      ...room,
      room_id: 'created-metaverse-room',
      title: 'Created room',
      manifest_blob_hash: 'mock-created-metaverse-room',
    };
    const api: DesktopApi = {
      ...baseApi,
      createMetaverseRoom: vi.fn().mockResolvedValue(createdRoom.room_id),
      publishMetaverseRoomEvent: vi.fn(baseApi.publishMetaverseRoomEvent),
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };
    function CreatedRoomHarness() {
      const [rooms, setRooms] = useState<GameRoomView[]>([]);
      return panelElement(api, {
        rooms,
        onRefresh: async () => {
          setRooms([createdRoom]);
        },
      });
    }

    render(<CreatedRoomHarness />);
    await user.click(screen.getByRole('button', { name: 'Create metaverse room' }));
    await user.type(screen.getByPlaceholderText('Atrium'), 'Created room');
    await user.click(screen.getAllByRole('button', { name: 'Create metaverse room' })[1]);

    expect(screen.queryByLabelText('Metaverse room viewport')).not.toBeInTheDocument();
    expect(api.publishMetaverseRoomEvent).not.toHaveBeenCalled();
    await user.click(await screen.findByRole('button', { name: 'Start hosting and enter' }));
    await waitFor(() => {
      expect(screen.getByLabelText('Metaverse room viewport')).toBeInTheDocument();
    });
    await waitFor(() => {
      expect(api.publishMetaverseRoomEvent).toHaveBeenCalled();
    });
  });

  test('renders host identity from profile data', () => {
    const api = createDesktopMockApi();

    renderPanel(api);

    expect(screen.getByText('Host: Host Author')).toBeInTheDocument();
    expect(screen.getByText('Host: Host Author').previousElementSibling).not.toHaveAttribute(
      'data-avatar-src'
    );
  });

  test('normalizes backend avatar animation states', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const api: DesktopApi = {
      ...baseApi,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
      submitDomeSessionInput: vi.fn().mockResolvedValue(physicsSnapshot([
        ['idle-remote', 'idle'],
        ['walk-remote', 'walk'],
        ['sprint-remote', 'sprint'],
        ['jump-remote', 'jump'],
        ['sitting-remote', 'sitting'],
        ['unknown-remote', 'dancing'],
      ])),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await openCategory(user, 'Diagnostics');

    await waitFor(() => {
      expect(screen.getByText(/idle-rem:idle/)).toBeInTheDocument();
      expect(screen.getByText(/walk-rem:walk/)).toBeInTheDocument();
      expect(screen.getByText(/sprint-r:sprint/)).toBeInTheDocument();
      expect(screen.getByText(/jump-rem:jump/)).toBeInTheDocument();
      expect(screen.getByText(/sitting-:sitting/)).toBeInTheDocument();
      expect(screen.getByText(/unknown-:idle/)).toBeInTheDocument();
    });
  });

  test('removes a remote avatar when a presence leave event arrives', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const api: DesktopApi = {
      ...baseApi,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([
        presenceJoinEvent('leaving-remote', 1),
        presenceLeaveEvent('leaving-remote', 2),
      ]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await openCategory(user, 'Diagnostics');

    await waitFor(() => {
      expect(screen.getByText('Remote animation: none')).toBeInTheDocument();
    });
  });

  test('submits local movement to the authoritative host', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const submitDomeSessionInput = vi.fn(baseApi.submitDomeSessionInput);
    const api: DesktopApi = {
      ...baseApi,
      submitDomeSessionInput,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await user.click(screen.getByRole('button', { name: 'Emit sprint transform' }));

    await waitFor(() => {
      expect(submitDomeSessionInput).toHaveBeenCalledWith(
        room.metaverse!.spatial_context,
        room.metaverse!.instance_id,
        expect.any(Number),
        {
          type: 'move',
          position: [10, 0, 20],
          rotation: [0, 90, 0],
          animation: 'sprint',
        },
        room.metaverse!.instance_generation
      );
    });
  });

  test('keeps room panel usable when animation playback falls back', async () => {
    const user = userEvent.setup();
    const api = createDesktopMockApi();

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await user.click(screen.getByRole('button', { name: 'Mark animation fallback' }));
    await openCategory(user, 'Diagnostics');

    expect(screen.getByLabelText('Metaverse room viewport')).toBeInTheDocument();
    expect(screen.getByText(/fallback-primitive/)).toBeInTheDocument();
  });

  test('imports a VRM avatar and resolves its blob preview through the API boundary', async () => {
    const user = userEvent.setup();
    const assetRef = {
      kind: 'vrm' as const,
      blob_hash: 'avatar-blob-hash',
      mime_type: 'model/vrm',
      size_bytes: 6,
      name: 'avatar.vrm',
    };
    const importMetaverseRoomAsset = vi.fn().mockResolvedValue(assetRef);
    const getBlobPreviewUrl = vi.fn().mockResolvedValue('blob:avatar-preview');
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      importMetaverseRoomAsset,
      getBlobPreviewUrl,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await openCategory(user, 'Avatar');
    await user.upload(
      screen.getByLabelText('VRM file'),
      new File(['avatar'], 'avatar.vrm', { type: 'model/vrm' })
    );

    await waitFor(() => {
      expect(importMetaverseRoomAsset).toHaveBeenCalledWith(
        'kukuri:topic:demo',
        room.room_id,
        'vrm',
        'model/vrm',
        'avatar.vrm',
        expect.any(String)
      );
      expect(getBlobPreviewUrl).toHaveBeenCalledWith('avatar-blob-hash', 'model/vrm', 'vrm');
    });
    await openCategory(user, 'Diagnostics');
    expect(screen.getByText('Blob asset resolve: avatar-blob-hash')).toBeInTheDocument();
  });

  test('advances the backend poll cursor from the latest received envelope', async () => {
    const listMetaverseRoomEvents = vi
      .fn()
      .mockResolvedValueOnce([presenceJoinEvent('remote-peer', 1)])
      .mockResolvedValue([]);
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents,
    };

    renderPanel(api);

    await waitFor(() => expect(listMetaverseRoomEvents).toHaveBeenCalledWith(
      'kukuri:topic:demo',
      room.room_id,
      'event-1',
      64
    ));
    expect(listMetaverseRoomEvents).toHaveBeenNthCalledWith(
      1,
      'kukuri:topic:demo',
      room.room_id,
      null,
      64
    );
    expect(listMetaverseRoomEvents).toHaveBeenNthCalledWith(
      2,
      'kukuri:topic:demo',
      room.room_id,
      'event-1',
      64
    );
  });

  test('closes the room BroadcastChannel when the panel unmounts', async () => {
    const close = vi.fn();
    const postMessage = vi.fn();
    class MockBroadcastChannel {
      onmessage: ((event: MessageEvent) => void) | null = null;
      readonly name: string;

      constructor(name: string) {
        this.name = name;
      }

      postMessage = postMessage;
      close = close;
    }
    vi.stubGlobal(
      'BroadcastChannel',
      MockBroadcastChannel as unknown as typeof BroadcastChannel
    );
    const api: DesktopApi = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    try {
      const view = renderPanel(api);
      await waitFor(() => {
        expect(postMessage).toHaveBeenCalled();
      });

      view.unmount();

      expect(close).toHaveBeenCalledTimes(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  test('renders durable room chat history when joining a room', async () => {
    const user = userEvent.setup();
    const api = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };
    const roomWithHistory: GameRoomView = {
      ...room,
      metaverse: {
        ...room.metaverse!,
        chat_history: [
          {
            room_id: room.room_id,
            message_id: 'durable-chat-1',
            author_peer_id: 'peer-history',
            display_name: 'History Peer',
            body: 'durable hello',
            created_at: 12,
          },
        ],
      },
    };

    renderPanel(api, { rooms: [roomWithHistory] });
    await user.click(screen.getByRole('button', { name: 'Join Room' }));

    if (!screen.queryByLabelText('ROOM Chat')) await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    expect(screen.getByLabelText('ROOM Chat')).toBeInTheDocument();
    expect(screen.getByText('History Peer')).toBeInTheDocument();
    expect(screen.getByText('durable hello')).toBeInTheDocument();
  });

  test('shows offline room status when sync connectivity is unhealthy', async () => {
    const user = userEvent.setup();
    const api = {
      ...createDesktopMockApi(),
      listMetaverseRoomEvents: vi.fn().mockRejectedValue(new Error('poll failed')),
    };
    const offlineStatus = {
      ...createSyncStatus(),
      connected: false,
      delivery_state: 'Offline' as const,
      peer_count: 0,
    };

    renderPanel(api, { syncStatus: offlineStatus });
    await user.click(screen.getByRole('button', { name: 'Join Room' }));

    expect(screen.getByText('External peers unavailable')).toBeInTheDocument();
    expect(screen.getByText('Scene connection: offline')).toBeInTheDocument();
  });

  test('sends room chat to the log, backend event, and avatar bubble', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const publishMetaverseRoomEvent = vi.fn(baseApi.publishMetaverseRoomEvent);
    const api: DesktopApi = {
      ...baseApi,
      publishMetaverseRoomEvent,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await user.click(screen.getByRole('button', { name: 'Open room chat' }));
    await user.type(screen.getByPlaceholderText('Say something in the room'), 'hello room');
    await user.click(screen.getByRole('button', { name: 'Send' }));

    expect(screen.getByText('hello room')).toBeInTheDocument();
    expect(screen.getByText(/Bubble .*hello room/)).toBeInTheDocument();
    await waitFor(() => {
      expect(
        publishMetaverseRoomEvent.mock.calls.some(
          ([, , , , event]) => event.type === 'chat_message' && event.message.body === 'hello room'
        )
      ).toBe(true);
    });
  });

  test('shared object movement is submitted as host-authoritative input', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const submitDomeSessionInput = vi.fn(baseApi.submitDomeSessionInput);
    const api: DesktopApi = {
      ...baseApi,
      submitDomeSessionInput,
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    renderPanel(api);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    await openCategory(user, 'Shared objects');
    await user.click(screen.getByRole('button', { name: /Forward/ }));

    await waitFor(() => {
      expect(
        submitDomeSessionInput.mock.calls.some(
          ([, , , input]) => input.type === 'push' && input.impulse[2] === -50
        )
      ).toBe(true);
    });
    expect(screen.getByLabelText('Metaverse room viewport')).toBeInTheDocument();
  });

  test('shared object optimistic movement is not rolled back by a stale refreshed room', async () => {
    const user = userEvent.setup();
    const baseApi = createDesktopMockApi();
    const api: DesktopApi = {
      ...baseApi,
      updateMetaverseRoom: vi.fn().mockResolvedValue(undefined),
      listMetaverseRoomEvents: vi.fn().mockResolvedValue([]),
    };

    function StaleRefreshHarness() {
      const [rooms, setRooms] = useState<GameRoomView[]>([room]);
      return panelElement(api, {
        rooms,
        onRefresh: async () => {
          setRooms([{ ...room }]);
        },
      });
    }

    render(<StaleRefreshHarness />);
    await user.click(screen.getByRole('button', { name: 'Join Room' }));
    expect(screen.getByText('Shared object position: 0,50,-240')).toBeInTheDocument();

    await openCategory(user, 'Shared objects');
    await user.click(screen.getByRole('button', { name: /Forward/ }));

    await waitFor(() => {
      expect(screen.getByText('Shared object position: 0,50,-290')).toBeInTheDocument();
    });
  });
});

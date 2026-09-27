import type { Meta, StoryObj } from '@storybook/react-vite';
import { createRef } from 'react';
import { ColumnFullscreenContext } from '@/shell/ColumnPresentationContext';
import { MetaverseRoomLayout } from './metaverse/MetaverseRoomLayout';

import type { GameRoomView, SyncStatus } from '@/lib/api';
import i18n from '@/i18n';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { MetaverseRoomPanel } from './MetaverseRoomPanel';
import { DEFAULT_SHARED_OBJECT } from './MetaverseSceneModel';
import { MetaverseRoomDiscovery } from './metaverse/MetaverseRoomDiscovery';
import { MetaverseRoomView } from './metaverse/MetaverseRoomView';
import type { MetaverseOverlay, MetaverseCategory } from './metaverse/MetaverseCategories';
import { createDefaultMetaverseRoomState } from './metaverse/DomeSceneModel';
import type { DomeNeighborTransitionView } from './metaverse/DomeTransitionModel';
import { createMetaverseRoomActions } from '@/shell/actions/metaverse';
import { ONLINE_DOME_RECOVERY, type DomeRecoveryStatus } from './metaverse/useMetaverseRoomSession';

const meta = {
  title: 'Extended/MetaverseRoomPanel',
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta;

export default meta;

type Story = StoryObj<typeof meta>;

const STORY_TIMESTAMP = 1_742_860_800_000;
const LOCAL_PUBKEY = 'f'.repeat(64);

const room: GameRoomView = {
  room_id: 'metaverse-room-1',
  host_pubkey: LOCAL_PUBKEY,
  title: 'Atrium',
  description: 'Small social space',
  status: 'Waiting',
  phase_label: 'fixed-dome-v1',
  scores: [],
  room_kind: 'metaverse_room',
  metaverse: createDefaultMetaverseRoomState(8),
  dome_hosting: { kind: 'owner_hosted' },
  manifest_blob_hash: 'mock-metaverse-room-1',
  updated_at: STORY_TIMESTAMP,
  channel_id: null,
  audience_label: 'Public',
};

const neighborRoom: GameRoomView = {
  ...room,
  room_id: 'metaverse-room-north',
  title: 'Northern Garden',
  metaverse: createDefaultMetaverseRoomState(8, { roomId: 'metaverse-room-north' }),
  manifest_blob_hash: 'mock-metaverse-room-north',
};

const readyNorthNeighbor: DomeNeighborTransitionView = {
  connectionId: 'story-connection-north',
  topologyDigest: 'story-topology',
  direction: 'north',
  targetDirection: 'south',
  room: neighborRoom,
  relativeCoordinateCm: [0, 0, -5_700],
  boundaryState: 'ready',
  textureUrls: { wall: null, floor: null },
};

const syncStatus: SyncStatus = {
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
  local_author_pubkey: LOCAL_PUBKEY,
  discovery: {
    mode: 'seeded_dht',
    connect_mode: 'direct_only',
    active_path: 'direct_p2p',
    env_locked: false,
    configured_seed_peer_count: 0,
    bootstrap_seed_peer_count: 1,
    connected_peer_count: 0,
    docs_assist_peer_count: 0,
    blob_assist_peer_count: 0,
    local_endpoint_id: 'local-endpoint-a',
  },
  gossip_disabled_topics: [],
  gossip_disabled_channels: [],
};

function StoryFrame({ children }: { children: React.ReactNode }) {
  return (
      <div style={{ width: '100%', maxWidth: 1180, margin: '0 auto', padding: 24, containerType: 'inline-size' }}>
      <div className='shell-main-stack'>{children}</div>
    </div>
  );
}

function panel(rooms: GameRoomView[]) {
  const api = createDesktopMockApi();
  return (
    <StoryFrame>
      <MetaverseRoomPanel
        actions={createMetaverseRoomActions({
          api,
          activeTopic: 'kukuri:topic:demo',
          activeComposeChannel: { kind: 'public' },
          onRefresh: async () => undefined,
        })}
        activeTopic='kukuri:topic:demo'
        rooms={rooms}
        syncStatus={syncStatus}
        locale='en'
        localProfile={{
          pubkey: LOCAL_PUBKEY,
          name: 'host',
          display_name: 'Host Author',
          about: null,
          picture_asset: null,
          updated_at: STORY_TIMESTAMP,
        }}
      />
    </StoryFrame>
  );
}

function selectedRoom(
  initialHudOpen = true,
  initialChatOpen = true,
  transitionNeighbors: DomeNeighborTransitionView[] = [],
  domeRecovery: DomeRecoveryStatus = ONLINE_DOME_RECOVERY,
  fullscreen = false,
  initialOverlay?: MetaverseOverlay,
  initialCategory?: MetaverseCategory
) {
  return (
    <StoryFrame>
      <ColumnFullscreenContext.Provider value={fullscreen}>
      <div style={fullscreen ? { height: 720 } : undefined}>
      <MetaverseRoomLayout admitted panelRef={createRef<HTMLDivElement>()}
        before={fullscreen ? <p>Dome discovery and management</p> : null}
        after={fullscreen ? <label>Connection draft<input defaultValue='North neighbor' /></label> : null}>
      <MetaverseRoomView
        initialOverlay={initialOverlay}
        initialCategory={initialCategory}
        room={room}
        activeTopic='kukuri:topic:demo'
        localPeerId='local-endpoint-a:story'
        remoteTransforms={{}}
        peerPresence={{}}
        sharedObject={DEFAULT_SHARED_OBJECT}
        avatarAssetUrl={null}
        domeTextureUrls={{ wall: null, floor: null }}
        transitionNeighbors={transitionNeighbors}
        transitionBoundaryStates={Object.fromEntries(
          transitionNeighbors.map((neighbor) => [neighbor.direction, neighbor.boundaryState])
        )}
        latestChatByPeer={{}}
        connectionState='live'
        domeRecovery={domeRecovery}
        now={STORY_TIMESTAMP}
        knownPeerCount={2}
        lastSentSeq={12}
        lastReceivedAt={STORY_TIMESTAMP - 1_000}
        remoteAnimationSummary='remote-pe:walk'
        avatarAssetStatus='sample-vrm'
        localAvatarAssetRef={null}
        communityAssistAvailable={true}
        locale='en'
        pending={false}
        isOwner={true}
        messages={[
          {
            roomId: room.room_id,
            messageId: 'story-message-1',
            authorPeerId: 'remote-peer',
            displayName: 'Remote Friend',
            body: 'Welcome to the room.',
            createdAt: STORY_TIMESTAMP - 2_000,
          },
        ]}
        messageDraft=''
        initialHudOpen={initialHudOpen}
        initialHudDebugOpen={initialHudOpen}
        initialChatOpen={initialChatOpen}
        onLocalTransform={() => undefined}
        onAvatarAssetStatus={() => undefined}
        onLeaveRoom={() => undefined}
        onReturnHome={() => undefined}
        onImportAvatar={() => undefined}
        onImportDefaultAvatar={() => undefined}
        onSaveCustomization={async () => undefined}
        onImportTexture={async () => ({
          kind: 'texture',
          blob_hash: 'story-texture',
          mime_type: 'image/png',
          size_bytes: 1,
          name: 'story-texture.png',
        })}
        onMoveSharedObject={() => undefined}
        onInteractWithProp={() => undefined}
        onMessageDraftChange={() => undefined}
        onSendMessage={(event) => event.preventDefault()}
      />
      </MetaverseRoomLayout>
      </div>
      </ColumnFullscreenContext.Provider>
    </StoryFrame>
  );
}

export const FullscreenRoom: Story = {
  render: () => selectedRoom(true, true, [], ONLINE_DOME_RECOVERY, true),
};

export const EmptyRooms: Story = {
  render: () => panel([]),
};

export const RoomList: Story = {
  render: () => panel([room]),
};

export const DiscoveryError: Story = {
  render: () => (
    <StoryFrame>
      <MetaverseRoomDiscovery
        rooms={[room]}
        selectedRoomId={null}
        joinedRoomIds={new Set()}
        pending={false}
        error={i18n.t('metaverse:connection.details.offline')}
        locale='en'
        localAuthorPubkey={LOCAL_PUBKEY}
        localProfile={null}
        knownAuthorsByPubkey={{}}
        mediaObjectUrls={{}}
        onCreateRoom={async () => false}
        onJoinRoom={() => undefined}
      />
    </StoryFrame>
  ),
};

export const ChannelEntrySelection: Story = {
  render: () => (
    <StoryFrame>
      <MetaverseRoomDiscovery
        rooms={[room, neighborRoom]}
        selectedRoomId={null}
        joinedRoomIds={new Set()}
        pending={false}
        error={null}
        admissionStatus='selection'
        activeChannelId='channel-garden'
        configuredEntryInstanceId={neighborRoom.metaverse!.instance_id}
        canSetEntryDome
        locale='en'
        localAuthorPubkey={LOCAL_PUBKEY}
        localProfile={null}
        knownAuthorsByPubkey={{}}
        mediaObjectUrls={{}}
        onCreateRoom={async () => false}
        onJoinRoom={() => undefined}
        onSetEntryDome={async () => undefined}
      />
    </StoryFrame>
  ),
};

export const SelectedHudAndChat: Story = {
  render: () => selectedRoom(),
};

export const NarrowHudAndChat: Story = {
  render: () => <div style={{ width: 360 }}>{selectedRoom()}</div>,
};

export const SelectedCollapsed: Story = {
  render: () => selectedRoom(false, false),
};

export const CategoryMenu: Story = { render: () => selectedRoom(false, false, [], ONLINE_DOME_RECOVERY, false, 'categories') };
export const NarrowCategoryMenu: Story = { render: () => <div style={{ width: 360 }}>{selectedRoom(false, false, [], ONLINE_DOME_RECOVERY, false, 'categories')}</div> };
export const DomeSettings: Story = { render: () => selectedRoom(true, false, [], ONLINE_DOME_RECOVERY, false, 'details', 'dome') };
export const AvatarSettings: Story = { render: () => selectedRoom(true, false, [], ONLINE_DOME_RECOVERY, false, 'details', 'avatar') };
export const SharedObjects: Story = { render: () => selectedRoom(true, false, [], ONLINE_DOME_RECOVERY, false, 'details', 'objects') };
export const Diagnostics: Story = { render: () => selectedRoom(true, false, [], ONLINE_DOME_RECOVERY, false, 'details', 'diagnostics') };

export const ReadyNorthTransition: Story = {
  render: () => selectedRoom(false, false, [readyNorthNeighbor]),
};

export const OfflineGraceBoundary: Story = {
  render: () => selectedRoom(false, false, [{ ...readyNorthNeighbor, boundaryState: 'offline' }], {
    state: 'offline', secondsRemaining: 9, reason: 'host_offline', targetTitle: null,
  }),
};

export const DrainingBoundary: Story = {
  render: () => selectedRoom(false, false, [{ ...readyNorthNeighbor, boundaryState: 'draining' }]),
};

export const BlockedBoundary: Story = {
  render: () => selectedRoom(false, false, [{ ...readyNorthNeighbor, boundaryState: 'blocked' }]),
};

export const ClosedBoundary: Story = {
  render: () => selectedRoom(false, false, [{ ...readyNorthNeighbor, boundaryState: 'closed' }], {
    state: 'closed', secondsRemaining: 0, reason: 'host_offline', targetTitle: null,
  }),
};

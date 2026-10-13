import type {
  AuthorSocialView,
  DomeCustomizationV1,
  DomeConnectionProposalView,
  DomeConnectionTopologyView,
  DomeConnectionView,
  DomeDirection,
  DomeHostingView,
  DomeLayoutCommitView,
  DomePhysicsSnapshotV1,
  DomeSessionInputKindV1,
  DomeTransitionAdmissionRequestV1,
  DomeTransitionAdmissionTicketV1,
  DomeTransitionAccessDecisionV1,
  GameRoomView,
  MetaverseAssetRef,
  MetaverseAudioStatus,
  MetaverseRoomEventView,
  MetaverseRoomEventV1,
  SetMetaverseAudioRequest,
  SpatialContextV1,
} from '@/lib/api';

export type CreateMetaverseRoomActionInput = {
  title: string;
  description: string;
  maxPeers: number | null;
};

export type MetaverseRoomActions = {
  listPendingDeletions: (context: SpatialContextV1) => ReturnType<import('@/lib/api').DesktopApi['listPendingDomeDeletions']>;
  deleteRoom: (context: SpatialContextV1, instanceId: string, generation: number, operationId: string) => Promise<{ deleted: boolean; cleanup_pending: boolean }>;
  createRoom: (input: CreateMetaverseRoomActionInput) => Promise<string>;
  publishRoomEvent: (
    roomId: string,
    peerId: string,
    seq: number,
    event: MetaverseRoomEventV1
  ) => Promise<void>;
  listRoomEvents: (
    roomId: string,
    afterEnvelopeId: string | null,
    limit: number
  ) => Promise<MetaverseRoomEventView[]>;
  /** 空間音声(ADR 0067)。Tauri の app だけが持つ(機器を使う)。 */
  setAudio?: (request: SetMetaverseAudioRequest) => Promise<MetaverseAudioStatus>;
  /** 話者の mute の状態の正本(空間音声の候補を選ぶ)。 */
  getAuthorSocialView: (pubkey: string) => Promise<AuthorSocialView>;
  importRoomAsset: (
    roomId: string,
    kind: 'vrm' | 'glb' | 'texture',
    mime: string,
    name: string,
    dataBase64: string
  ) => Promise<MetaverseAssetRef>;
  getBlobPreviewUrl: (
    blobHash: string,
    mime: string,
    metaverseKind?: MetaverseAssetRef['kind'] | null
  ) => Promise<string | null>;
  updateRoom: (
    roomId: string,
    status: GameRoomView['status'],
    customization: DomeCustomizationV1
  ) => Promise<void>;
  getHosting: (context: SpatialContextV1, instanceId: string) => Promise<DomeHostingView>;
  startOwnerHosting: (
    context: SpatialContextV1,
    instanceId: string,
    endpointId: string,
    expectedGeneration?: number
  ) => Promise<DomeHostingView>;
  delegateHosting: (
    context: SpatialContextV1,
    instanceId: string,
    nodeId: string,
    baseUrl: string,
    expectedGeneration?: number
  ) => Promise<DomeHostingView>;
  closeHosting: (context: SpatialContextV1, instanceId: string, expectedGeneration?: number) => Promise<DomeHostingView>;
  setChannelEntryDome?: (
    topicId: string,
    channelId: string,
    instanceId: string | null
  ) => Promise<void>;
  submitSessionInput: (
    context: SpatialContextV1,
    instanceId: string,
    sequence: number,
    input: DomeSessionInputKindV1,
    expectedGeneration?: number
  ) => Promise<DomePhysicsSnapshotV1>;
  prepareTransition: (
    request: DomeTransitionAdmissionRequestV1
  ) => Promise<DomeTransitionAdmissionTicketV1>;
  previewTransitionAccess: (
    request: DomeTransitionAdmissionRequestV1
  ) => Promise<DomeTransitionAccessDecisionV1>;
  commitTransition: (
    ticket: DomeTransitionAdmissionTicketV1,
    position: [number, number, number],
    rotation: [number, number, number]
  ) => Promise<void>;
  abortTransition: (ticket: DomeTransitionAdmissionTicketV1) => Promise<void>;
  commitLayout: (
    context: SpatialContextV1,
    instanceId: string,
    operationId: string
  ) => Promise<DomeLayoutCommitView>;
  resyncSnapshots: (
    context: SpatialContextV1,
    instanceId: string,
    afterSequence: number
  ) => Promise<DomePhysicsSnapshotV1[]>;
  moveRoom: (
    moveId: string,
    roomId: string,
    targetContext: SpatialContextV1
  ) => Promise<void>;
  listConnections: (context: SpatialContextV1) => Promise<DomeConnectionTopologyView>;
  createConnectionProposal: (
    proposalId: string,
    context: SpatialContextV1,
    proposerInstanceId: string,
    receiverInstanceId: string,
    direction: DomeDirection
  ) => Promise<DomeConnectionProposalView>;
  acceptConnectionProposal: (
    context: SpatialContextV1,
    proposalId: string
  ) => Promise<DomeConnectionView>;
  withdrawConnectionProposal: (
    context: SpatialContextV1,
    proposalId: string
  ) => Promise<DomeConnectionProposalView>;
  revokeConnection: (
    context: SpatialContextV1,
    connectionId: string
  ) => Promise<DomeConnectionView>;
  refresh: () => Promise<void>;
};

import type { ChannelRef, DesktopApi } from '@/lib/api';
import type { MetaverseRoomActions } from '@/components/extended/metaverse/MetaverseRoomActions';
import { explainScopeLimit } from '@/shell/columnScopeLeases';

type CreateMetaverseRoomActionsArgs = {
  api: DesktopApi;
  activeTopic: string;
  activeComposeChannel: ChannelRef;
  onRefresh: () => Promise<void>;
};

export function createMetaverseRoomActions({
  api,
  activeTopic,
  activeComposeChannel,
  onRefresh,
}: CreateMetaverseRoomActionsArgs): MetaverseRoomActions {
  return {
    listPendingDeletions: (context) => api.listPendingDomeDeletions(context),
    deleteRoom: (context, instanceId, generation, operationId) => api.deleteDome(context, instanceId, generation, operationId),
    createRoom: (input) =>
      api.createMetaverseRoom(
        activeTopic,
        input.title,
        input.description,
        input.maxPeers,
        activeComposeChannel
      ),
    publishRoomEvent: (roomId, peerId, seq, event) =>
      api
        .publishMetaverseRoomEvent(activeTopic, roomId, peerId, seq, event)
        .then(() => undefined),
    listRoomEvents: (roomId, afterEnvelopeId, limit) =>
      api.listMetaverseRoomEvents(activeTopic, roomId, afterEnvelopeId, limit),
    importRoomAsset: (roomId, kind, mime, name, dataBase64) =>
      api.importMetaverseRoomAsset(
        activeTopic,
        roomId,
        kind,
        mime,
        name,
        dataBase64
      ),
    getBlobPreviewUrl: (blobHash, mime, metaverseKind) =>
      api.getBlobPreviewUrl(blobHash, mime, metaverseKind),
    updateRoom: (roomId, status, customization) =>
      api.updateMetaverseRoom(activeTopic, roomId, status, customization),
    getHosting: (context, instanceId) => api.getDomeHosting(context, instanceId),
    startOwnerHosting: (context, instanceId, endpointId, expectedGeneration) =>
      explainScopeLimit(
        api.startOwnerDomeHosting(context, instanceId, endpointId, 86_400_000, expectedGeneration)
      ),
    delegateHosting: (context, instanceId, nodeId, baseUrl, expectedGeneration) =>
      api.delegateDomeHosting(context, instanceId, nodeId, baseUrl, 86_400_000, expectedGeneration),
    closeHosting: (context, instanceId, expectedGeneration) => api.closeDomeHosting(context, instanceId, expectedGeneration),
    setChannelEntryDome: (topicId, channelId, instanceId) =>
      api.setPrivateChannelEntryDome(topicId, channelId, instanceId).then(() => undefined),
    submitSessionInput: (context, instanceId, sequence, input, expectedGeneration) =>
      api.submitDomeSessionInput(context, instanceId, sequence, input, expectedGeneration),
    prepareTransition: (request) => api.prepareDomeTransition(request),
    previewTransitionAccess: (request) => api.previewDomeTransitionAccess(request),
    commitTransition: (ticket, position, rotation) =>
      api.commitDomeTransition(ticket, position, rotation),
    abortTransition: (ticket) => api.abortDomeTransition(ticket),
    commitLayout: (context, instanceId, operationId) =>
      api.commitDomeLayout(context, instanceId, operationId),
    resyncSnapshots: (context, instanceId, afterSequence) =>
      api.resyncDomeSnapshots(context, instanceId, afterSequence),
    moveRoom: (moveId, roomId, targetContext) =>
      api.moveDome(activeTopic, moveId, roomId, targetContext).then(() => undefined),
    listConnections: (context) => api.listDomeConnectionTopology(context),
    createConnectionProposal: (
      proposalId,
      context,
      proposerInstanceId,
      receiverInstanceId,
      direction
    ) =>
      api.createDomeConnectionProposal(
        proposalId,
        context,
        proposerInstanceId,
        receiverInstanceId,
        direction
      ),
    acceptConnectionProposal: (context, proposalId) =>
      api.acceptDomeConnectionProposal(context, proposalId),
    withdrawConnectionProposal: (context, proposalId) =>
      api.withdrawDomeConnectionProposal(context, proposalId),
    revokeConnection: (context, connectionId) =>
      api.revokeDomeConnection(context, connectionId),
    refresh: onRefresh,
  };
}

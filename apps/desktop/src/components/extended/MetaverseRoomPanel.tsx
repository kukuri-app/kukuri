import { useSessionDisplay, type SessionDisplayContext } from './useSessionDisplay';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { FetchCommunityNodePolicyView, AcceptCommunityNodePolicyView } from '@/shell/actions/useCommunityNodePolicyDialog';

import type {
  AuthorSocialView,
  DomeCustomizationV1,
  GameRoomView,
  JoinedPrivateChannelView,
  MetaverseAssetRef,
  Profile,
  SpatialContextV1,
  SyncStatus,
} from '@/lib/api';
import type { SupportedLocale } from '@/i18n';
import type { CommunityNodePanelView } from '@/components/settings/types';
import { Notice } from '@/components/ui/notice';
import { Button } from '@/components/ui/button';
import { blobToBase64 } from '@/lib/attachments';
import {
  MetaverseRoomDiscovery,
  type CreateMetaverseRoomInput,
} from './metaverse/MetaverseRoomDiscovery';
import { MetaverseRoomLayout } from './metaverse/MetaverseRoomLayout';
import { MetaverseRoomView } from './metaverse/MetaverseRoomView';
import { DomeConnectionPanel } from './metaverse/DomeConnectionPanel';
import { PendingDomeDeletions } from './metaverse/PendingDomeDeletions';
import { DomeManagementPanel } from './metaverse/DomeManagementPanel';
import { DomeHostingPanel } from './metaverse/DomeHostingPanel';
import { useMetaverseRoomSession } from './metaverse/useMetaverseRoomSession';
import type { MetaverseRoomActions } from './metaverse/MetaverseRoomActions';
import {
  DEFAULT_AVATAR_ASSET_NAME,
  DEFAULT_AVATAR_ASSET_URL,
  type AvatarAssetStatus,
} from './MetaverseSceneModel';
import { useColumnRuntime } from '@/shell/ColumnRuntimeContext';

type MetaverseRoomPanelProps = {
  sessionDisplay?: SessionDisplayContext;
  loadError?: string | null;
  catalogReady?: boolean;
  actions: MetaverseRoomActions;
  activeTopic: string;
  rooms: GameRoomView[];
  syncStatus: SyncStatus;
  locale: SupportedLocale;
  localProfile?: Profile | null;
  knownAuthorsByPubkey?: Record<string, AuthorSocialView>;
  mediaObjectUrls?: Record<string, string | null>;
  initialSelectedRoomId?: string | null;
  activeChannel?: JoinedPrivateChannelView | null;
  communityNodePanelView?: CommunityNodePanelView;
  onFetchCommunityNodeConsents?: FetchCommunityNodePolicyView;
  onAcceptCommunityNodeConsents?: AcceptCommunityNodePolicyView;
  onOpenCommunityNodeSettings?: () => void;
};

const EMPTY_KNOWN_AUTHORS_BY_PUBKEY: Record<string, AuthorSocialView> = {};

export function MetaverseRoomPanel({
  sessionDisplay,
  loadError = null,
  catalogReady = true,
  actions,
  activeTopic,
  rooms,
  syncStatus,
  locale,
  localProfile = null,
  knownAuthorsByPubkey = EMPTY_KNOWN_AUTHORS_BY_PUBKEY,
  mediaObjectUrls = {},
  initialSelectedRoomId = null,
  activeChannel = null,
  communityNodePanelView,
  onFetchCommunityNodeConsents,
  onAcceptCommunityNodeConsents,
  onOpenCommunityNodeSettings,
}: MetaverseRoomPanelProps) {
  const { t } = useTranslation('metaverse', { lng: locale });
  const columnRuntime = useColumnRuntime();
  const managementContext = useMemo<SpatialContextV1>(() => activeChannel ? { kind: 'channel', topic_id: activeTopic, channel_id: activeChannel.channel_id } : { kind: 'topic', topic_id: activeTopic }, [activeTopic, activeChannel]);
  const [managedId, setManagedId] = useState<string | null>(null);
  const [managementRequest, setManagementRequest] = useState(0);
  const [managedSnapshot, setManagedSnapshot] = useState<GameRoomView | null>(null);
  const [managedScope, setManagedScope] = useState('');
  const scope = `${syncStatus.local_author_pubkey}:${activeTopic}:${activeChannel?.channel_id ?? ''}`;
  const refreshedManaged = rooms.find((r) => r.room_id === managedId && r.host_pubkey === syncStatus.local_author_pubkey
    && (!managedSnapshot || r.metaverse?.instance_generation === managedSnapshot.metaverse?.instance_generation));
  const managementSuperseded = managedScope === scope && managedSnapshot !== null && rooms.some((r) => r.room_id === managedId && r.metaverse?.instance_generation !== managedSnapshot.metaverse?.instance_generation);
  const managedRoom = managedScope === scope && managedId && !managementSuperseded ? refreshedManaged ?? managedSnapshot : null;
  useEffect(() => { if (refreshedManaged) setManagedSnapshot(refreshedManaged); }, [refreshedManaged]);
  const focusIdentity = (roomId: string) => JSON.stringify([scope, roomId, rooms.find((room) => room.room_id === roomId)?.metaverse?.instance_generation]);
  const [focusRoomId, setFocusRoomId] = useState<string | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const managementOrigin = useRef<HTMLElement | null>(null);
  function manageRoom(roomId: string) {
    if (session.admittedRoom?.room_id === roomId) setManagementRequest(value => value + 1);
    managementOrigin.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setManagedSnapshot(rooms.find((r) => r.room_id === roomId) ?? null);
    setManagedScope(scope); setManagedId(roomId);
  }
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [avatarAssetStatus, setAvatarAssetStatus] = useState<AvatarAssetStatus>('loading');
  const [localAvatarAssetRef, setLocalAvatarAssetRef] = useState<MetaverseAssetRef | null>(null);
  const [localAvatarAssetUrl, setLocalAvatarAssetUrl] = useState<string | null>(null);
  const [domeTextureUrls, setDomeTextureUrls] = useState<{ wall: string | null; floor: string | null }>({
    wall: null,
    floor: null,
  });
  const localDisplayName = localProfile?.display_name?.trim() || localProfile?.name?.trim() || null;
  const mutedAuthorPubkeys = useMemo(() => new Set(
    Object.values(knownAuthorsByPubkey)
      .filter((author) => author.muted)
      .map((author) => author.author_pubkey)
  ), [knownAuthorsByPubkey]);
  const session = useMetaverseRoomSession({
    actions,
    managementActive: managedScope === scope && managedId !== null,
    activeTopic,
    rooms,
    syncStatus,
    locale,
    localDisplayName,
    localAvatarAssetRef,
    localAvatarAssetUrl,
    avatarFetchActive:
      columnRuntime.active && columnRuntime.visible && !columnRuntime.suspended,
    mutedAuthorPubkeys,
    initialSelectedRoomId,
    activeChannelId: activeChannel?.channel_id ?? null,
    configuredEntryInstanceId: activeChannel?.entry_dome_instance_id ?? null,
    onError: setError,
  });

  useEffect(() => {
    let cancelled = false;
    const surface = session.selectedRoom?.metaverse?.dome.customization.surface;
    const resolve = async (asset: MetaverseAssetRef | null | undefined) => {
      if (!asset) return null;
      return actions.getBlobPreviewUrl(
        asset.blob_hash,
        asset.mime_type ?? 'image/png',
        asset.kind
      );
    };
    void Promise.all([resolve(surface?.wall_texture), resolve(surface?.floor_texture)])
      .then(([wall, floor]) => {
        if (!cancelled) setDomeTextureUrls({ wall, floor });
      })
      .catch(() => {
        if (!cancelled) setDomeTextureUrls({ wall: null, floor: null });
      });
    return () => {
      cancelled = true;
    };
  }, [actions, session.selectedRoom]);

  const admittedFocus = session.admittedRoom ? JSON.stringify([scope, session.admittedRoom.room_id, session.admittedRoom.metaverse?.instance_generation]) : null;
  useSessionDisplay<HTMLDivElement>({ context: session.admittedRoom ? sessionDisplay : undefined,
    sessionId: session.admittedRoom?.room_id ?? '', kind: 'game', target: panelRef });

  useEffect(() => {
    if (!focusRoomId || admittedFocus !== focusRoomId) return;
    const frame = requestAnimationFrame(() => {
      const stage = panelRef.current?.querySelector<HTMLElement>('.metaverse-room-stage');
      stage?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
      stage?.focus({ preventScroll: true });
      setFocusRoomId(null);
    });
    return () => cancelAnimationFrame(frame);
  }, [focusRoomId, admittedFocus]);

  async function handleCreateRoom(input: CreateMetaverseRoomInput) {
    setPending(true);
    try {
      const roomId = await actions.createRoom(input);
      setError(null);
      manageRoom(roomId);
      await actions.refresh();
      return true;
    } catch (createError) {
      setError(createError instanceof Error ? createError.message : t('errors.createFailed'));
      return false;
    } finally {
      setPending(false);
    }
  }

  async function handleMoveRoom(roomId: string, targetContext: SpatialContextV1) {
    setPending(true);
    try {
      const suffix = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}`;
      await actions.moveRoom(`dome-move-${suffix}`, roomId, targetContext);
      await actions.refresh();
      setError(null);
      return true;
    } catch (moveError) {
      setError(moveError instanceof Error ? moveError.message : t('errors.moveFailed'));
      return false;
    } finally {
      setPending(false);
    }
  }

  async function importAvatarBlob(blob: Blob, name: string) {
    if (!session.selectedRoom) {
      return;
    }
    setPending(true);
    try {
      const mime = blob.type || 'model/vrm';
      const dataBase64 = await blobToBase64(blob);
      const assetRef = await actions.importRoomAsset(
        session.selectedRoom.room_id,
        'vrm',
        mime,
        name,
        dataBase64
      );
      const resolvedUrl =
        (await actions.getBlobPreviewUrl(
          assetRef.blob_hash,
          assetRef.mime_type ?? mime,
          assetRef.kind
        )) ??
        `data:${mime};base64,${dataBase64}`;
      setLocalAvatarAssetRef(assetRef);
      setLocalAvatarAssetUrl(resolvedUrl);
      setError(null);
    } catch (assetError) {
      setError(assetError instanceof Error ? assetError.message : t('errors.importAvatarFailed'));
    } finally {
      setPending(false);
    }
  }

  async function handleSampleAvatarImport() {
    const response = await fetch(DEFAULT_AVATAR_ASSET_URL);
    if (!response.ok) {
      throw new Error(t('errors.sampleFetchFailed', { status: response.status }));
    }
    await importAvatarBlob(await response.blob(), DEFAULT_AVATAR_ASSET_NAME);
  }

  async function importTexture(file: File): Promise<MetaverseAssetRef> {
    if (!session.selectedRoom) {
      throw new Error(t('errors.roomRequired'));
    }
    const mime = file.type || 'image/png';
    return actions.importRoomAsset(
      session.selectedRoom.room_id,
      'texture',
      mime,
      file.name,
      await blobToBase64(file)
    );
  }

  async function saveCustomization(customization: DomeCustomizationV1) {
    if (!session.selectedRoom) return;
    setPending(true);
    try {
      await actions.updateRoom(session.selectedRoom.room_id, session.selectedRoom.status, customization);
      await actions.refresh();
      setError(null);
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : t('errors.customizationFailed'));
      throw saveError;
    } finally {
      setPending(false);
    }
  }

  const managementPanel = managedRoom?.metaverse ? <DomeManagementPanel
        key={`${scope}:${managedRoom.room_id}:${managedRoom.metaverse.instance_generation}`}
        room={managedRoom} actions={actions} endpointId={syncStatus.discovery.local_endpoint_id} locale={locale}
        admitted={session.admittedRoom?.room_id === managedRoom.room_id}
        onEnter={async (roomId) => {
          const joined = await session.joinRoom(roomId);
          if (joined) { setFocusRoomId(focusIdentity(roomId)); setManagedId(null); }
          return joined;
        }}
        onStopped={() => { if (session.admittedRoom?.room_id === managedRoom.room_id) session.leaveRoom(); }}
        onDeleted={() => { if (session.admittedRoom?.room_id === managedRoom.room_id) session.leaveRoom(); setManagedId(null); }}
        onClose={() => { setManagedId(null); managementOrigin.current?.focus(); }}
      /> : null;
  const connectionPanel = <DomeConnectionPanel
        connections={session.admittedRoom ? session.connections : undefined}
        boundaries={session.admittedRoom ? session.transitionBoundaryStates : undefined}
        actions={actions}
        room={session.admittedRoom ?? (managedScope === scope && managedId ? managedRoom : session.selectedRoom)}
        rooms={rooms}
        localAuthorPubkey={syncStatus.local_author_pubkey}
        locale={locale}
      />;

  return (
          <DomeHostingPanel
        key={`hosting:${scope}:${(session.admittedRoom ?? managedRoom ?? session.selectedRoom)?.room_id}:${(session.admittedRoom ?? managedRoom ?? session.selectedRoom)?.metaverse?.instance_generation}`}
        actions={actions}
        room={session.admittedRoom ?? (managedScope === scope && managedId ? managedRoom : session.selectedRoom)}
        localAuthorPubkey={syncStatus.local_author_pubkey}
        localEndpointId={syncStatus.discovery.local_endpoint_id}
        locale={locale}
        onSpawnGuestProp={session.spawnGuestProp}
        onAddPersistentProp={session.addPersistentProp}
        onDeletePersistentProp={session.deletePersistentProp}
        communityNodes={communityNodePanelView?.nodes ?? []}
        onFetchCommunityNodeConsents={onFetchCommunityNodeConsents}
        onAcceptCommunityNodeConsents={onAcceptCommunityNodeConsents}
        onOpenCommunityNodeSettings={onOpenCommunityNodeSettings}
      renderSections={hostingSections => <MetaverseRoomLayout admitted={Boolean(session.admittedRoom)} panelRef={panelRef} before={<>
      <PendingDomeDeletions key={scope} actions={actions} context={managementContext} locale={locale} />
      <MetaverseRoomDiscovery
        requestedSessionId={initialSelectedRoomId}
        sessionDisplay={sessionDisplay}
        rooms={rooms}
        catalogReady={catalogReady}
        onRetry={() => actions.refresh().catch((cause: unknown) => setError(cause instanceof Error ? cause.message : t('management.pendingReadFailed')))}
        selectedRoomId={session.selectedRoomId}
        joinedRoomIds={session.joinedRoomIds}
        pending={pending}
        error={error ?? loadError}
        locale={locale}
        localAuthorPubkey={syncStatus.local_author_pubkey}
        localProfile={localProfile}
        knownAuthorsByPubkey={knownAuthorsByPubkey}
        mediaObjectUrls={mediaObjectUrls}
        onCreateRoom={handleCreateRoom}
        onManageRoom={manageRoom}
        onJoinRoom={async (roomId) => { if (await session.joinRoom(roomId)) setFocusRoomId(focusIdentity(roomId)); }}
        admissionStatus={session.admissionStatus}
        activeChannelId={activeChannel?.channel_id ?? null}
        configuredEntryInstanceId={activeChannel?.entry_dome_instance_id ?? null}
        canSetEntryDome={Boolean(activeChannel?.is_owner)}
        onSetEntryDome={activeChannel && actions.setChannelEntryDome ? async (instanceId) => {
          setPending(true);
          try {
            await actions.setChannelEntryDome!(activeTopic, activeChannel.channel_id, instanceId);
            await actions.refresh();
            setError(null);
          } catch (settingError) {
            setError(settingError instanceof Error ? settingError.message : t('entry.settingFailed'));
          } finally {
            setPending(false);
          }
        } : undefined}
        onMoveRoom={handleMoveRoom}
      />

      {managementSuperseded ? <Notice>{t('management.staleTarget')}<Button variant='secondary' onClick={() => setManagedId(null)}>{t('management.close')}</Button></Notice> : null}
      {!session.admittedRoom && managementPanel}

      </>} after={<>
      {!session.admittedRoom && <>{connectionPanel}{hostingSections.hosting}{hostingSections.objects}{hostingSections.diagnostics}</>}

      </>}>
      <MetaverseRoomView
        room={session.admittedRoom}
        hostingRequest={managedRoom && managedRoom.room_id === session.admittedRoom?.room_id ? managementRequest : 0}
        sessionIdentity={`${scope}:${session.admittedRoom?.room_id}:${session.admittedRoom?.metaverse?.instance_generation}`}
        panels={{
          hosting: <>{hostingSections.hosting}
            {session.admittedRoom?.host_pubkey === syncStatus.local_author_pubkey && <Button type='button' variant='secondary'
              onClick={() => manageRoom(session.admittedRoom!.room_id)}>{t('management.open')}</Button>}
            {managedRoom?.room_id === session.admittedRoom?.room_id ? managementPanel : null}
          </>,
          connections: connectionPanel,
          objects: hostingSections.objects,
          diagnostics: hostingSections.diagnostics,
        }}
        activeTopic={activeTopic}
        localPeerId={session.localPeerId}
        remoteTransforms={session.remoteTransforms}
        peerPresence={session.peerPresence}
        sharedObject={session.sharedObject}
        sessionProps={session.sessionProps}
        avatarAssetUrl={localAvatarAssetUrl}
        domeTextureUrls={domeTextureUrls}
        transitionNeighbors={session.transitionNeighbors}
        transitionBoundaryStates={session.transitionBoundaryStates}
        handoffTransform={session.handoffTransform}
        latestChatByPeer={session.latestChatByPeer}
        connectionState={session.roomConnectionState}
        domeRecovery={session.domeRecovery}
        now={session.clockNow}
        knownPeerCount={session.knownPeerCount}
        lastSentSeq={session.lastSentSeq}
        lastReceivedAt={session.lastReceivedAt}
        remoteAnimationSummary={session.remoteAnimationSummary}
        avatarAssetStatus={avatarAssetStatus}
        localAvatarAssetRef={localAvatarAssetRef}
        communityAssistAvailable={syncStatus.discovery.bootstrap_seed_peer_count > 0}
        locale={locale}
        pending={pending}
        isOwner={session.selectedRoom?.host_pubkey === syncStatus.local_author_pubkey}
        messages={session.messages}
        messageDraft={session.messageDraft}
        onLocalTransform={session.handleLocalTransform}
        onAvatarAssetStatus={setAvatarAssetStatus}
        onLeaveRoom={session.leaveRoom}
        onReturnHome={session.returnHome}
        onImportAvatar={(file) => void importAvatarBlob(file, file.name)}
        onImportDefaultAvatar={() => void handleSampleAvatarImport()}
        onSaveCustomization={saveCustomization}
        onImportTexture={importTexture}
        onMoveSharedObject={session.moveSharedObject}
        onInteractWithProp={session.interactWithProp}
        onMessageDraftChange={session.setMessageDraft}
        onSendMessage={session.handleSendMessage}
        microphoneEnabled={session.microphoneEnabled}
        onToggleMicrophone={session.toggleMicrophone}
      />
    </MetaverseRoomLayout>} />
  );
}

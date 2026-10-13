import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, test, vi } from 'vitest';

import i18n from '@/i18n';
import type { CommunityNodeConsentDocumentRef, GameRoomView } from '@/lib/api';
import { InvokeError } from '@/lib/api/invoke/error';
import type { CommunityNodeEntryView } from '@/components/settings/types';
import { createDefaultMetaverseRoomState } from './DomeSceneModel';
import { DomeHostingPanel } from './DomeHostingPanel';
import type { MetaverseRoomActions } from './MetaverseRoomActions';

afterEach(async () => {
  await i18n.changeLanguage('en');
});

const owner = 'a'.repeat(64);
const room: GameRoomView = {
  room_id: 'dome-consent-room',
  host_pubkey: owner,
  title: 'Consent Dome',
  description: '',
  status: 'Waiting',
  phase_label: 'fixed-dome-v1',
  scores: [],
  room_kind: 'metaverse_room',
  metaverse: createDefaultMetaverseRoomState(8, {
    roomId: 'dome-consent-room',
    topicId: 'kukuri:topic:dome-consent',
    ownerPubkey: owner,
  }),
  manifest_blob_hash: 'manifest-dome-consent',
  updated_at: 1,
  channel_id: null,
  audience_label: 'Public',
};

function node(hasLocalConsent: boolean): CommunityNodeEntryView {
  return {
    id: 'node-entry-1',
    baseUrl: 'https://node.example',
    nodeId: 'f'.repeat(64),
    nodeName: 'Example Node',
    saved: true,
    diagnostics: [],
    dependency: {
      diagnostics: [],
      boundaryNotes: [],
    },
    consent: {
      loaded: true,
      loading: false,
      loadError: null,
      withdrawn: false,
      hasLocalConsent,
      allRequiredAccepted: hasLocalConsent,
      hasPendingUpdate: false,
      policies: [
        {
          policySlug: 'builder-preview',
          title: 'Builder Preview',
          body: 'Builder preview policy body.',
          policyVersion: 2,
          effectiveDate: '2026-09-03',
          language: 'en',
          policySnapshotRevision: 'snapshot-2',
          authoritativeLanguage: 'en',
          referenceTranslation: false,
          fallback: false,
          required: true,
          acceptedAtLabel: hasLocalConsent ? '2026-09-03' : null,
          updated: false,
          previouslyAcceptedVersion: null,
        },
      ],
    },
    distanceOptoutEligible: false,
    inviteCodeSaved: false,
  };
}

function actions(delegateHosting = vi.fn().mockResolvedValue(undefined)) {
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
    getHosting: vi.fn().mockResolvedValue(null),
    startOwnerHosting: vi.fn(),
    delegateHosting,
    closeHosting: vi.fn(),
    submitSessionInput: vi.fn(),
    prepareTransition: vi.fn(),
    previewTransitionAccess: vi.fn(),
    commitTransition: vi.fn(),
    abortTransition: vi.fn(),
    commitLayout: vi.fn(),
    resyncSnapshots: vi.fn(),
    moveRoom: vi.fn(),
    listConnections: vi.fn(),
    createConnectionProposal: vi.fn(),
    acceptConnectionProposal: vi.fn(),
    withdrawConnectionProposal: vi.fn(),
    revokeConnection: vi.fn(),
    refresh: vi.fn().mockResolvedValue(undefined),
  } satisfies MetaverseRoomActions;
}

function renderPanel(options: {
  entry: CommunityNodeEntryView;
  roomActions?: ReturnType<typeof actions>;
  onFetch?: (baseUrl: string) => Promise<void>;
  onAccept?: (
    baseUrl: string,
    documents: CommunityNodeConsentDocumentRef[]
  ) => Promise<void>;
}) {
  const roomActions = options.roomActions ?? actions();
  const onFetch = options.onFetch ?? vi.fn().mockResolvedValue(undefined);
  const onAccept = options.onAccept ?? vi.fn().mockResolvedValue(undefined);
  render(
    <DomeHostingPanel
      actions={roomActions}
      room={room}
      localAuthorPubkey={owner}
      localEndpointId='local-endpoint'
      locale='en'
      onSpawnGuestProp={vi.fn().mockResolvedValue(undefined)}
      onAddPersistentProp={vi.fn().mockResolvedValue(undefined)}
      onDeletePersistentProp={vi.fn().mockResolvedValue(undefined)}
      communityNodes={[options.entry]}
      onFetchCommunityNodeConsents={onFetch}
      onAcceptCommunityNodeConsents={onAccept}
    />
  );
  return { roomActions, onFetch, onAccept };
}

describe('DomeHostingPanel Community Node consent', () => {
  test('keeps a consent failure inside the modal without delegating', async () => {
    const user = userEvent.setup();
    const { roomActions } = renderPanel({ entry: node(false), onAccept: vi.fn().mockRejectedValue(new Error('save failed')) });
    await user.click(screen.getByRole('button', { name: 'Explicitly delegate to Community Node' }));
    const dialog = await screen.findByRole('dialog');
    await user.click(await within(dialog).findByRole('button', { name: 'Accept' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Consent could not be completed');
    expect(roomActions.delegateHosting).not.toHaveBeenCalled();
  });
  test('delegates directly to the manifest identity only with current local consent', async () => {
    const user = userEvent.setup();
    const { roomActions, onFetch } = renderPanel({ entry: node(true) });

    await user.click(screen.getByRole('button', { name: 'Explicitly delegate to Community Node' }));

    await waitFor(() =>
      expect(roomActions.delegateHosting).toHaveBeenCalledWith(
        room.metaverse!.spatial_context,
        room.metaverse!.instance_id,
        'f'.repeat(64),
        'https://node.example',
        room.metaverse!.instance_generation
      )
    );
    expect(onFetch).not.toHaveBeenCalled();
    expect(screen.queryByText('Community Node ID')).not.toBeInTheDocument();
    expect(screen.queryByText('Community Node API URL')).not.toBeInTheDocument();
  });

  test('shows the target policies and resumes the exact delegation only after acceptance', async () => {
    const user = userEvent.setup();
    const { roomActions, onFetch, onAccept } = renderPanel({ entry: node(false) });

    await user.click(screen.getByRole('button', { name: 'Explicitly delegate to Community Node' }));

    expect(onFetch).toHaveBeenCalledWith('https://node.example', 'en');
    expect(roomActions.delegateHosting).not.toHaveBeenCalled();
    await user.click(await screen.findByRole('button', { name: 'Builder Preview' }));
    expect(screen.getByText('Builder preview policy body.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Accept' }));

    await waitFor(() =>
      expect(onAccept).toHaveBeenCalledWith('https://node.example', [
        {
          policy_slug: 'builder-preview',
          policy_version: 2,
          policy_snapshot_revision: 'snapshot-2',
        },
      ], 'en')
    );
    expect(roomActions.delegateHosting).toHaveBeenCalledWith(
      room.metaverse!.spatial_context,
      room.metaverse!.instance_id,
      'f'.repeat(64),
      'https://node.example',
      room.metaverse!.instance_generation
    );
  });

  test('declining the target policies keeps Dome delegation untouched', async () => {
    const user = userEvent.setup();
    const { roomActions } = renderPanel({ entry: node(false) });

    await user.click(screen.getByRole('button', { name: 'Explicitly delegate to Community Node' }));
    await user.click(await screen.findByRole('button', { name: 'Not now' }));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(roomActions.delegateHosting).not.toHaveBeenCalled();
  });

  test('does not expose a free-form target when the saved manifest is unavailable', () => {
    const entry = { ...node(false), nodeId: null, nodeName: null };
    renderPanel({ entry });

    expect(
      screen.getByText("Hosting cannot be delegated until the saved node's public manifest is available.")
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Explicitly delegate to Community Node' })).toBeDisabled();
    expect(screen.queryByPlaceholderText('https://community.example')).not.toBeInTheDocument();
  });

  test('keeps delegation blocked on policy load failure and retries without auth action', async () => {
    const user = userEvent.setup();
    const onFetch = vi
      .fn()
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce(undefined);
    const { roomActions } = renderPanel({ entry: node(false), onFetch });

    await user.click(screen.getByRole('button', { name: 'Explicitly delegate to Community Node' }));
    expect(
      await screen.findByText('Could not load the policies (you may be offline). Retry when the node is reachable.')
    ).toBeInTheDocument();
    expect(screen.getByText('offline')).toBeInTheDocument();
    expect(roomActions.delegateHosting).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(onFetch).toHaveBeenCalledTimes(2));
    expect(roomActions.delegateHosting).not.toHaveBeenCalled();
  });

  test('turns a runtime consent race back into the same policy flow', async () => {
    const user = userEvent.setup();
    const delegateHosting = vi
      .fn()
      .mockRejectedValueOnce(new InvokeError('CONSENT_REQUIRED', 'updated policy', 403))
      .mockResolvedValueOnce(undefined);
    const roomActions = actions(delegateHosting);
    const { onFetch } = renderPanel({ entry: node(true), roomActions });

    await user.click(screen.getByRole('button', { name: 'Explicitly delegate to Community Node' }));

    await waitFor(() => expect(onFetch).toHaveBeenCalledWith('https://node.example', 'en'));
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(delegateHosting).toHaveBeenCalledTimes(1);
  });
});

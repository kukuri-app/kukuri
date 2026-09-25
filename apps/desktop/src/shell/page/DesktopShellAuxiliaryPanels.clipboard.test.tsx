import { fireEvent, render, screen } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

import {
  DesktopShellMessagesSurface,
  type DesktopShellMessagesSurfaceProps,
} from './DesktopShellAuxiliaryPanels';

test('auxiliary Conversation composer forwards clipboard images and leaves text paste native', () => {
  const peerPubkey = 'b'.repeat(64);
  const store = createDesktopShellStore();
  store.getState().patchState({
    selectedDirectMessagePeerPubkey: peerPubkey,
    directMessageStatusByPeer: {
      [peerPubkey]: {
        peer_pubkey: peerPubkey,
        dm_id: 'dm-1',
        mutual: true,
        send_enabled: true,
        pending_outbox_count: 64,
        pending_outbox_has_more: true,
      },
    },
  });
  const handleDirectMessageAttachmentPaste = vi.fn(async () => undefined);
  const viewModels = {
    directMessageDraftViews: [],
    selectedDirectMessagePeerLabel: 'Bob',
    selectedDirectMessagePeerPicture: null,
    selectedDirectMessageStatus: store.getState().directMessageStatusByPeer[peerPubkey],
    selectedDirectMessageTimeline: [],
    localDirectMessageAuthorPicture: null,
  } satisfies DesktopShellMessagesSurfaceProps['viewModels'];

  render(
    <DesktopShellStoreContext.Provider value={store}>
      <DesktopShellMessagesSurface
        t={(key, options) =>
          key === 'shell:messages.pendingOutbox' ? String(options?.count) : key
        }
        locale='en'
        viewModels={viewModels}
        openDirectMessageList={vi.fn()}
        openDirectMessagePane={vi.fn(async () => undefined)}
        openAuthorDetail={vi.fn(async () => undefined)}
        handleDeleteDirectMessageMessage={vi.fn(async () => undefined)}
        handleDirectMessageAttachmentSelection={vi.fn(async () => undefined)}
        handleDirectMessageAttachmentPaste={handleDirectMessageAttachmentPaste}
        handleRemoveDirectMessageDraftAttachment={vi.fn()}
        handleSendDirectMessage={vi.fn(async () => undefined)}
        surfaceKind='conversation'
        peerPubkey={peerPubkey}
      />
    </DesktopShellStoreContext.Provider>
  );

  const textarea = screen.getByPlaceholderText('common:composer.writeMessage');
  expect(screen.getByText('64+')).toBeTruthy();
  const image = new File(['image'], 'clipboard.png', { type: 'image/png' });
  expect(
    fireEvent.paste(textarea, {
      clipboardData: {
        items: [{ kind: 'file', type: image.type, getAsFile: () => image }],
        files: [image],
      },
    })
  ).toBe(false);
  expect(handleDirectMessageAttachmentPaste).toHaveBeenCalledWith([image]);

  expect(
    fireEvent.paste(textarea, {
      clipboardData: {
        items: [{ kind: 'string', type: 'text/plain', getAsFile: () => null }],
        files: [],
      },
    })
  ).toBe(true);
  expect(handleDirectMessageAttachmentPaste).toHaveBeenCalledTimes(1);
});

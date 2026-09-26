import type { FormEvent } from 'react';

import type {
  AttachmentView,
  DirectMessageConversationView,
  DirectMessageMessageView,
} from '@/lib/api';
import type { DraftMediaItem } from '@/shell/slices/shared';

import { updateRecordEntry } from '@/shell/stateUpdates';
import type { DesktopShellState } from '@/shell/store';
import { messageFromError } from '@/shell/presentation';

import type {
  ActionsBaseParams,
  OpenDirectMessagePane,
  Setter,
} from './shared';

type DirectMessagesParams = Pick<ActionsBaseParams, 'api' | 'translate'> & {
  getState: () => DesktopShellState;
  openDirectMessagePane: OpenDirectMessagePane;
  releaseAllDirectMessageDraftPreviews: () => void;
  setDirectMessageTimelineByPeer: Setter<'directMessageTimelineByPeer'>;
  setDirectMessages: Setter<'directMessages'>;
  setDirectMessageComposer: Setter<'directMessageComposer'>;
  setDirectMessageDraftMediaItems: Setter<'directMessageDraftMediaItems'>;
  setDirectMessageAttachmentInputKey: Setter<'directMessageAttachmentInputKey'>;
  setDirectMessageError: Setter<'directMessageError'>;
  setDirectMessageSending: Setter<'directMessageSending'>;
};

function directMessagePreviewFromAttachments(attachments: AttachmentView[]) {
  if (attachments.some((attachment) => attachment.role === 'video_manifest')) {
    return '[Video]';
  }
  return attachments.length > 0 ? '[Image]' : null;
}

export function createDirectMessageActions({
  api,
  translate,
  getState,
  openDirectMessagePane,
  releaseAllDirectMessageDraftPreviews,
  setDirectMessageTimelineByPeer,
  setDirectMessages,
  setDirectMessageComposer,
  setDirectMessageDraftMediaItems,
  setDirectMessageAttachmentInputKey,
  setDirectMessageError,
  setDirectMessageSending,
}: DirectMessagesParams) {
  async function sendDirectMessageDraft(
    peerPubkey: string,
    composerValue: string,
    draftMediaItems: DraftMediaItem[],
    onSuccess: () => void
  ) {
    const currentState = getState();
    const trimmedComposer = composerValue.trim();
    const attachments = draftMediaItems.flatMap((item) => item.attachments);
    if (!trimmedComposer && attachments.length === 0) {
      return false;
    }
    setDirectMessageSending(true);
    try {
      const messageId = await api.sendDirectMessage(
        peerPubkey,
        trimmedComposer || null,
        attachments,
        null
      );
      const existingConversation =
        currentState.directMessages.find((conversation) => conversation.peer_pubkey === peerPubkey) ??
        null;
      const existingStatus =
        existingConversation?.status ?? currentState.directMessageStatusByPeer[peerPubkey] ?? null;
      const knownPeerAuthor =
        currentState.knownAuthorsByPubkey[peerPubkey] ?? currentState.selectedAuthor ?? null;
      const createdAt = Date.now();
      const optimisticAttachments: AttachmentView[] = attachments.map((attachment, index) => ({
        hash: `${messageId}-attachment-${index}`,
        mime: attachment.mime,
        bytes: attachment.byte_size,
        role: attachment.role ?? 'image_original',
        status: 'Available',
      }));
      const optimisticMessage = {
        dm_id:
          existingConversation?.dm_id ??
          existingStatus?.dm_id ??
          [currentState.syncStatus.local_author_pubkey, peerPubkey].sort().join(':'),
        message_id: messageId,
        sender_pubkey: currentState.syncStatus.local_author_pubkey,
        recipient_pubkey: peerPubkey,
        created_at: createdAt,
        text: trimmedComposer,
        reply_to_message_id: null,
        attachments: optimisticAttachments,
        outgoing: true,
        delivered: true,
      } satisfies DirectMessageMessageView;
      const optimisticConversation = {
        dm_id: optimisticMessage.dm_id,
        peer_pubkey: peerPubkey,
        peer_name: knownPeerAuthor?.name ?? existingConversation?.peer_name ?? null,
        peer_display_name:
          knownPeerAuthor?.display_name ?? existingConversation?.peer_display_name ?? null,
        peer_picture_asset:
          knownPeerAuthor?.picture_asset ?? existingConversation?.peer_picture_asset ?? null,
        updated_at: createdAt,
        last_message_at: createdAt,
        last_message_id: messageId,
        last_message_preview:
          trimmedComposer || directMessagePreviewFromAttachments(optimisticAttachments),
        status:
          existingStatus ??
          existingConversation?.status ?? {
            peer_pubkey: peerPubkey,
            dm_id: optimisticMessage.dm_id,
            mutual: true,
            send_enabled: true,
            pending_outbox_count: 0,
            pending_outbox_has_more: false,
          },
      } satisfies DirectMessageConversationView;
      setDirectMessageTimelineByPeer(updateRecordEntry(peerPubkey, (prev) => [
          optimisticMessage,
          ...(prev ?? []).filter((message) => message.message_id !== messageId),
        ]));
      setDirectMessages((current) => {
        const remaining = current.filter((conversation) => conversation.peer_pubkey !== peerPubkey);
        return [optimisticConversation, ...remaining];
      });
      onSuccess();
      setDirectMessageError(null);
    } catch (sendError) {
      setDirectMessageError(
        messageFromError(sendError, translate('common:errors.failedToSendDirectMessage'))
      );
      return false;
    } finally {
      setDirectMessageSending(false);
    }
    try {
      await openDirectMessagePane(peerPubkey, { historyMode: 'replace' });
    } catch (refreshError) {
      setDirectMessageError(
        messageFromError(refreshError, translate('common:errors.failedToOpenDirectMessage'))
      );
    }
    return true;
  }

  async function handleSendDirectMessage(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    const currentState = getState();
    const peerPubkey = currentState.selectedDirectMessagePeerPubkey;
    if (!peerPubkey) {
      return;
    }
    const composerField = event.currentTarget.querySelector('textarea');
    const composerValue = composerField?.value ?? currentState.directMessageComposer;
    await sendDirectMessageDraft(
      peerPubkey,
      composerValue,
      currentState.directMessageDraftMediaItems,
      () => {
        releaseAllDirectMessageDraftPreviews();
        setDirectMessageComposer('');
        setDirectMessageDraftMediaItems([]);
        setDirectMessageAttachmentInputKey((value) => value + 1);
      }
    );
  }

  return {
    handleSendDirectMessage,
    sendDirectMessageDraft,
  };
}

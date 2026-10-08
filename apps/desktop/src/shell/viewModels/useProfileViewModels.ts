import { useMemo } from 'react';

import type { AuthorDetailView } from '@/components/core/types';
import {
  authorDisplayLabel,
  resolveProfilePictureSrc,
  shortPubkey,
  strongestRelationshipLabel,
} from '@/shell/presentation';
import type { DesktopShellState } from '@/shell/store';

type UseProfileViewModelsArgs = {
  authorError: DesktopShellState['authorError'];
  knownAuthorsByPubkey: DesktopShellState['knownAuthorsByPubkey'];
  localAuthorPubkey: string;
  localProfile: DesktopShellState['localProfile'];
  mediaObjectUrls: DesktopShellState['mediaObjectUrls'];
  profileAvatarPreviewUrl: string | null;
  profileDraft: DesktopShellState['profileDraft'];
  selectedAuthor: DesktopShellState['selectedAuthor'];
  t: (key: string, options?: Record<string, unknown>) => string;
};

// profile / author section の projection(Q3 T5 の分割先)。
export function useProfileViewModels({
  authorError,
  knownAuthorsByPubkey,
  localAuthorPubkey,
  localProfile,
  mediaObjectUrls,
  profileAvatarPreviewUrl,
  profileDraft,
  selectedAuthor,
  t,
}: UseProfileViewModelsArgs) {
  const profileEditorFields = useMemo(
    () => ({
      displayName: profileDraft.display_name ?? '',
      name: profileDraft.name ?? '',
      about: profileDraft.about ?? '',
      nip05: profileDraft.nip05 ?? '',
    }),
    [profileDraft]
  );

  const profileEditorPictureSrc =
    profileAvatarPreviewUrl ?? resolveProfilePictureSrc(localProfile, mediaObjectUrls);
  const profileEditorHasPicture = Boolean(
    profileAvatarPreviewUrl ||
      profileDraft.clear_picture ||
      profileDraft.picture_upload ||
      localProfile?.picture_asset
  ) && !profileDraft.clear_picture;

  const resolvedSelectedAuthor = useMemo(
    () =>
      selectedAuthor ? knownAuthorsByPubkey[selectedAuthor.author_pubkey] ?? selectedAuthor : null,
    [knownAuthorsByPubkey, selectedAuthor]
  );
  const authorDetailView = useMemo<AuthorDetailView>(
    () => ({
      author: resolvedSelectedAuthor,
      displayLabel: resolvedSelectedAuthor
        ? authorDisplayLabel(
            resolvedSelectedAuthor.author_pubkey,
            resolvedSelectedAuthor.display_name,
            resolvedSelectedAuthor.name
          )
        : t('common:fallbacks.authorDetail'),
      pictureSrc: resolveProfilePictureSrc(resolvedSelectedAuthor, mediaObjectUrls),
      summary: resolvedSelectedAuthor
        ? {
            label: strongestRelationshipLabel(resolvedSelectedAuthor),
            following: resolvedSelectedAuthor.following,
            followedBy: resolvedSelectedAuthor.followed_by,
            mutual: resolvedSelectedAuthor.mutual,
            friendOfFriend: resolvedSelectedAuthor.friend_of_friend,
            muted: resolvedSelectedAuthor.muted,
            blocking: resolvedSelectedAuthor.blocking,
            blockedBy: resolvedSelectedAuthor.blocked_by,
            viaPubkeys: resolvedSelectedAuthor.friend_of_friend_via_pubkeys.map(shortPubkey),
            isSelf: resolvedSelectedAuthor.author_pubkey === localAuthorPubkey,
            canFollow: resolvedSelectedAuthor.author_pubkey !== localAuthorPubkey,
            followActionLabel: resolvedSelectedAuthor.following ? 'Unfollow' : 'Follow',
            muteActionLabel: resolvedSelectedAuthor.muted ? 'Unmute' : 'Mute',
            blockActionLabel: resolvedSelectedAuthor.blocking ? 'Unblock' : 'Block',
          }
        : null,
      canMessage: Boolean(
        resolvedSelectedAuthor &&
          resolvedSelectedAuthor.author_pubkey !== localAuthorPubkey &&
          resolvedSelectedAuthor.mutual
      ),
      authorError,
    }),
    [authorError, mediaObjectUrls, resolvedSelectedAuthor, localAuthorPubkey, t]
  );

  return {
    profileEditorFields,
    profileEditorPictureSrc,
    profileEditorHasPicture,
    authorDetailView,
  };
}

import { useMemo } from 'react';

import { resolveProfilePictureSrc } from '@/shell/presentation';
import type { DesktopShellState } from '@/shell/store';

type UseProfileViewModelsArgs = {
  localProfile: DesktopShellState['localProfile'];
  mediaObjectUrls: DesktopShellState['mediaObjectUrls'];
  profileAvatarPreviewUrl: string | null;
  profileDraft: DesktopShellState['profileDraft'];
};

// プロフィール編集の表示用データ。
export function useProfileViewModels({
  localProfile,
  mediaObjectUrls,
  profileAvatarPreviewUrl,
  profileDraft,
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

  return {
    profileEditorFields,
    profileEditorPictureSrc,
    profileEditorHasPicture,
  };
}

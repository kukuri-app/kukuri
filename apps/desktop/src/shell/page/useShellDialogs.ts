import { useCallback, useEffect, useState } from 'react';

import type { PrimarySection } from '@/components/shell/types';

type PendingLeaveChannel = {
  channelId: string;
  topicId: string;
};

type UseShellDialogsArgs = {
  activePrimarySection: PrimarySection;
};

export function useShellDialogs({ activePrimarySection }: UseShellDialogsArgs) {
  const [channelDialogOpen, setChannelDialogOpen] = useState(false);
  const [channelSettingsDialogOpen, setChannelSettingsDialogOpen] = useState(false);
  const [leaveChannelDialogOpen, setLeaveChannelDialogOpenState] = useState(false);
  const [pendingLeaveChannel, setPendingLeaveChannel] = useState<PendingLeaveChannel | null>(null);
  const [gameCreateDialogOpen, setGameCreateDialogOpen] = useState(false);
  const [profileAvatarCropFile, setProfileAvatarCropFile] = useState<File | null>(null);
  const [profileAvatarCropOpen, setProfileAvatarCropOpen] = useState(false);
  const [profileAvatarInputKey, setProfileAvatarInputKey] = useState(0);
  useEffect(() => {
    if (activePrimarySection !== 'game' && gameCreateDialogOpen) {
      setGameCreateDialogOpen(false);
    }
  }, [activePrimarySection, gameCreateDialogOpen]);

  const openLeaveChannelDialog = useCallback((topicId: string, channelId: string) => {
    setPendingLeaveChannel({ topicId, channelId });
    setLeaveChannelDialogOpenState(true);
  }, []);

  const setLeaveChannelDialogOpen = useCallback((open: boolean) => {
    setLeaveChannelDialogOpenState(open);
    if (!open) {
      setPendingLeaveChannel(null);
    }
  }, []);

  const confirmLeaveChannel = useCallback(
    async (leaveChannel: (topicId: string, channelId: string) => Promise<void>) => {
      if (!pendingLeaveChannel) {
        return;
      }
      await leaveChannel(pendingLeaveChannel.topicId, pendingLeaveChannel.channelId);
      setLeaveChannelDialogOpenState(false);
      setPendingLeaveChannel(null);
    },
    [pendingLeaveChannel]
  );

  return {
    channelDialogOpen,
    channelSettingsDialogOpen,
    confirmLeaveChannel,
    gameCreateDialogOpen,
    leaveChannelDialogOpen,
    openLeaveChannelDialog,
    profileAvatarCropFile,
    profileAvatarCropOpen,
    profileAvatarInputKey,
    setChannelDialogOpen,
    setChannelSettingsDialogOpen,
    setGameCreateDialogOpen,
    setLeaveChannelDialogOpen,
    setProfileAvatarCropFile,
    setProfileAvatarCropOpen,
    setProfileAvatarInputKey,
  };
}

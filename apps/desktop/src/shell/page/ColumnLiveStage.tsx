import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import type { JoinedPrivateChannelView, PostView, ReactionKeyInput } from '@/lib/api';
import { isPublicChannelId } from '@/lib/publicChannels';
import { topicDisplayName } from '@/lib/topicId';
import { useColumnRuntime } from '@/shell/ColumnRuntimeContext';
import { knownPublicThreadTitle } from '@/shell/slices/publicThreads';
import type { ColumnScope, ColumnState } from '@/shell/slices/workspace';
import { useDesktopShellStore } from '@/shell/store';
import type { useDesktopShellViewModels } from '@/shell/useDesktopShellViewModels';

import { LiveStage } from './LiveStage';

type ColumnLiveStageProps = {
  column: ColumnState & { scope: ColumnScope; entityId: string };
  joinedChannels: JoinedPrivateChannelView[];
  buildPostCardView: ReturnType<typeof useDesktopShellViewModels>['buildPostCardView'];
  onReactionPickerOpen: () => void;
  onToggleReaction: (post: PostView, reactionKey: ReactionKeyInput) => void;
  onReply: (post: PostView) => void;
};

/**
 * #1841 AC-3: ライブの列の左側に、列の Live(entity)を大きく出す。会話は列の scope、Live の場所は列の `live`(無ければ scope)。
 * 重ねる投稿は、通常の投稿の表示と同じ判定(成人向け・信頼値の評価)で隠す。
 */
export function ColumnLiveStage({
  column,
  joinedChannels,
  buildPostCardView,
  onReactionPickerOpen,
  onToggleReaction,
  onReply,
}: ColumnLiveStageProps) {
  const { t } = useTranslation('shell');
  const { visible } = useColumnRuntime();
  const { topicId, channelId } = column.scope;
  const live = column.live ?? column.scope;
  const { recentReactions, owned, bookmarked, threadTitle } = useDesktopShellStore(
    useShallow((state) => ({
      recentReactions: state.recentReactions,
      owned: state.ownedReactionAssets,
      bookmarked: state.bookmarkedReactionAssets,
      threadTitle: isPublicChannelId(channelId)
        ? knownPublicThreadTitle(state.publicThreadsById, state.knownAuthorsByPubkey, channelId)
        : undefined,
    }))
  );
  const reactionAssets = useMemo(
    () => [...new Map([...owned, ...bookmarked].map((asset) => [asset.asset_id, asset])).values()],
    [owned, bookmarked]
  );
  const conversationLabel = !channelId
    ? topicDisplayName(topicId)
    : isPublicChannelId(channelId)
      ? (threadTitle ?? t('publicThreads.title'))
      : (joinedChannels.find((channel) => channel.channel_id === channelId)?.label ?? channelId);
  return (
    <LiveStage
      source={{
        live: { session_id: column.entityId, topic_id: live.topicId, channel_id: live.channelId },
        conversation: { topic: topicId, channelId },
      }}
      conversationLabel={conversationLabel}
      visible={visible}
      mediaForPost={(post) => buildPostCardView(post, 'timeline', joinedChannels).media}
      hidden={(post) => {
        const view = buildPostCardView(post, 'timeline', joinedChannels);
        return Boolean(view.adultContentGated || view.trustGate);
      }}
      recentReactions={recentReactions}
      reactionAssets={reactionAssets}
      onReactionPickerOpen={onReactionPickerOpen}
      onToggleReaction={onToggleReaction}
      onReply={onReply}
    />
  );
}

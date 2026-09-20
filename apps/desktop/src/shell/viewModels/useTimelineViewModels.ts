import { resolvePostTrustGate } from '@/shell/authorTrustGates';
import { type SyntheticEvent, useCallback, useMemo } from 'react';

import type {
  ContentAdvisoryView,
  MentionAuthorView,
  PostCardView,
  ReferencedAuthorMeta,
} from '@/components/core/types';
import type { PostView, ProfileAssetView } from '@/lib/api';
import { contentProvenanceFromView } from '@/lib/api/provenance';
import type { SupportedLocale } from '@/i18n';
import { extractMentions } from '@/lib/internalLinks';
import {
  isAdultLabeledPost,
  logMediaDebug,
  mediaElementDebugFields,
  selectVideoManifest,
  selectVideoPoster,
} from '@/shell/media';
import {
  authorDisplayLabel,
  canCreateRepostFromPost,
  isQuoteRepost,
  localizeAudienceLabel,
  publishedTopicIdForPost,
  resolveProfilePictureSrc,
  strongestRelationshipLabel,
} from '@/shell/presentation';
import {
  useDesktopShellFieldSetter,
  useDesktopShellStore,
  type DesktopShellState,
} from '@/shell/store';
import { buildPostMediaView } from '@/shell/viewModels/postMediaView';
import {
  resolvePostAdvisory,
  type TimelineAdvisoryLookupState,
  type TimelineContentAdvisoryIndex,
} from '@/shell/contentAdvisories';

type UseTimelineViewModelsArgs = {
  activeJoinedChannels: DesktopShellState['joinedChannelsByTopic'][string];
  activeTimeline: PostView[];
  adultContentEnabled: boolean;
  bookmarkedPosts: DesktopShellState['bookmarkedPosts'];
  developerModeEnabled: boolean;
  knownAuthorsByPubkey: DesktopShellState['knownAuthorsByPubkey'];
  localAuthorPubkey: string;
  locale: SupportedLocale;
  localProfile: DesktopShellState['localProfile'];
  mediaObjectUrls: DesktopShellState['mediaObjectUrls'];
  profileTimeline: PostView[];
  selectedAuthorTimeline: PostView[];
  thread: PostView[];
  unsupportedVideoManifests: DesktopShellState['unsupportedVideoManifests'];
  /// #1056: 採用 node への一括照会の結果と照会中の subject。
  timelineContentAdvisories?: TimelineContentAdvisoryIndex;
  timelineAdvisoryLookup?: TimelineAdvisoryLookupState;
  /// #1056: advisory の発行元の表示名を manifest から引く。
  communityNodeManifests?: DesktopShellState['communityNodeManifests'];
  /// #1107: 表示設定 OFF の間ゲートする添付 blob hash(`useAdultGatedMediaHashes`)。
  gatedMediaHashes?: readonly string[];
  /// #1061: 採用 CN の信頼値による著者の表示判断（著者 pubkey → 判断）。
  authorTrustGates?: DesktopShellState['authorTrustGates'];
};

const EMPTY_TRUST_GATES: DesktopShellState['authorTrustGates'] = {};
const EMPTY_ADVISORIES: TimelineContentAdvisoryIndex = {};
const INACTIVE_LOOKUP: TimelineAdvisoryLookupState = { active: false, settled: {} };
const EMPTY_MANIFESTS: DesktopShellState['communityNodeManifests'] = {};
const EMPTY_HASHES: readonly string[] = [];

export function useTimelineViewModels({
  activeJoinedChannels,
  activeTimeline,
  adultContentEnabled,
  bookmarkedPosts,
  developerModeEnabled,
  knownAuthorsByPubkey,
  localAuthorPubkey,
  locale,
  localProfile,
  mediaObjectUrls,
  profileTimeline,
  selectedAuthorTimeline,
  thread,
  unsupportedVideoManifests,
  timelineContentAdvisories = EMPTY_ADVISORIES,
  timelineAdvisoryLookup = INACTIVE_LOOKUP,
  communityNodeManifests = EMPTY_MANIFESTS,
  gatedMediaHashes = EMPTY_HASHES,
  authorTrustGates = EMPTY_TRUST_GATES,
}: UseTimelineViewModelsArgs) {
  const gatedMediaHashSet = useMemo(() => new Set(gatedMediaHashes), [gatedMediaHashes]);
  const mediaRetryingHashes = useDesktopShellStore((state) => state.mediaRetryingHashes);
  const setUnsupportedVideoManifests = useDesktopShellFieldSetter(
    'unsupportedVideoManifests'
  );
  const buildPostCardView = useCallback(
    (
      post: PostView,
      context: 'timeline' | 'thread',
      joinedChannels = activeJoinedChannels
    ): PostCardView => {
      // #858: 表示許可前は成人向けラベル付き投稿の本文・メディアを代替表示にする。
      // #1056: 第 2 のラベル源として、採用 node が発行した content advisory も同じゲートへ合成する
      // (ADR 0046 §6.1)。advisory は `content_labels` へ書き戻さず、説明用の別欄で運ぶ。
      const selfLabeled = isAdultLabeledPost(post);
      const advisoryState = resolvePostAdvisory(
        post,
        timelineContentAdvisories,
        timelineAdvisoryLookup
      );
      const adultContentGated =
        !adultContentEnabled && (selfLabeled || advisoryState.advisory !== null);
      const gatedBy = adultContentGated ? (selfLabeled ? 'self_label' : 'advisory') : undefined;
      const advisoryEntry = advisoryState.advisory;
      const manifestEntry = advisoryEntry
        ? communityNodeManifests[advisoryEntry.nodeBaseUrl]
        : undefined;
      const contentAdvisory: ContentAdvisoryView | null =
        adultContentGated && advisoryEntry
          ? {
              issuerNodeId: advisoryEntry.advisory.issuer_node_id,
              nodeBaseUrl: advisoryEntry.nodeBaseUrl,
              nodeName:
                manifestEntry?.status === 'ok'
                  ? manifestEntry.manifest.node_name.trim() || null
                  : null,
              category: advisoryEntry.advisory.category,
              label: advisoryEntry.advisory.label,
              confidence: advisoryEntry.advisory.confidence ?? null,
              basis: advisoryEntry.advisory.basis,
              signalId: advisoryEntry.advisory.signal_id,
              subjectKind: advisoryEntry.advisory.subject_kind,
              subjectId: advisoryEntry.advisory.subject_id,
            }
          : null;
      const videoPoster = selectVideoPoster(post);
      const videoManifest = selectVideoManifest(post);
      // #1052: メディア表示データは「見つける」の解決済み投稿と同じ builder を使う。
      // #1056: 照会が未決の間はメディアを取得せずスケルトンにする(確定後の代替表示とは分ける)。
      // #1107: 投稿自体は対象外でも、同じ blob が別の投稿でゲートされていればメディアだけを伏せる
      // (本文は bytes 取得を伴わないため表示する)。照会中のスケルトンより確定した代替表示を優先する。
      const sharedMediaGated =
        !adultContentGated &&
        post.attachments.some((attachment) => gatedMediaHashSet.has(attachment.hash));
      const media = buildPostMediaView(post, {
        adultContentGated: adultContentGated || sharedMediaGated,
        gatedBy: sharedMediaGated ? 'shared_media' : gatedBy,
        advisoryPending: advisoryState.pending,
        locale,
        mediaObjectUrls,
        mediaRetryingHashes,
        unsupportedVideoManifests,
      });
      const logPlaybackEvent =
        (eventName: string) => (event: SyntheticEvent<HTMLVideoElement>) => {
          const video = event.currentTarget;
          logMediaDebug(eventName === 'error' ? 'warn' : 'info', `playback ${eventName}`, {
            manifest_hash: videoManifest?.hash ?? null,
            mime: videoManifest?.mime ?? null,
            post_id: post.object_id,
            poster_hash: videoPoster?.hash ?? null,
            playback_src: media.videoPlaybackSrc ?? null,
            ...mediaElementDebugFields(video),
            video_height: video.videoHeight || null,
            video_width: video.videoWidth || null,
          });
          if (eventName === 'error' && videoManifest) {
            setUnsupportedVideoManifests((current) =>
              current[videoManifest.hash]
                ? current
                : { ...current, [videoManifest.hash]: true }
            );
          }
        };
      const publishedTopicId = publishedTopicIdForPost(post);
      const threadTargetId =
        post.object_kind === 'repost' && !isQuoteRepost(post) && post.repost_of
          ? post.repost_of.root_id ?? post.repost_of.source_object_id
          : post.root_id ?? post.object_id;
      const threadTopicId =
        post.object_kind === 'repost' && !isQuoteRepost(post) && post.repost_of
          ? post.repost_of.source_topic_id
          : publishedTopicId;
      const knownAuthor =
        post.author_pubkey === localAuthorPubkey
          ? localProfile
          : knownAuthorsByPubkey[post.author_pubkey] ?? null;
      const authorPicture = resolveProfilePictureSrc(
        {
          picture_asset: post.author_picture_asset ?? knownAuthor?.picture_asset ?? null,
        },
        mediaObjectUrls
      );
      const resolveRefAuthor = (
        pubkey: string,
        label: string,
        pictureAsset?: ProfileAssetView | null
      ): ReferencedAuthorMeta => {
        const known =
          pubkey === localAuthorPubkey ? localProfile : knownAuthorsByPubkey[pubkey] ?? null;
        return {
          pubkey,
          label,
          picture: resolveProfilePictureSrc(
            {
              picture_asset: pictureAsset ?? known?.picture_asset ?? null,
            },
            mediaObjectUrls
          ),
        };
      };
      const repostSource = post.repost_of ?? null;
      const repostSourceAuthor = repostSource
        ? resolveRefAuthor(
            repostSource.source_author_pubkey,
            authorDisplayLabel(
              repostSource.source_author_pubkey,
              repostSource.source_author_display_name,
              repostSource.source_author_name
            ),
            repostSource.source_author_picture_asset
          )
        : null;
      const replyPreview = post.reply_preview ?? null;
      const replyParentAuthor = replyPreview
        ? resolveRefAuthor(
            replyPreview.author.pubkey,
            authorDisplayLabel(
              replyPreview.author.pubkey,
              replyPreview.author.display_name,
              replyPreview.author.name
            ),
            replyPreview.author.picture_asset
          )
        : null;
      const mentionAuthors: Record<string, MentionAuthorView> = {};
      for (const source of [post.content, repostSource?.content, replyPreview?.content]) {
        if (!source) continue;
        for (const { pubkey, label } of extractMentions(source)) {
          if (mentionAuthors[pubkey]) continue;
          const known =
            pubkey === localAuthorPubkey ? localProfile : knownAuthorsByPubkey[pubkey] ?? null;
          if (!known) continue;
          mentionAuthors[pubkey] = {
            pubkey,
            label:
              known.display_name?.trim() ||
              known.name?.trim() ||
              label ||
              authorDisplayLabel(pubkey),
            displayName: known.display_name ?? null,
            name: known.name ?? null,
            aboutPreview: known.about?.slice(0, 50) ?? null,
            picture: resolveProfilePictureSrc(known, mediaObjectUrls),
          };
        }
      }
      // #1061: 著者または引用元の著者が非表示推奨なら、投稿を折りたたむ（ADR 0026 §8.4）。
      const trustGate = resolvePostTrustGate(post, authorTrustGates);
      return {
        post,
        trustGate,
        provenance: contentProvenanceFromView(post.provenance),
        context,
        authorLabel: authorDisplayLabel(
          post.author_pubkey,
          post.author_display_name,
          post.author_name
        ),
        authorPicture,
        relationshipLabel: strongestRelationshipLabel(post),
        audienceChipLabel: post.channel_id
          ? joinedChannels.find((channel) => channel.channel_id === post.channel_id)?.label ??
            localizeAudienceLabel(post.audience_label)
          : localizeAudienceLabel(post.audience_label),
        threadTargetId,
        threadTopicId,
        canReply: post.is_threadable ?? (post.object_kind !== 'repost' || isQuoteRepost(post)),
        canRepost: canCreateRepostFromPost(post),
        repostSourceAuthor,
        replyParentAuthor,
        mentionAuthors,
        suppressReplyPreview: context === 'thread',
        showUnavailableDiagnostics: developerModeEnabled,
        adultContentGated,
        gatedBy,
        contentAdvisory,
        media: {
          ...media,
          // 再生診断ログはイベントを購読するタイムライン側の責務として builder の外に置く。
          videoProps:
            media.kind === 'video' && media.videoPlaybackSrc && !media.videoUnsupportedOnClient
              ? {
                  onCanPlay: logPlaybackEvent('canplay'),
                  onDurationChange: logPlaybackEvent('durationchange'),
                  onError: logPlaybackEvent('error'),
                  onLoadedData: logPlaybackEvent('loadeddata'),
                  onLoadedMetadata: logPlaybackEvent('loadedmetadata'),
                  onLoadStart: logPlaybackEvent('loadstart'),
                  onPlaying: logPlaybackEvent('playing'),
                }
              : undefined,
        },
      };
    },
    [
      activeJoinedChannels,
      adultContentEnabled,
      authorTrustGates,
      communityNodeManifests,
      developerModeEnabled,
      gatedMediaHashSet,
      knownAuthorsByPubkey,
      localAuthorPubkey,
      localProfile,
      locale,
      mediaObjectUrls,
      mediaRetryingHashes,
      setUnsupportedVideoManifests,
      timelineAdvisoryLookup,
      timelineContentAdvisories,
      unsupportedVideoManifests,
    ]
  );

  return {
    buildPostCardView,
    activeTimelinePostViews: useMemo(
      () => activeTimeline.map((post) => buildPostCardView(post, 'timeline')),
      [activeTimeline, buildPostCardView]
    ),
    bookmarkedTimelinePostViews: useMemo(
      () => bookmarkedPosts.map((item) => buildPostCardView(item.post, 'timeline')),
      [bookmarkedPosts, buildPostCardView]
    ),
    profileTimelinePostViews: useMemo(
      () => profileTimeline.map((post) => buildPostCardView(post, 'timeline')),
      [buildPostCardView, profileTimeline]
    ),
    selectedAuthorTimelinePostViews: useMemo(
      () => selectedAuthorTimeline.map((post) => buildPostCardView(post, 'timeline')),
      [buildPostCardView, selectedAuthorTimeline]
    ),
    threadPostViews: useMemo(
      () => thread.map((post) => buildPostCardView(post, 'thread')),
      [buildPostCardView, thread]
    ),
  };
}

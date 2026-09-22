import i18n from '@/i18n';
import type {
  AuthorSocialView,
  CommunityIndexResolvedPostView,
  IndexEntryView,
} from '@/lib/api';
import { isAdultLabeledPost, primaryContentAdvisory } from '@/shell/media';
import { authorDisplayLabel, resolveProfilePictureSrc } from '@/shell/presentation';
import { buildPostMediaView } from '@/shell/viewModels/postMediaView';

import type { ContentAdvisoryView, PostCardView } from './types';

export type CommunityIndexOperation = 'search' | 'discovery' | 'recommendations';

type CommunityIndexPostCardViewOptions = {
  nodeBaseUrl: string;
  operation: CommunityIndexOperation;
  topicId: string | null;
  knownAuthor: AuthorSocialView | null;
  authorStatus?: 'loading' | 'resolved' | 'failed';
  resolutionStatus?: 'loading' | 'resolved' | 'failed';
  resolvedEntry?: CommunityIndexResolvedPostView | null;
  mediaObjectUrls: Record<string, string | null>;
  adultContentEnabled?: boolean;
  /// #1107: 表示設定 OFF の間ゲートする添付 blob hash(全表示経路で共通)。
  gatedMediaHashes?: readonly string[];
  /// #1052: 解決済み投稿の添付メディアをタイムラインと同じ規則で組み立てるために使う。
  unsupportedVideoManifests?: Record<string, true>;
  locale?: string | null;
  /// #1055: index を返した node の manifest 表示名。取得できていなければ null を渡す。
  nodeName?: string | null;
};

function audienceLabel(entry: IndexEntryView): string {
  return entry.scope_kind === 'private_channel'
    ? i18n.t('common:audience.privateChannel')
    : i18n.t('common:audience.public');
}

export function communityIndexPostCardView(
  entry: IndexEntryView,
  options: CommunityIndexPostCardViewOptions
): PostCardView {
  const recommendation = options.operation === 'recommendations';
  const capability = recommendation ? 'recommendation' : 'community_index';
  const knownAuthor = options.knownAuthor;
  const resolvedPost = options.resolvedEntry?.post ?? null;
  const resolutionStatus = resolvedPost
    ? 'resolved'
    : options.resolutionStatus ?? 'loading';
  const capabilities = options.resolvedEntry?.capabilities ?? {
    open_thread: false,
    reply: false,
    repost: false,
    quote_repost: false,
    react: false,
    copy_link: false,
    bookmark: false,
    withdraw: false,
  };
  const topicId = resolvedPost
    ? resolvedPost.published_topic_id?.trim() || resolvedPost.origin_topic_id?.trim() || null
    : entry.scope_kind === 'public_topic'
      ? entry.scope_id
      : options.topicId?.trim() || null;
  const resolvedAuthorDisplayName = resolvedPost?.author_display_name ?? null;
  const resolvedAuthorName = resolvedPost?.author_name ?? null;
  const hasResolvedAuthorName = Boolean(
    resolvedAuthorDisplayName?.trim() || resolvedAuthorName?.trim()
  );
  const authorLabel = knownAuthor || hasResolvedAuthorName
    ? authorDisplayLabel(
        entry.author_pubkey,
        knownAuthor?.display_name ?? resolvedAuthorDisplayName,
        knownAuthor?.name ?? resolvedAuthorName
      )
    : options.authorStatus === 'failed'
      ? i18n.t('common:fallbacks.authorUnavailable')
      : options.authorStatus === 'loading'
        ? i18n.t('common:fallbacks.authorLoading')
        : i18n.t('common:fallbacks.unknownAuthor');
  const audience = audienceLabel(entry);
  const displayPost = resolvedPost
    ? {
        ...resolvedPost,
        author_pubkey: entry.author_pubkey,
        author_name: knownAuthor?.name ?? resolvedPost.author_name ?? null,
        author_display_name:
          knownAuthor?.display_name ?? resolvedPost.author_display_name ?? null,
        author_picture_asset:
          knownAuthor?.picture_asset ?? resolvedPost.author_picture_asset ?? null,
        // Node index text is never a canonical content source. Render only the
        // locally resolved, signed post after its labels are available (#858).
        content: resolvedPost.content,
        content_status: resolvedPost.content_status,
        // #1052: 添付はローカル解決済みの署名付き envelope 由来であり、node の index
        // メタデータからの推測ではない。タイムラインと同じ表示経路へそのまま渡す。
        attachments: resolvedPost.attachments,
        created_at: resolvedPost.created_at,
      }
    : {
        object_id: entry.object_id,
        // IndexEntryView は envelope id を提供しない。identifierCopy でも明示的に除外する。
        envelope_id: '',
        author_pubkey: entry.author_pubkey,
        author_name: knownAuthor?.name ?? null,
        author_display_name: knownAuthor?.display_name ?? null,
        author_picture_asset: knownAuthor?.picture_asset ?? null,
        following: knownAuthor?.following ?? false,
        followed_by: knownAuthor?.followed_by ?? false,
        mutual: knownAuthor?.mutual ?? false,
        friend_of_friend: knownAuthor?.friend_of_friend ?? false,
        provenance: null,
        withdrawal: null,
        content:
          resolutionStatus === 'loading'
            ? i18n.t('shell:communityIndex.contentResolving')
            : i18n.t('shell:communityIndex.contentUnavailable'),
        content_status: 'Available' as const,
        attachments: [],
        created_at: entry.created_at,
        reply_to: null,
        reply_preview: null,
        root_id: null,
        object_kind: 'post',
        published_topic_id: null,
        origin_topic_id: null,
        repost_of: null,
        repost_commentary: null,
        is_threadable: false,
        channel_id: null,
        audience_label: audience,
        reaction_summary: [],
        my_reactions: [],
      };

  // #858: canonical post の top-level / quote / reply-preview labels を検索でも共有する。
  // #1055: 第 2 のラベル源として、index を返した設定済み node が発行した content advisory も
  // 同じゲートへ合成する(ADR 0046 §6.1)。desktop-runtime が issuer 照合済みの advisory だけを
  // 渡すため、ここでは採用可否を再判定しない。advisory は canonical 解決の前後で変わらないので、
  // 解決済みになっても代替表示を維持する。
  const advisory = primaryContentAdvisory(entry.content_advisories);
  const selfLabeled = resolvedPost !== null && isAdultLabeledPost(resolvedPost);
  const adultContentGated = !options.adultContentEnabled && (selfLabeled || advisory !== null);
  // advisory は投稿の canonical でも署名対象でもないため `content_labels` へ書き戻さない。
  // 表示用の説明だけを別欄で運ぶ(`content_advisories_are_separate_from_signed_content_labels`)。
  // 両方あるときは投稿者自身の申告を根拠として示す(利用者にとって強い根拠であり、
  // node への申し立て対象でもない)。advisory の説明は別途添える。
  const gatedBy = adultContentGated ? (selfLabeled ? 'self_label' : 'advisory') : undefined;
  const contentAdvisory: ContentAdvisoryView | null =
    adultContentGated && advisory
      ? {
          issuerNodeId: advisory.issuer_node_id,
          nodeBaseUrl: options.nodeBaseUrl,
          nodeName: options.nodeName ?? null,
          category: advisory.category,
          label: advisory.label,
          confidence: advisory.confidence ?? null,
          basis: advisory.basis,
          signalId: advisory.signal_id,
          subjectKind: advisory.subject_kind,
          subjectId: advisory.subject_id,
        }
      : null;

  // #1107: 投稿自体は対象外でも、同じ blob が別の投稿でゲートされていればメディアだけを伏せる。
  const gatedMediaHashes = options.gatedMediaHashes ?? [];
  const sharedMediaGated =
    !adultContentGated &&
    displayPost.attachments.some((attachment) => gatedMediaHashes.includes(attachment.hash));

  return {
    post: displayPost,
    actionPost: resolvedPost,
    context: 'timeline',
    adultContentGated,
    gatedBy,
    // #1055: canonical 解決前は隠すべき本文がまだ無い。代替表示にしつつ待機文言を保つ。
    gatedBodyText: resolvedPost === null ? displayPost.content : null,
    contentAdvisory,
    authorLabel,
    authorPicture: knownAuthor
      ? resolveProfilePictureSrc(knownAuthor, options.mediaObjectUrls)
      : resolvedPost?.author_picture_asset?.hash &&
          typeof options.mediaObjectUrls[resolvedPost.author_picture_asset.hash] === 'string'
        ? options.mediaObjectUrls[resolvedPost.author_picture_asset.hash]
        : null,
    relationshipLabel: null,
    audienceChipLabel: audience,
    threadTargetId: resolvedPost?.root_id ?? entry.object_id,
    threadTopicId: capabilities.open_thread ? topicId : null,
    canOpenThread: capabilities.open_thread,
    canReply: capabilities.reply,
    canRepost: capabilities.repost || capabilities.quote_repost,
    canReact: capabilities.react,
    // #1052: 未解決 entry は attachments が空のままなので、同じ builder でもメディアは
    // 描画されない(推測補完しない)。解決済みだけがタイムラインと同じ表示になる。
    media: buildPostMediaView(displayPost, {
      adultContentGated: adultContentGated || sharedMediaGated,
      gatedBy: sharedMediaGated ? 'shared_media' : gatedBy,
      locale: options.locale ?? i18n.resolvedLanguage ?? null,
      mediaObjectUrls: options.mediaObjectUrls,
      unsupportedVideoManifests: options.unsupportedVideoManifests ?? {},
    }),
    provenance: {
      canonicalSource: 'unknown',
      observedVia: [{ nodeBaseUrl: options.nodeBaseUrl, capability }],
      responsibleReportTargets: [],
    },
    identifierCopy: {
      postId: entry.object_id,
      authorId: entry.author_pubkey,
    },
    reportSubjectKind: recommendation ? 'recommendation' : 'search_result',
    allowReadOnlyReport: true,
  };
}

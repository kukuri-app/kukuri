import { useMemo } from 'react';

import type {
  AttachmentView,
  BookmarkedCustomReactionView,
  DirectMessageMessageView,
  NotificationView,
  PostView,
  Profile,
  RecentReactionView,
} from '@/lib/api';

import {
  isAdultLabeledPost,
  logMediaDebug,
  selectPrimaryImage,
  selectPrimaryImageAttachment,
  selectVideoManifest,
  selectVideoManifestAttachment,
  selectVideoPoster,
  selectVideoPosterAttachment,
} from '@/shell/media';
import type { DesktopShellState } from '@/shell/store';
import {
  resolvePostAdvisory,
  type TimelineAdvisoryLookupState,
  type TimelineContentAdvisoryIndex,
} from '@/shell/contentAdvisories';

type UsePreviewableMediaAttachmentsArgs = {
  activeTimeline: PostView[];
  activePublicTimeline: PostView[];
  /// #1052: 「見つける」Column の解決済み投稿。タイムラインと同じ規則でメディアを
  /// プリフェッチする(未解決 entry は attachments を持たないため対象にならない)。
  communityIndexResolvedPosts: PostView[];
  /// #1107: `activeTimeline` 等に含まれない描画中のタイムライン系投稿(ブックマーク・非 active な
  /// Column)。取得の規則は他のタイムライン系投稿と同じ。
  additionalTimelinePosts?: PostView[];
  /// #1055 / #1107: 表示設定 OFF の間ゲートする添付 blob hash(`useAdultGatedMediaHashes`)。
  /// どの表示経路・どの投稿から現れてもプリフェッチしない(ADR 0046 §6.2)。同じ blob を
  /// advisory の無い投稿が参照していても、投稿単位ではなく hash 単位で止める。
  gatedMediaHashes: string[];
  demandedMediaHashes: ReadonlySet<string>;
  /// #1056: タイムライン系の一括照会結果と照会中の subject。照会中の投稿の添付は表示設定に
  /// かかわらず取得しない(確定前に取得すると、ゲートや ephemeral 取得の判定より先に bytes が届く)。
  /// advisory が確定した投稿の添付は、表示設定 OFF の間は取得しない。
  timelineContentAdvisories?: TimelineContentAdvisoryIndex;
  timelineAdvisoryLookup?: TimelineAdvisoryLookupState;
  profileTimeline: PostView[];
  selectedAuthorTimeline: PostView[];
  thread: PostView[];
  selectedDirectMessageTimeline: DirectMessageMessageView[];
  directMessages: DesktopShellState['directMessages'];
  ownedReactionAssets: DesktopShellState['ownedReactionAssets'];
  bookmarkedReactionAssets: BookmarkedCustomReactionView[];
  recentReactions: RecentReactionView[];
  localProfile: Profile | null;
  knownAuthorsByPubkey: DesktopShellState['knownAuthorsByPubkey'];
  notifications: NotificationView[];
  adultContentEnabled: boolean;
};

const EMPTY_ADVISORIES: TimelineContentAdvisoryIndex = {};
const INACTIVE_LOOKUP: TimelineAdvisoryLookupState = { active: false, settled: {} };
const EMPTY_POSTS: PostView[] = [];
export type PreviewableMediaAttachment = AttachmentView & { source_object_id?: string };

export function usePreviewableMediaAttachments({
  activeTimeline,
  activePublicTimeline,
  additionalTimelinePosts = EMPTY_POSTS,
  communityIndexResolvedPosts,
  gatedMediaHashes,
  demandedMediaHashes,
  timelineContentAdvisories = EMPTY_ADVISORIES,
  timelineAdvisoryLookup = INACTIVE_LOOKUP,
  profileTimeline,
  selectedAuthorTimeline,
  thread,
  selectedDirectMessageTimeline,
  directMessages,
  ownedReactionAssets,
  bookmarkedReactionAssets,
  recentReactions,
  localProfile,
  knownAuthorsByPubkey,
  notifications,
  adultContentEnabled,
}: UsePreviewableMediaAttachmentsArgs): PreviewableMediaAttachment[] {
  return useMemo(() => {
    const attachments = new Map<string, PreviewableMediaAttachment>();
    const gatedHashes = new Set(gatedMediaHashes);

    const tryAddAttachment = (attachment: AttachmentView | null, sourceObjectId?: string, demandOnly = false) => {
      if (!attachment) {
        return;
      }
      const hash = attachment.hash.trim();
      const mime = attachment.mime.trim();
      // #1055 / #1107: ゲート中の hash は、どの投稿から現れても取得対象に入れない。
      if (gatedHashes.has(hash)) {
        return;
      }
      if (!hash || !mime) {
        logMediaDebug('warn', 'remote media metadata skipped', {
          hash: attachment.hash || null,
          mime: attachment.mime || null,
          role: attachment.role,
          status: attachment.status,
        });
        return;
      }
      if (demandOnly && !demandedMediaHashes.has(hash)) {
        return;
      }
      if (attachments.get(hash)?.source_object_id && !sourceObjectId) return;
      attachments.set(hash, {
        ...attachment,
        hash,
        mime,
        ...(sourceObjectId ? { source_object_id: sourceObjectId } : {}),
      });
    };

    // #1056: 「見つける」の解決済み投稿は index 応答の advisory(#1055)で扱い、タイムライン向け
    // 照会の対象にしない。照会中判定をかけると照会済みにならず取得が止まり続けるため分ける。
    const communityIndexPosts = new Set(communityIndexResolvedPosts);
    for (const post of [
      ...activeTimeline,
      ...activePublicTimeline,
      ...profileTimeline,
      ...selectedAuthorTimeline,
      ...thread,
      ...additionalTimelinePosts,
      ...communityIndexResolvedPosts,
    ]) {
      if (post.author_picture_asset) {
        tryAddAttachment({
          hash: post.author_picture_asset.hash,
          mime: post.author_picture_asset.mime,
          bytes: post.author_picture_asset.bytes,
          role: post.author_picture_asset.role,
          status: 'Available',
        });
      }
      // #858: 表示許可前は成人向けラベル付き投稿の添付をプリフェッチ対象に入れない
      // (author avatar とリアクションはラベル対象外)。
      const advisoryState = communityIndexPosts.has(post)
        ? { advisory: null, pending: false }
        : resolvePostAdvisory(post, timelineContentAdvisories, timelineAdvisoryLookup);
      const advisoryBlocked =
        advisoryState.pending || (advisoryState.advisory !== null && !adultContentEnabled);
      if (!advisoryBlocked && (adultContentEnabled || !isAdultLabeledPost(post))) {
        for (const attachment of [
          selectPrimaryImage(post),
          selectVideoPoster(post),
          selectVideoManifest(post),
        ]) {
          tryAddAttachment(attachment, post.object_id, true);
        }
      }
      for (const reaction of post.reaction_summary ?? []) {
        if (!reaction.custom_asset) {
          continue;
        }
        tryAddAttachment({
          hash: reaction.custom_asset.blob_hash,
          mime: reaction.custom_asset.mime,
          bytes: reaction.custom_asset.bytes,
          role: 'image_original',
          status: 'Available',
        });
      }
    }

    for (const message of selectedDirectMessageTimeline) {
      for (const attachment of [
        selectPrimaryImageAttachment(message.attachments),
        selectVideoPosterAttachment(message.attachments),
        selectVideoManifestAttachment(message.attachments),
      ]) {
        tryAddAttachment(attachment, undefined, true);
      }
    }

    for (const asset of [...ownedReactionAssets, ...bookmarkedReactionAssets]) {
      tryAddAttachment({
        hash: asset.blob_hash,
        mime: asset.mime,
        bytes: asset.bytes,
        role: 'image_original',
        status: 'Available',
      });
    }

    for (const reaction of recentReactions) {
      if (!reaction.custom_asset) {
        continue;
      }
      tryAddAttachment({
        hash: reaction.custom_asset.blob_hash,
        mime: reaction.custom_asset.mime,
        bytes: reaction.custom_asset.bytes,
        role: 'image_original',
        status: 'Available',
      });
    }

    for (const pictureAsset of [
      localProfile?.picture_asset ?? null,
      ...Object.values(knownAuthorsByPubkey).map((author) => author.picture_asset ?? null),
      ...directMessages.map((conversation) => conversation.peer_picture_asset ?? null),
      ...notifications.map((notification) => notification.actor_picture_asset ?? null),
    ]) {
      tryAddAttachment(
        pictureAsset
          ? {
              hash: pictureAsset.hash,
              mime: pictureAsset.mime,
              bytes: pictureAsset.bytes,
              role: pictureAsset.role,
              status: 'Available',
            }
          : null
      );
    }

    return [...attachments.values()];
  }, [
    activePublicTimeline,
    activeTimeline,
    additionalTimelinePosts,
    adultContentEnabled,
    demandedMediaHashes,
    gatedMediaHashes,
    bookmarkedReactionAssets,
    timelineAdvisoryLookup,
    timelineContentAdvisories,
    communityIndexResolvedPosts,
    directMessages,
    knownAuthorsByPubkey,
    localProfile?.picture_asset,
    notifications,
    ownedReactionAssets,
    profileTimeline,
    recentReactions,
    selectedDirectMessageTimeline,
    selectedAuthorTimeline,
    thread,
  ]);
}

import { selectSelectedAuthorPosts } from '@/shell/storeSelectors';
import type {
  AttachmentView,
  DesktopApi,
  LocalDraftMediaItem,
  LocalPostDraft,
  PostLiveView,
  PostView,
} from '@/lib/api';
import {
  activeTimelineStorageKey,
  PUBLIC_CHANNEL_REF,
  timelineStorageKeyForChannel,
  type DesktopShellState,
  type DesktopShellStoreApi,
  type DraftMediaItem,
} from '@/shell/store';
import { channelComposeTarget, channelIdOfRef, publishedTopicIdForPost } from '@/shell/presentation';
import {
  activeWorkspaceScope,
} from '@/shell/slices/workspace';
import { updateRecordEntry } from '@/shell/stateUpdates';

import type { Setter, Translate } from './shared';

type OptimisticPostActionsParams = {
  api: DesktopApi;
  activeTopic: string;
  localAuthorPubkey: string;
  localProfile: DesktopShellState['localProfile'];
  refreshVisibleTimelineAfterPublish: (
    topic: string,
    currentThread: string | null,
    scopeChannelId?: string | null
  ) => Promise<void>;
  releaseDraftPreview: (itemId: string) => void;
  storeApi: DesktopShellStoreApi;
  translate: Translate;
  setProfileTimeline: Setter<'profileTimeline'>;
  setThreadsById: Setter<'threadsById'>;
  setTimelinesByKey: Setter<'timelinesByKey'>;
};

export function createOptimisticPostActions({
  api,
  activeTopic,
  localAuthorPubkey,
  localProfile,
  refreshVisibleTimelineAfterPublish,
  releaseDraftPreview,
  storeApi,
  translate,
  setProfileTimeline,
  setThreadsById,
  setTimelinesByKey,
}: OptimisticPostActionsParams) {
  function cloneDraftMediaItems(items: DraftMediaItem[]): LocalDraftMediaItem[] {
    return items.map((item) => ({
      id: item.id,
      source_name: item.source_name,
      preview_url: item.preview_url,
      attachments: item.attachments.map((attachment) => ({ ...attachment })),
    }));
  }

  function attachmentViewsFromDraftMediaItems(
    localId: string,
    items: LocalDraftMediaItem[]
  ): AttachmentView[] {
    let attachmentIndex = 0;
    return items.flatMap((item) =>
      item.attachments.flatMap((attachment) => attachment.reused_attachments ?? [{
        hash: `${localId}-attachment-${attachmentIndex++}`,
        mime: attachment.mime,
        bytes: attachment.byte_size,
        role: attachment.role ?? 'image_original',
        status: 'Available',
      }])
    );
  }

  function prependPost(posts: PostView[], post: PostView) {
    return [post, ...posts.filter((current) => current.object_id !== post.object_id)];
  }

  function patchLocalPosts(
    localId: string,
    updater: (post: PostView) => PostView,
    topicId: string = activeTopic
  ) {
    const patch = (posts: PostView[]) =>
      posts.map((post) => (post.local_id === localId ? updater(post) : post));

    setTimelinesByKey((current) => {
      let changed = false;
      const next = { ...current };
      for (const [key, posts] of Object.entries(current)) {
        if (!key.startsWith(`${topicId}::`) || !posts.some((post) => post.local_id === localId)) {
          continue;
        }
        next[key] = patch(posts);
        changed = true;
      }
      return changed ? next : current;
    });
    setThreadsById((current) => {
      let changed = false;
      const next = { ...current };
      for (const [threadId, posts] of Object.entries(current)) {
        if (!posts.some((post) => post.local_id === localId)) continue;
        next[threadId] = patch(posts);
        changed = true;
      }
      return changed ? next : current;
    });
    setProfileTimeline((current) => patch(current));
  }

  function findKnownPost(objectId: string): PostView | null {
    const currentState = storeApi.getState();
    const activeScope = activeWorkspaceScope(currentState.workspaceState);
    const lists = [
      ...Object.values(currentState.threadsById),
      currentState.timelinesByKey[
        activeTimelineStorageKey(currentState, activeScope.topicId)
      ] ?? [],
      ...Object.values(currentState.timelinesByKey),
      currentState.profileTimeline,
      selectSelectedAuthorPosts(currentState),
    ];
    for (const posts of lists) {
      const match = posts.find((post) => post.object_id === objectId);
      if (match) {
        return match;
      }
    }
    return null;
  }

  function createOptimisticPost(args: {
    createdAt: number;
    localId: string;
    draft: LocalPostDraft;
    draftMedia: LocalDraftMediaItem[];
    replyPost?: PostView | null;
    repostPost?: PostView | null;
  }): PostView {
    const { createdAt, localId, draft, draftMedia, replyPost = null, repostPost = null } = args;
    const isRepost = draft.kind === 'repost' && repostPost;
    const channelId = draft.kind === 'post' ? channelIdOfRef(draft.channel_ref) : null;
    const publicThread = draft.kind === 'post' && draft.channel_ref?.kind === 'public_channel';
    const channelLabel = channelId
      ? storeApi
          .getState()
          .joinedChannelsByTopic[draft.topic]?.find(
            (channel) => channel.channel_id === channelId
          )?.label
      : null;
    const rootId = replyPost ? replyPost.root_id ?? replyPost.object_id : localId;
    return {
      object_id: localId,
      envelope_id: localId,
      author_pubkey: localAuthorPubkey,
      author_name: localProfile?.name ?? null,
      author_display_name: localProfile?.display_name ?? null,
      following: false,
      followed_by: false,
      mutual: false,
      friend_of_friend: false,
      object_kind: isRepost ? 'repost' : replyPost ? 'comment' : 'post',
      content: draft.content,
      content_status: 'Available',
      content_labels: draft.kind === 'post' ? draft.content_labels ?? [] : [],
      live: draft.live ?? null,
      attachments: attachmentViewsFromDraftMediaItems(localId, draftMedia),
      created_at: createdAt,
      reply_to: replyPost?.object_id ?? null,
      reply_preview: replyPost
        ? {
            object_id: replyPost.object_id,
            topic: publishedTopicIdForPost(replyPost) ?? draft.topic,
            author: {
              pubkey: replyPost.author_pubkey,
              name: replyPost.author_name ?? null,
              display_name: replyPost.author_display_name ?? null,
              picture_asset: replyPost.author_picture_asset ?? null,
            },
            content: replyPost.content,
            content_status: replyPost.content_status,
            attachments: replyPost.attachments.map((attachment) => ({ ...attachment })),
            root_id: replyPost.root_id ?? null,
            reply_to: replyPost.reply_to ?? null,
          }
        : null,
      root_id: isRepost ? null : rootId,
      published_topic_id: draft.topic,
      origin_topic_id: draft.topic,
      repost_of: repostPost
        ? {
            source_object_id: repostPost.object_id,
            source_topic_id: publishedTopicIdForPost(repostPost) ?? draft.topic,
            source_author_pubkey: repostPost.author_pubkey,
            source_author_name: repostPost.author_name ?? null,
            source_author_display_name: repostPost.author_display_name ?? null,
            source_object_kind: repostPost.object_kind,
            content: repostPost.content,
            attachments: repostPost.attachments.map((attachment) => ({ ...attachment })),
            reply_to: repostPost.reply_to ?? null,
            root_id: repostPost.root_id ?? null,
          }
        : null,
      repost_commentary: repostPost ? (draft.content.trim() || null) : null,
      is_threadable: repostPost ? Boolean(draft.content.trim()) : true,
      channel_id: channelId,
      audience_label:
        replyPost?.audience_label ??
        (publicThread ? 'Public thread' : channelId ? channelLabel ?? 'Private channel' : 'Public'),
      reaction_summary: [],
      my_reactions: [],
      local_id: localId,
      local_state: 'pending',
      local_error: null,
      server_object_id: null,
      local_draft: {
        ...draft,
        attachments: draft.attachments?.map((attachment) => ({ ...attachment })) ?? [],
      },
      local_draft_media_items: draftMedia,
    };
  }

  function insertOptimisticPost(post: PostView) {
    const currentState = storeApi.getState();
    const topicId = post.published_topic_id ?? activeTopic;
    const timelineKey = timelineStorageKeyForChannel(topicId, post.channel_id ?? null);
    setTimelinesByKey(updateRecordEntry(timelineKey, (prev) => prependPost(prev ?? [], post)));
    if (post.root_id && currentState.selectedThread === post.root_id) {
      setThreadsById(
        updateRecordEntry(post.root_id, (current) => prependPost(current ?? [], post))
      );
    }
    // Profile lists confirmed author-replica data. Pending/failed drafts stay in
    // their posting timeline; a successful publish refreshes the profile feed.
  }

  async function submitOptimisticPost(post: PostView): Promise<boolean> {
    const draft = post.local_draft;
    const draftMedia = cloneDraftMediaItems(post.local_draft_media_items ?? []);
    if (!draft || !post.local_id) {
      return false;
    }
    patchLocalPosts(
      post.local_id,
      (current) => ({
        ...current,
        local_state: 'pending',
        local_error: null,
      }),
      draft.topic
    );
    try {
      const serverObjectId =
        draft.kind === 'repost' && draft.source_topic && draft.source_object_id
          ? await api.createRepost(
              draft.topic,
              draft.source_topic,
              draft.source_object_id,
              draft.content.trim() || null
            )
          : await api.createPost(
              draft.topic,
              draft.content,
              draft.reply_to ?? null,
              draft.attachments ?? [],
              draft.channel_ref ?? PUBLIC_CHANNEL_REF,
              draft.content_labels ?? [],
              draft.live ?? null
            );
      for (const item of draftMedia) {
        releaseDraftPreview(item.id);
      }
      patchLocalPosts(
        post.local_id,
        (current) => ({
          ...current,
          local_state: 'syncing',
          local_error: null,
          server_object_id: serverObjectId,
          local_draft: null,
          local_draft_media_items: [],
        }),
        draft.topic
      );
      void refreshVisibleTimelineAfterPublish(
        draft.topic,
        draft.reply_to ? post.root_id ?? draft.reply_to : null,
        channelIdOfRef(draft.channel_ref)
      );
      return true;
    } catch (publishError) {
      const message =
        publishError instanceof Error
          ? publishError.message
          : translate('common:errors.failedToPublish');
      patchLocalPosts(
        post.local_id,
        (current) => ({
          ...current,
          local_state: 'failed',
          local_error: message,
        }),
        draft.topic
      );
      return false;
    }
  }

  /** #1839: Live の紹介 post を出し先の timeline へ置いて送る。送れたら true。 */
  async function publishLiveIntro(topic: string, channelId: string | null, content: string, live: PostLiveView) {
    const createdAt = Math.floor(Date.now() / 1000);
    const post = createOptimisticPost({
      createdAt,
      localId: `local-post:${Date.now()}:${Math.random().toString(16).slice(2)}`,
      draft: {
        kind: 'post',
        topic,
        content,
        channel_ref: channelComposeTarget(channelId),
        live,
      },
      draftMedia: [],
    });
    insertOptimisticPost(post);
    return submitOptimisticPost(post);
  }

  return {
    cloneDraftMediaItems,
    createOptimisticPost,
    findKnownPost,
    insertOptimisticPost,
    publishLiveIntro,
    submitOptimisticPost,
  };
}

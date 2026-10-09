import type {
  AuthorDetailView,
  PostCardView,
  PostMediaView,
  ThreadPanelState,
  TopicDiagnosticSummary,
} from '@/components/core/types';
import type { PrimarySection } from '@/components/shell/types';
import type { ContentProvenance } from '@/lib/api';
import i18n from '@/i18n';
import { formatLocalizedTime } from '@/i18n/format';

export const STORY_ACTIVE_TOPIC = 'kukuri:topic:demo';

export function createStoryPrimaryItems(): Array<{
  id: PrimarySection;
  label: string;
  description: string;
}> {
  return [
    { id: 'timeline', label: i18n.t('shell:primarySections.timeline'), description: 'Feed and scope controls' },
    { id: 'live', label: i18n.t('shell:primarySections.live'), description: 'Live sessions and status' },
    { id: 'game', label: i18n.t('shell:primarySections.game'), description: 'Scoreboards and room editing' },
    { id: 'profile', label: i18n.t('shell:primarySections.profile'), description: 'Edit author identity' },
  ];
}


export function createStoryTopicItems(): TopicDiagnosticSummary[] {
  return [
    {
      topic: STORY_ACTIVE_TOPIC,
      active: true,
      publicActive: true,
      removable: false,
      connectionLabel: i18n.t('common:states.joined'),
      peerCount: 2,
      lastReceivedLabel: formatLocalizedTime('2026-03-28T12:45:11Z'),
    },
    {
      topic: 'kukuri:topic:relay',
      active: false,
      publicActive: false,
      removable: true,
      connectionLabel: i18n.t('common:states.relayAssisted'),
      peerCount: 1,
      lastReceivedLabel: i18n.t('common:fallbacks.noEvents'),
    },
  ];
}


export const STORY_IMAGE_PREVIEW =
  'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%2300b3a4"/><path d="M0 300l140-130 100 80 110-120 150 170H0z" fill="%230f2231"/></svg>';

export const STORY_VIDEO_POSTER_PREVIEW =
  'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%23101923"/><rect x="110" y="70" width="420" height="220" rx="24" fill="%23f59d62"/><polygon points="285,145 285,215 355,180" fill="%23101923"/></svg>';

export const STORY_VIDEO_PLAYBACK_SRC =
  'data:video/mp4;base64,AAAAIGZ0eXBpc29tAAACAGlzb21pc28yYXZjMW1wNDEAAABsbXZoZAAAAAAAAAAAAAAAAAAAA+gAAAPoAAEAAAEAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIAAAIVdHJhawAAAFx0a2hkAAAAAwAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAQAAAEAAAAAAAAkZWR0cwAAABxlbHN0AAAAAAAAAAEAAAPoAAAAAAABAAAAAAEAAAAqbWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAAAyAAAAMgBVxAAAAAAALWhkbHIAAAAAAAAAAHZpZGUAAAAAAAAAAAAAAABWaWRlb0hhbmRsZXIAAAAClW1pbmYAAAAUdm1oZAAAAAEAAAAAAAAAAAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAAl1zdGJsAAAArXN0c2QAAAAAAAAAAQAAAJ1hdmMxAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAAQABAAABAAABAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABj//wAAADNhdmNDAWQAHv/hABdnZAAerNlChAAAAMAEAAAAwHihUkA=';

const timelineRootPost = {
  object_id: 'timeline-post-1',
  envelope_id: 'env-timeline-post-1',
  is_threadable: true,
  author_pubkey: 'b'.repeat(64),
  author_name: 'bob',
  author_display_name: null,
  following: true,
  followed_by: true,
  mutual: true,
  friend_of_friend: false,
  object_kind: 'post' as const,
  content: 'Core workspace now owns topic switching, composer, and thread context.',
  content_status: 'Available' as const,
  attachments: [],
  created_at: 1,
  reply_to: null,
  root_id: 'timeline-post-1',
  channel_id: null,
  audience_label: 'Public',
  reaction_summary: [],
  my_reactions: [],
};

// 正本は author docs だが、community index 経由で観測したことを示す provenance 例。
// truth source ではなく観測経路として community node を表す。
export const STORY_INDEX_PROVENANCE: ContentProvenance = {
  canonicalSource: 'author_docs',
  observedVia: [
    {
      nodeBaseUrl: 'https://node.example',
      capability: 'community_index',
      nodeRole: 'community-node',
      manifestVersion: 'v1',
    },
  ],
  responsibleReportTargets: [
    {
      nodeBaseUrl: 'https://node.example',
      capability: 'community_index',
      reportEndpoint: 'https://node.example/v1/report',
      abuseContact: 'abuse@node.example',
    },
  ],
};

export function createStoryTimelinePosts(): PostCardView[] {
  return [
    {
      post: timelineRootPost,
      context: 'timeline',
      authorLabel: 'bob',
      authorPicture: null,
      relationshipLabel: 'mutual',
      audience: { kind: 'public' },
      threadTargetId: 'timeline-post-1',
      provenance: STORY_INDEX_PROVENANCE,
      media: {
        objectId: 'timeline-post-1',
        kind: null,
        statusLabel: null,
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: null,
        metaBytesLabel: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    },
    {
      post: {
        object_id: 'timeline-post-2',
        envelope_id: 'env-timeline-post-2',
        author_pubkey: 'c'.repeat(64),
        author_name: 'carol',
        author_display_name: 'Carol',
        following: false,
        followed_by: false,
        mutual: false,
        friend_of_friend: true,
        object_kind: 'post',
        is_threadable: true,
        content: 'Image preview stays visible before the blob finishes syncing.',
        content_status: 'Available',
        attachments: [],
        created_at: 2,
        reply_to: null,
        root_id: 'timeline-post-2',
        channel_id: null,
        audience_label: 'Public',
        reaction_summary: [],
        my_reactions: [],
      },
      context: 'timeline',
      authorLabel: 'Carol',
      authorPicture: null,
      relationshipLabel: 'friend of friend',
      audience: { kind: 'public' },
      threadTargetId: 'timeline-post-2',
      media: {
        objectId: 'timeline-post-2',
        kind: 'image',
        statusLabel: i18n.t('common:media.imageReady'),
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: 'image/png',
        metaBytesLabel: '144 KB',
        imageGalleryItems: [{ hash: 'story-image-hash', src: STORY_IMAGE_PREVIEW, mime: 'image/png' }],
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    },
  ];
}


export function createStoryThreadPosts(): PostCardView[] {
  const timelinePosts = createStoryTimelinePosts();
  return [
    {
      ...timelinePosts[0],
      context: 'thread',
      suppressReplyPreview: true,
    },
    {
      post: {
        object_id: 'thread-reply-1',
        envelope_id: 'env-thread-reply-1',
        author_pubkey: 'd'.repeat(64),
        author_name: 'dan',
        author_display_name: null,
        following: false,
        followed_by: true,
        mutual: false,
        friend_of_friend: false,
        object_kind: 'comment',
        is_threadable: true,
        content: 'Reply mode keeps the audience label and lets the user publish in place.',
        content_status: 'Available',
        attachments: [],
        created_at: 3,
        reply_to: 'timeline-post-1',
        root_id: 'timeline-post-1',
        channel_id: null,
        audience_label: 'Public',
        reaction_summary: [],
        my_reactions: [],
      },
      context: 'thread',
      suppressReplyPreview: true,
      authorLabel: 'dan',
      authorPicture: null,
      relationshipLabel: 'follows you',
      audience: { kind: 'public' },
      threadTargetId: 'timeline-post-1',
      media: {
        objectId: 'thread-reply-1',
        kind: null,
        statusLabel: null,
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: null,
        metaBytesLabel: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    },
    {
      post: {
        object_id: 'thread-reply-2',
        envelope_id: 'env-thread-reply-2',
        author_pubkey: 'b'.repeat(64),
        author_name: 'bob',
        author_display_name: null,
        following: true,
        followed_by: true,
        mutual: true,
        friend_of_friend: false,
        object_kind: 'comment',
        is_threadable: true,
        content: 'Nested replies indent under their parent so the conversation reads as a tree.',
        content_status: 'Available',
        attachments: [],
        created_at: 4,
        reply_to: 'thread-reply-1',
        root_id: 'timeline-post-1',
        channel_id: null,
        audience_label: 'Public',
        reaction_summary: [],
        my_reactions: [],
      },
      context: 'thread',
      suppressReplyPreview: true,
      authorLabel: 'bob',
      authorPicture: null,
      relationshipLabel: 'mutual',
      audience: { kind: 'public' },
      threadTargetId: 'timeline-post-1',
      media: {
        objectId: 'thread-reply-2',
        kind: null,
        statusLabel: null,
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: null,
        metaBytesLabel: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    },
    {
      post: {
        object_id: 'thread-reply-3',
        envelope_id: 'env-thread-reply-3',
        author_pubkey: 'e'.repeat(64),
        author_name: 'erin',
        author_display_name: null,
        following: false,
        followed_by: false,
        mutual: false,
        friend_of_friend: true,
        object_kind: 'comment',
        is_threadable: true,
        content: 'A sibling reply to the root keeps its own branch in the tree.',
        content_status: 'Available',
        attachments: [],
        created_at: 5,
        reply_to: 'timeline-post-1',
        root_id: 'timeline-post-1',
        channel_id: null,
        audience_label: 'Public',
        reaction_summary: [],
        my_reactions: [],
      },
      context: 'thread',
      suppressReplyPreview: true,
      authorLabel: 'erin',
      authorPicture: null,
      relationshipLabel: null,
      audience: { kind: 'public' },
      threadTargetId: 'timeline-post-1',
      media: {
        objectId: 'thread-reply-3',
        kind: null,
        statusLabel: null,
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: null,
        metaBytesLabel: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    },
  ];
}

export const STORY_THREAD_POSTS = createStoryThreadPosts();

export function createStoryThreadPanelState(): ThreadPanelState {
  return {
    selectedThreadId: 'timeline-post-1',
    summary: i18n.t('shell:context.threadSummary', { count: 2 }),
    emptyCopy: i18n.t('shell:context.threadEmpty'),
  };
}

export const STORY_THREAD_PANEL_STATE = createStoryThreadPanelState();

export function createStoryAuthorDetailView(): AuthorDetailView {
  return {
    author: {
      author_pubkey: 'b'.repeat(64),
      name: 'bob',
      display_name: null,
      about: 'Maintains the topic-first shell and community-node connectivity reviews.',
      updated_at: 1,
      following: true,
      followed_by: true,
      mutual: true,
      friend_of_friend: false,
      friend_of_friend_via_pubkeys: [],
      muted: false,
      blocking: false,
      blocked_by: false,
    },
    displayLabel: 'bob',
    summary: {
      label: i18n.t('common:relationships.mutual'),
      following: true,
      followedBy: true,
      mutual: true,
      friendOfFriend: false,
      muted: false,
      viaPubkeys: [],
      isSelf: false,
      canFollow: true,
      followActionLabel: 'Unfollow',
      muteActionLabel: 'Mute',
      blockActionLabel: 'Block',
    },
    authorError: null,
  };
}

export const STORY_AUTHOR_DETAIL_VIEW = createStoryAuthorDetailView();

export const STORY_EMPTY_AUTHOR_DETAIL_VIEW: AuthorDetailView = {
  author: null,
  displayLabel: '',
  summary: null,
  authorError: null,
};

export function createStoryImageMedia(): PostMediaView {
  return {
    objectId: 'timeline-post-2',
    kind: 'image',
    statusLabel: i18n.t('common:media.imageReady'),
    extraAttachmentCount: 0,
    state: 'ready',
    metaMime: 'image/png',
    metaBytesLabel: '144 KB',
    imageGalleryItems: [{ hash: 'story-image-hash', src: STORY_IMAGE_PREVIEW, mime: 'image/png' }],
    videoPosterPreviewSrc: null,
    videoPlaybackSrc: null,
    videoReportHash: 'story-video-poster-hash',
    videoUnsupportedOnClient: false,
  };
}

export const STORY_IMAGE_MEDIA = createStoryImageMedia();

export function createStoryVideoPosterMedia(): PostMediaView {
  return {
    objectId: 'video-post',
    kind: 'video',
    statusLabel: i18n.t('common:media.posterReady'),
    extraAttachmentCount: 1,
    state: 'ready',
    metaMime: 'video/mp4',
    metaBytesLabel: '8.0 KB',
    videoPosterPreviewSrc: STORY_VIDEO_POSTER_PREVIEW,
    videoPlaybackSrc: null,
    videoReportHash: 'story-video-poster-hash',
    videoUnsupportedOnClient: false,
  };
}

export const STORY_VIDEO_POSTER_MEDIA = createStoryVideoPosterMedia();

export function createStoryVideoPlayableMedia(): PostMediaView {
  return {
    objectId: 'video-post',
    kind: 'video',
    statusLabel: i18n.t('common:media.playableVideo'),
    extraAttachmentCount: 0,
    state: 'ready',
    metaMime: 'video/mp4',
    metaBytesLabel: '8.0 KB',
    videoPosterPreviewSrc: STORY_VIDEO_POSTER_PREVIEW,
    videoPlaybackSrc: STORY_VIDEO_PLAYBACK_SRC,
    videoReportHash: 'story-video-manifest-hash',
    videoUnsupportedOnClient: false,
  };
}

export const STORY_VIDEO_PLAYABLE_MEDIA = createStoryVideoPlayableMedia();

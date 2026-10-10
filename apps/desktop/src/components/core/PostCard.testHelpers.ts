import type { PostCardView } from './types';

/// PostCard 試験の共通の表示データ。PostCard.test.tsx と PostCard.report.test.tsx が共有する。
export function createView(overrides?: Partial<PostCardView>): PostCardView {
  return {
    post: {
      object_id: 'post-1',
      envelope_id: 'envelope-post-1',
      author_pubkey: 'a'.repeat(64),
      author_name: 'alice',
      author_display_name: 'Alice',
      following: false,
      followed_by: false,
      mutual: false,
      friend_of_friend: false,
      object_kind: 'post',
      content: 'hello',
      content_status: 'Available',
      attachments: [],
      created_at: 1,
      reply_to: null,
      root_id: 'post-1',
      published_topic_id: null,
      origin_topic_id: null,
      repost_of: null,
      repost_commentary: null,
      is_threadable: true,
      channel_id: null,
      audience_label: 'Public',
      reaction_summary: [],
      my_reactions: [],
    },
    context: 'timeline',
    authorLabel: 'Alice',
    authorPicture: null,
    relationshipLabel: null,
    audience: { kind: 'public' },
    threadTargetId: 'post-1',
    media: {
      objectId: 'post-1',
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
    ...overrides,
  };
}


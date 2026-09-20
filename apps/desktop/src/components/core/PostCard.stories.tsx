import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import type { LinkPreviewFetcher, SubmitCommunityNodeReportResult } from '@/lib/api';

import { MediaRetryContext } from './mediaRetryContext';
import { PostCard } from './PostCard';
import { type PostCardView } from './types';

const basePost = {
  object_id: 'post-1',
  envelope_id: 'envelope-post-1',
  is_threadable: true,
  author_pubkey: 'a'.repeat(64),
  author_name: 'alice',
  author_display_name: 'Alice',
  following: false,
  followed_by: false,
  mutual: false,
  friend_of_friend: false,
  object_kind: 'post',
  content: 'Core product flow draft post',
  content_status: 'Available' as const,
  attachments: [],
  created_at: 1,
  reply_to: null,
  root_id: 'post-1',
  channel_id: null,
  audience_label: 'Public',
};

const inviteTokenPostContent = JSON.stringify({
  envelope: {
    kind: 'channel-invite',
    pubkey: 'b'.repeat(64),
    content: JSON.stringify({
      channel_id: 'channel-core',
      topic_id: 'kukuri:topic:demo',
      channel_label: 'Core Contributors',
      owner_pubkey: 'b'.repeat(64),
      epoch_id: 'epoch-1',
    }),
  },
});

const storyLinkPreviewFetcher: LinkPreviewFetcher = async (url) => ({
  status: 'available',
  preview: {
    url,
    source_label: 'Kukuri Preview Notes',
    title: 'A bounded OGP preview for public posts with a deliberately long title',
    description:
      'The post stays readable while metadata is fetched, and falls back to the inline link when a preview is unavailable.',
    image_data_url:
      'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsMAAA7DAcdvqGQAAAA0SURBVFhH7c4hEQBACABBZugfixJf5vEEALPizKmNfPUvizm2AwAAAAAAAAAAAAAAAAAAaInJhJdNUjWLAAAAAElFTkSuQmCC',
  },
});

function createView(overrides?: Partial<PostCardView>): PostCardView {
  return {
    post: basePost,
    context: 'timeline',
    authorLabel: 'Alice',
    authorPicture: null,
    relationshipLabel: null,
    audienceChipLabel: 'Public',
    threadTargetId: 'post-1',
    media: {
      objectId: 'post-1',
      kind: null,
      extraAttachmentCount: 0,
      state: 'ready',
      metaMime: null,
      metaBytesLabel: null,
      imagePreviewSrc: null,
      videoPosterPreviewSrc: null,
      videoPlaybackSrc: null,
      videoUnsupportedOnClient: false,
    },
    ...overrides,
  };
}

const meta = {
  title: 'Core/PostCard',
  parameters: {
    layout: 'centered',
  },
  render: (args) => (
    <div className='w-[min(42rem,calc(100vw-2rem))]'>
      <PostCard
        view={args.view}
        onOpenAuthor={() => undefined}
        onOpenThread={() => undefined}
        onReply={() => undefined}
        onSubmitReport={
          args.reportable
            ? () => new Promise<SubmitCommunityNodeReportResult>(() => undefined)
            : undefined
        }
      />
    </div>
  ),
} satisfies Meta<{ view: PostCardView; reportable?: boolean }>;

export default meta;

type Story = StoryObj<typeof meta>;

export const ImagePending: Story = {
  args: {
    view: createView({
      media: {
        objectId: 'image-post',
        kind: 'image',
        extraAttachmentCount: 0,
        state: 'loading',
        metaMime: 'image/png',
        metaBytesLabel: '2.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    }),
  },
};

export const ImageReady: Story = {
  args: {
    view: createView({
      media: {
        objectId: 'image-post',
        kind: 'image',
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: 'image/png',
        metaBytesLabel: '2.0 KB',
        imagePreviewSrc:
          'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%2300b3a4"/><circle cx="180" cy="120" r="42" fill="%23ffd36e"/><path d="M0 280l120-100 80 70 110-130 130 160H0z" fill="%230f2231"/></svg>',
        imageGalleryItems: [
          {
            hash: 'image-1',
            src: 'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%2300b3a4"/><circle cx="180" cy="120" r="42" fill="%23ffd36e"/><path d="M0 280l120-100 80 70 110-130 130 160H0z" fill="%230f2231"/></svg>',
            mime: 'image/png',
          },
          {
            hash: 'image-2',
            src: 'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%23f59d62"/><path d="M0 260l140-90 120 80 140-130 240 140H0z" fill="%23101923"/></svg>',
            mime: 'image/png',
          },
        ],
        currentImageIndex: 0,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    }),
  },
};

export const ExternalLinkPreview: Story = {
  args: {
    view: createView({
      post: {
        ...basePost,
        content:
          'URL parsing and OGP preview notes https://example.test/releases/1174?source=public-post',
      },
    }),
  },
  render: ({ view }) => (
    <div className='w-[min(30rem,calc(100vw-2rem))]'>
      <PostCard
        enableLinkPreview
        linkPreviewFetcher={storyLinkPreviewFetcher}
        view={view}
        onOpenAuthor={() => undefined}
        onOpenThread={() => undefined}
        onReply={() => undefined}
      />
    </div>
  ),
};

export const CustomReactionSummary: Story = {
  args: {
    view: createView({
      post: {
        ...basePost,
        reaction_summary: [
          {
            reaction_key_kind: 'emoji',
            normalized_reaction_key: 'emoji:👍',
            emoji: '👍',
            custom_asset: null,
            count: 8,
          },
          {
            reaction_key_kind: 'custom_asset',
            normalized_reaction_key: 'custom_asset:party-parrot',
            emoji: null,
            custom_asset: {
              asset_id: 'party-parrot',
              owner_pubkey: 'b'.repeat(64),
              blob_hash: 'party-parrot-blob',
              search_key: 'party-parrot-with-a-long-name',
              mime: 'image/png',
              bytes: 128,
              width: 128,
              height: 128,
            },
            count: 4,
          },
          {
            reaction_key_kind: 'custom_asset',
            normalized_reaction_key: 'custom_asset:preview-pending',
            emoji: null,
            custom_asset: {
              asset_id: 'preview-pending',
              owner_pubkey: 'c'.repeat(64),
              blob_hash: 'preview-pending-blob',
              search_key: 'preview-pending',
              mime: 'image/png',
              bytes: 128,
              width: 128,
              height: 128,
            },
            count: 2,
          },
        ],
        my_reactions: [
          {
            reaction_key_kind: 'custom_asset',
            normalized_reaction_key: 'custom_asset:party-parrot',
            emoji: null,
            custom_asset: {
              asset_id: 'party-parrot',
              owner_pubkey: 'b'.repeat(64),
              blob_hash: 'party-parrot-blob',
              search_key: 'party-parrot-with-a-long-name',
              mime: 'image/png',
              bytes: 128,
              width: 128,
              height: 128,
            },
          },
        ],
      },
    }),
  },
  render: ({ view }) => (
    <div className='w-[min(42rem,calc(100vw-2rem))]'>
      <PostCard
        view={view}
        onOpenAuthor={() => undefined}
        onOpenThread={() => undefined}
        onReply={() => undefined}
        onToggleReaction={() => undefined}
        mediaObjectUrls={{
          'party-parrot-blob':
            'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="%2300b3a4"/><circle cx="16" cy="16" r="8" fill="%23ffd36e"/></svg>',
        }}
      />
    </div>
  ),
};

// #1055: Community Node の content advisory による代替表示。断定せず推定であることと、
// 発行元 / 分類 / 確信度 / 根拠を示し、異議申し立てへの導線を持つ(ADR 0046 §6.3)。
// #1108: 一覧には枠と短いラベルだけを出し、説明は枠を開いた詳細 dialog に置く。
const storyAdvisory = {
  issuerNodeId: 'd'.repeat(64),
  nodeBaseUrl: 'https://index-a.example',
  nodeName: 'index-a.example',
  category: 'nsfw',
  label: 'adult',
  confidence: 84,
  basis: 'classifier_score',
  signalId: 'signal-1',
  subjectKind: 'blob_cid',
  subjectId: 'a'.repeat(64),
} as const;

function advisoryGatedView(kind: 'image' | 'video' | null): PostCardView {
  return createView({
    adultContentGated: true,
    gatedBy: 'advisory',
    contentAdvisory: kind
      ? storyAdvisory
      : { ...storyAdvisory, subjectKind: 'post_id', subjectId: 'post-1' },
    media: {
      objectId: 'post-1',
      kind,
      extraAttachmentCount: 0,
      state: kind ? 'gated' : 'ready',
      gatedBy: kind ? 'advisory' : undefined,
      metaMime: kind === 'video' ? 'video/mp4' : kind ? 'image/png' : null,
      metaBytesLabel: kind ? '2.0 KB' : null,
      imagePreviewSrc: null,
      imageGalleryItems: [],
      videoPosterPreviewSrc: null,
      videoPlaybackSrc: null,
      videoUnsupportedOnClient: false,
    },
  });
}

export const AdvisoryGated: Story = {
  args: { view: advisoryGatedView('image'), reportable: true },
};

export const AdvisoryGatedVideo: Story = {
  args: { view: advisoryGatedView('video'), reportable: true },
};

// メディア枠を持たない投稿は、本文欄に詳細を開く操作だけを出す。
export const AdvisoryGatedTextOnly: Story = {
  args: { view: advisoryGatedView(null), reportable: true },
};

export const AdvisoryDetailsOpen: Story = {
  args: { view: advisoryGatedView('image'), reportable: true },
  play: async ({ canvasElement }) => {
    await userEvent.click(within(canvasElement).getByTestId('media-adult-gated-post-1'));
    const dialog = await within(canvasElement.ownerDocument.body).findByRole('dialog');
    await expect(within(dialog).getByTestId('post-advisory-gated-post-1')).toBeInTheDocument();
    await expect(within(dialog).getByTestId('post-advisory-appeal-post-1')).toBeInTheDocument();
  },
};

export const UnavailableContentAndMedia: Story = {
  args: {
    view: createView({
      post: {
        ...basePost,
        content: '[blob pending]',
        content_status: 'Missing',
      },
      media: {
        objectId: 'image-post',
        kind: 'image',
        extraAttachmentCount: 0,
        state: 'unavailable',
        metaMime: 'image/png',
        metaBytesLabel: '2.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
      showUnavailableDiagnostics: false,
    }),
  },
};

export const DeveloperUnavailableDiagnostics: Story = {
  args: {
    view: createView({
      post: {
        ...basePost,
        content: '[blob pending]',
        content_status: 'Missing',
      },
      media: {
        objectId: 'image-post',
        kind: 'image',
        extraAttachmentCount: 0,
        state: 'unavailable',
        metaMime: 'image/png',
        metaBytesLabel: '2.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
      showUnavailableDiagnostics: true,
    }),
  },
};

// #1207: 自動取得が上限に達したメディア。失敗した部分だけを置き換え、再取得の icon button を出す。
export const MediaFetchFailed: Story = {
  decorators: [
    (StoryComponent) => (
      <MediaRetryContext.Provider value={() => undefined}>
        <StoryComponent />
      </MediaRetryContext.Provider>
    ),
  ],
  args: {
    view: createView({
      media: {
        objectId: 'image-post',
        kind: 'image',
        extraAttachmentCount: 0,
        state: 'unavailable',
        metaMime: 'image/png',
        metaBytesLabel: '2.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
        retryHashes: ['hash-image'],
        retrying: false,
      },
    }),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText('Failed to load.')).toBeInTheDocument();
    await expect(canvas.getByRole('button', { name: 'Retry loading' })).toHaveAttribute(
      'aria-disabled',
      'false'
    );
  },
};

// #1207: 再取得中。button は同じ位置に残し、busy を示して重複操作を受け付けない。
export const MediaFetchRetrying: Story = {
  decorators: MediaFetchFailed.decorators,
  args: {
    view: createView({
      media: {
        objectId: 'image-post',
        kind: 'image',
        extraAttachmentCount: 0,
        state: 'unavailable',
        metaMime: 'image/png',
        metaBytesLabel: '2.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc: null,
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
        retryHashes: ['hash-image'],
        retrying: true,
      },
    }),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole('button', { name: 'Retry loading' })).toHaveAttribute(
      'aria-disabled',
      'true'
    );
  },
};

export const VideoPosterOnly: Story = {
  args: {
    view: createView({
      relationshipLabel: 'friend of friend',
      media: {
        objectId: 'video-post',
        kind: 'video',
        extraAttachmentCount: 1,
        state: 'ready',
        metaMime: 'video/mp4',
        metaBytesLabel: '8.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc:
          'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%23101923"/><rect x="110" y="70" width="420" height="220" rx="24" fill="%23f59d62"/><polygon points="285,145 285,215 355,180" fill="%23101923"/></svg>',
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: false,
      },
    }),
  },
};

export const VideoPlayable: Story = {
  args: {
    view: createView({
      relationshipLabel: 'mutual',
      media: {
        objectId: 'video-post',
        kind: 'video',
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: 'video/mp4',
        metaBytesLabel: '8.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc:
          'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%23101923"/><rect x="110" y="70" width="420" height="220" rx="24" fill="%2300b3a4"/></svg>',
        videoPlaybackSrc:
          'data:video/mp4;base64,AAAAIGZ0eXBpc29tAAACAGlzb21pc28yYXZjMW1wNDEAAABsbXZoZAAAAAAAAAAAAAAAAAAAA+gAAAPoAAEAAAEAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAIAAAIVdHJhawAAAFx0a2hkAAAAAwAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAQAAAEAAAAAAAAkZWR0cwAAABxlbHN0AAAAAAAAAAEAAAPoAAAAAAABAAAAAAEAAAAqbWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAAAyAAAAMgBVxAAAAAAALWhkbHIAAAAAAAAAAHZpZGUAAAAAAAAAAAAAAABWaWRlb0hhbmRsZXIAAAAClW1pbmYAAAAUdm1oZAAAAAEAAAAAAAAAAAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAAl1zdGJsAAAArXN0c2QAAAAAAAAAAQAAAJ1hdmMxAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAAQABAAABAAABAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABj//wAAADNhdmNDAWQAHv/hABdnZAAerNlChAAAAMAEAAAAwHihUkA=',
        videoUnsupportedOnClient: false,
      },
    }),
  },
};

export const UnsupportedVideo: Story = {
  args: {
    view: createView({
      media: {
        objectId: 'video-post',
        kind: 'video',
        extraAttachmentCount: 0,
        state: 'ready',
        metaMime: 'video/mp4',
        metaBytesLabel: '8.0 KB',
        imagePreviewSrc: null,
        videoPosterPreviewSrc:
          'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%23101923"/><text x="50%" y="52%" dominant-baseline="middle" text-anchor="middle" fill="%23f6f1e8" font-size="40">unsupported</text></svg>',
        videoPlaybackSrc: null,
        videoUnsupportedOnClient: true,
      },
    }),
  },
};

export const ChannelAccessToken: Story = {
  args: {
    view: createView({
      post: {
        ...basePost,
        object_id: 'token-post',
        envelope_id: 'envelope-token-post',
        is_threadable: true,
        content: inviteTokenPostContent,
      },
      threadTargetId: 'token-post',
    }),
  },
};

export const PureRepost: Story = {
  args: {
    view: createView({
      canReply: false,
      threadTargetId: 'source-root',
      threadTopicId: 'kukuri:topic:source',
      repostSourceAuthor: { pubkey: 'b'.repeat(64), label: 'Source Author', picture: null },
      post: {
        ...basePost,
        object_kind: 'repost',
        is_threadable: false,
        content: '',
        repost_commentary: null,
        repost_of: {
          source_object_id: 'source-1',
          source_topic_id: 'kukuri:topic:source',
          source_author_pubkey: 'b'.repeat(64),
          source_author_display_name: 'Source Author',
          source_author_name: null,
          source_object_kind: 'post',
          content: 'The original post is now the star of a repost, with a subtle attribution above.',
          attachments: [],
          reply_to: null,
          root_id: 'source-root',
        },
      },
    }),
  },
};

export const QuoteRepost: Story = {
  args: {
    view: createView({
      repostSourceAuthor: { pubkey: 'b'.repeat(64), label: 'Source Author', picture: null },
      post: {
        ...basePost,
        object_kind: 'repost',
        is_threadable: true,
        content: 'Adding my own take on top of the original.',
        repost_commentary: 'Adding my own take on top of the original.',
        repost_of: {
          source_object_id: 'source-1',
          source_topic_id: 'kukuri:topic:source',
          source_author_pubkey: 'b'.repeat(64),
          source_author_display_name: 'Source Author',
          source_author_name: null,
          source_object_kind: 'post',
          content: 'The quoted post sits in an embedded card with the author avatar.',
          attachments: [],
          reply_to: null,
          root_id: 'source-1',
        },
      },
    }),
  },
};

export const Reply: Story = {
  args: {
    view: createView({
      replyParentAuthor: { pubkey: 'b'.repeat(64), label: 'Parent Author', picture: null },
      post: {
        ...basePost,
        content: '直前の返信先から派生する返信。投稿者・日時・操作はこの返信自身の情報です。',
        reply_to: 'parent-1',
        reply_preview: {
          object_id: 'parent-1',
          topic: 'kukuri:topic:demo',
          author: {
            pubkey: 'b'.repeat(64),
            name: 'parent-author',
            display_name: 'Parent Author',
            picture_asset: null,
          },
          content: '直前の返信対象の簡略表示です。長い本文でも、返信自身の本文と混同せずに会話を追えるようにします。'.repeat(3),
          attachments: [],
          root_id: 'ancestor-root',
          reply_to: 'ancestor-root',
        },
      },
    }),
  },
};

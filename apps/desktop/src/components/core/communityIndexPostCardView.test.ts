import { beforeEach, describe, expect, test } from 'vitest';

import i18n from '@/i18n';
import type {
  AttachmentView,
  AuthorSocialView,
  CommunityIndexResolvedPostView,
  ContentAdvisory,
  IndexEntryView,
  PostView,
} from '@/lib/api';

import { communityIndexPostCardView } from './communityIndexPostCardView';

const entry: IndexEntryView = {
  scope_kind: 'public_topic',
  scope_id: 'kukuri:topic:rust',
  object_id: 'post-1',
  author_pubkey: 'a'.repeat(64),
  text: 'indexed text\nderived-tag',
  created_at: 1_700_000_000,
  content_advisories: [],
};

const knownAuthor: AuthorSocialView = {
  author_pubkey: entry.author_pubkey,
  name: 'alice',
  display_name: 'Alice',
  about: null,
  picture_asset: {
    hash: 'avatar-hash',
    mime: 'image/png',
    bytes: 42,
    role: 'profile_avatar',
  },
  updated_at: null,
  following: true,
  followed_by: false,
  mutual: false,
  friend_of_friend: false,
  friend_of_friend_via_pubkeys: [],
  provenance: null,
  muted: false,
  blocking: false,
  blocked_by: false,
};

function resolvedEntry(content: string): CommunityIndexResolvedPostView {
  const post: PostView = {
    object_id: entry.object_id,
    envelope_id: 'envelope-1',
    author_pubkey: entry.author_pubkey,
    author_name: 'alice',
    author_display_name: 'Alice',
    author_picture_asset: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    provenance: null,
    withdrawal: null,
    content,
    content_status: 'Available',
    attachments: [],
    content_labels: [],
    created_at: entry.created_at,
    reply_to: null,
    reply_preview: null,
    root_id: entry.object_id,
    object_kind: 'post',
    published_topic_id: entry.scope_id,
    origin_topic_id: entry.scope_id,
    repost_of: null,
    repost_commentary: null,
    is_threadable: true,
    channel_id: null,
    audience_label: 'Public',
    reaction_summary: [],
    my_reactions: [],
  };
  return {
    key: `${entry.scope_kind}:${entry.scope_id}:${entry.object_id}`,
    post,
    capabilities: {
      open_thread: true,
      reply: true,
      repost: true,
      quote_repost: true,
      react: true,
      copy_link: true,
      bookmark: true,
      withdraw: false,
    },
  };
}

const PRIMARY_IMAGE_HASH = 'b'.repeat(64);
const SECOND_IMAGE_HASH = 'c'.repeat(64);

function imageAttachments(): AttachmentView[] {
  return [
    {
      hash: PRIMARY_IMAGE_HASH,
      mime: 'image/png',
      bytes: 2048,
      role: 'image_original',
      status: 'Available',
    },
    {
      hash: SECOND_IMAGE_HASH,
      mime: 'image/png',
      bytes: 4096,
      role: 'image_original',
      status: 'Available',
    },
  ];
}

function resolvedImageEntry(labels: string[] = []): CommunityIndexResolvedPostView {
  const resolved = resolvedEntry('canonical signed content');
  if (!resolved.post) throw new Error('resolved post fixture missing');
  resolved.post.attachments = imageAttachments();
  resolved.post.content_labels = labels;
  return resolved;
}

function imageCardView(
  overrides: {
    mediaObjectUrls?: Record<string, string | null>;
    adultContentEnabled?: boolean;
    labels?: string[];
  } = {}
) {
  return communityIndexPostCardView(entry, {
    nodeBaseUrl: 'https://node.example',
    operation: 'search',
    topicId: null,
    knownAuthor,
    resolutionStatus: 'resolved',
    resolvedEntry: resolvedImageEntry(overrides.labels),
    mediaObjectUrls: overrides.mediaObjectUrls ?? {},
    adultContentEnabled: overrides.adultContentEnabled,
    locale: 'en',
  });
}

// #1055: index を返した node が発行した content advisory(ADR 0028 §8.6)。issuer 照合は
// desktop-runtime が済ませているため、view fixture では採用済みのものだけを置く。
const ISSUER_NODE_ID = 'd'.repeat(64);

function blobAdvisory(overrides: Partial<ContentAdvisory> = {}): ContentAdvisory {
  return {
    issuer_node_id: ISSUER_NODE_ID,
    subject_kind: 'blob_cid',
    subject_id: PRIMARY_IMAGE_HASH,
    category: 'nsfw',
    label: 'adult',
    confidence: 84,
    signal_id: 'signal-1',
    basis: 'classifier_score',
    ...overrides,
  };
}

function advisoryEntry(advisories: ContentAdvisory[]): IndexEntryView {
  return { ...entry, content_advisories: advisories };
}

beforeEach(async () => {
  await i18n.changeLanguage('en');
});

describe('communityIndexPostCardView', () => {
  test('does not expose node-provided text before canonical post resolution', () => {
    const view = communityIndexPostCardView(entry, {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      mediaObjectUrls: { 'avatar-hash': 'blob:avatar-hash' },
    });

    expect(view.post.content).not.toContain(entry.text);
  });

  test('maps only resolved canonical post data and known author presentation', () => {
    const canonicalContent = 'canonical signed content';
    const view = communityIndexPostCardView(entry, {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved',
      resolvedEntry: resolvedEntry(canonicalContent),
      mediaObjectUrls: { 'avatar-hash': 'blob:avatar-hash' },
    });

    expect(view.post).toMatchObject({
      object_id: entry.object_id,
      envelope_id: 'envelope-1',
      author_pubkey: entry.author_pubkey,
      content: canonicalContent,
      content_status: 'Available',
      created_at: entry.created_at,
      object_kind: 'post',
      attachments: [],
      reaction_summary: [],
      my_reactions: [],
      is_threadable: true,
      published_topic_id: entry.scope_id,
      reply_to: null,
      repost_of: null,
    });
    expect(view.authorLabel).toBe('Alice');
    expect(view.authorPicture).toBe('blob:avatar-hash');
    expect(view.audienceChipLabel).toBe('Public');
    expect(view.threadTopicId).toBe(entry.scope_id);
    expect(view.canReply).toBe(true);
    expect(view.canRepost).toBe(true);
    expect(view.canReact).toBe(true);
    expect(view.media).toMatchObject({ kind: null, state: 'ready', extraAttachmentCount: 0 });
    expect(view.identifierCopy).toEqual({
      postId: entry.object_id,
      authorId: entry.author_pubkey,
    });
    expect(view.reportSubjectKind).toBe('search_result');
    expect(view.allowReadOnlyReport).toBe(true);
    expect(view.provenance).toEqual({
      canonicalSource: 'unknown',
      observedVia: [
        { nodeBaseUrl: 'https://node.example', capability: 'community_index' },
      ],
      responsibleReportTargets: [],
    });
  });

  test('preserves missing local body status for a resolved bucket result', () => {
    const resolved = resolvedEntry('[blob pending]');
    if (!resolved.post) throw new Error('resolved post fixture missing');
    resolved.post.content_status = 'Missing';
    const view = communityIndexPostCardView({
      ...entry,
      source_replica_id: 'bucket::v1::topic::72757374::1',
    }, {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved',
      resolvedEntry: resolved,
      mediaObjectUrls: {},
    });
    expect(view.post.content_status).toBe('Missing');
    expect(view.post.content).not.toContain(entry.text);
  });

  test('gates an adult-labeled canonical result until adult display is enabled', () => {
    const resolved = resolvedEntry('canonical adult content');
    if (!resolved.post) throw new Error('resolved post fixture missing');
    resolved.post.content_labels = ['adult'];

    const hidden = communityIndexPostCardView(entry, {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved',
      resolvedEntry: resolved,
      mediaObjectUrls: {},
      adultContentEnabled: false,
    });
    const visible = communityIndexPostCardView(entry, {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved',
      resolvedEntry: resolved,
      mediaObjectUrls: {},
      adultContentEnabled: true,
    });

    expect(hidden.adultContentGated).toBe(true);
    expect(visible.adultContentGated).toBe(false);
    expect(hidden.post.content).not.toContain(entry.text);
  });

  // #1052: 解決済み投稿の添付はローカルの署名済み envelope 由来であり、タイムラインと
  // 同じ builder 規則で組み立てる(node の index text からの推測補完ではない)。
  test('builds resolved attachments and media with the timeline rules', () => {
    const view = imageCardView({
      mediaObjectUrls: {
        [PRIMARY_IMAGE_HASH]: 'blob:primary-image',
        [SECOND_IMAGE_HASH]: 'blob:second-image',
      },
    });

    expect(view.post.attachments).toEqual(imageAttachments());
    expect(view.media).toMatchObject({
      objectId: entry.object_id,
      kind: 'image',
      state: 'ready',
      extraAttachmentCount: 1,
      imagePreviewSrc: 'blob:primary-image',
      metaMime: 'image/png',
    });
    expect(view.media.metaBytesLabel).toBeTruthy();
    expect(view.media.imageGalleryItems?.map((item) => item.hash)).toEqual([
      PRIMARY_IMAGE_HASH,
      SECOND_IMAGE_HASH,
    ]);
    expect(view.media.imageGalleryItems?.map((item) => item.src)).toEqual([
      'blob:primary-image',
      'blob:second-image',
    ]);
    expect(view.media.currentImageIndex).toBe(0);
  });

  test('keeps resolved media loading until its object url settles', () => {
    const view = imageCardView({ mediaObjectUrls: {} });

    expect(view.media).toMatchObject({ kind: 'image', state: 'loading' });
    expect(view.media.imagePreviewSrc).toBeNull();
  });

  test('marks resolved media unavailable once its object url settles empty', () => {
    const view = imageCardView({ mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: null } });

    expect(view.media).toMatchObject({ kind: 'image', state: 'unavailable' });
    expect(view.media.imagePreviewSrc).toBeNull();
  });

  test('gates resolved adult-labeled media without exposing a preview source', () => {
    const view = imageCardView({
      labels: ['adult'],
      adultContentEnabled: false,
      mediaObjectUrls: {
        [PRIMARY_IMAGE_HASH]: 'blob:primary-image',
        [SECOND_IMAGE_HASH]: 'blob:second-image',
      },
    });

    expect(view.adultContentGated).toBe(true);
    expect(view.media).toMatchObject({ kind: 'image', state: 'gated' });
    expect(view.media.imagePreviewSrc).toBeNull();
    expect(view.media.imageGalleryItems).toEqual([]);
  });

  test('renders resolved adult-labeled media once adult display is enabled', () => {
    const view = imageCardView({
      labels: ['adult'],
      adultContentEnabled: true,
      mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: 'blob:primary-image' },
    });

    expect(view.adultContentGated).toBe(false);
    expect(view.media).toMatchObject({
      kind: 'image',
      state: 'ready',
      imagePreviewSrc: 'blob:primary-image',
    });
  });

  test.each([
    ['loading', 'loading'],
    ['failed', 'failed'],
  ] as const)('renders no media for a %s entry', (_label, resolutionStatus) => {
    const view = communityIndexPostCardView(entry, {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus,
      resolvedEntry: null,
      mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: 'blob:primary-image' },
      locale: 'en',
    });

    expect(view.post.attachments).toEqual([]);
    expect(view.media).toMatchObject({ kind: null, extraAttachmentCount: 0 });
  });

  test.each([
    ['search', 'search_result', 'community_index'],
    ['discovery', 'search_result', 'community_index'],
    ['recommendations', 'recommendation', 'recommendation'],
  ] as const)('maps %s to its report identity', (operation, subjectKind, capability) => {
    const view = communityIndexPostCardView(entry, {
      nodeBaseUrl: 'https://node.example',
      operation,
      topicId: null,
      knownAuthor: null,
      mediaObjectUrls: {},
    });

    expect(view.reportSubjectKind).toBe(subjectKind);
    expect(view.provenance?.observedVia[0]?.capability).toBe(capability);
    expect(view.authorLabel).toBe('Unknown user');
    expect(view.authorPicture).toBeNull();
  });

  test('localizes private scope without exposing its raw identifier', () => {
    const view = communityIndexPostCardView(
      { ...entry, scope_kind: 'private_channel', scope_id: 'private-channel-secret-id' },
      {
        nodeBaseUrl: 'https://node.example',
        operation: 'search',
        topicId: null,
        knownAuthor: null,
        mediaObjectUrls: {},
      }
    );

    expect(view.audienceChipLabel).toBe('Private channel');
    expect(view.threadTopicId).toBeNull();
    expect(view.post.channel_id).toBeNull();
    expect(JSON.stringify(view)).not.toContain('private-channel-secret-id');
  });

  test('keeps a private channel result read-only until its canonical post is resolved', () => {
    const view = communityIndexPostCardView(
      { ...entry, scope_kind: 'private_channel', scope_id: 'channel-1' },
      {
        nodeBaseUrl: 'https://node.example',
        operation: 'search',
        topicId: 'kukuri:topic:rust',
        knownAuthor: null,
        mediaObjectUrls: {},
      }
    );

    expect(view.threadTopicId).toBeNull();
    expect(view.post).toMatchObject({
      published_topic_id: null,
      channel_id: null,
      is_threadable: false,
    });
    expect(view.canReply).toBe(false);
    expect(view.canRepost).toBe(false);
  });

  // #1055 / AC-1 / TR-2: 設定済み node の advisory は self-label と同じ代替表示にする。
  // 添付があるので media も gated になり、preview source を出さない。
  test('gates advisory-labeled resolved media and exposes the advisory', () => {
    const view = communityIndexPostCardView(advisoryEntry([blobAdvisory()]), {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved',
      resolvedEntry: resolvedImageEntry(),
      mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: 'blob:primary' },
      adultContentEnabled: false,
      nodeName: 'index-node.example',
      locale: 'en',
    });

    expect(view.adultContentGated).toBe(true);
    expect(view.media.state).toBe('gated');
    expect(view.media.imagePreviewSrc).toBeNull();
    expect(view.media.imageGalleryItems).toEqual([]);
    expect(view.contentAdvisory).toEqual({
      issuerNodeId: ISSUER_NODE_ID,
      nodeBaseUrl: 'https://node.example',
      nodeName: 'index-node.example',
      category: 'nsfw',
      label: 'adult',
      confidence: 84,
      basis: 'classifier_score',
      signalId: 'signal-1',
      subjectKind: 'blob_cid',
      subjectId: PRIMARY_IMAGE_HASH,
    });
    // canonical 解決済みなので待機文言の差し替えは行わない。
    expect(view.gatedBodyText).toBeNull();
  });

  // #1055 / INVAR-1: advisory を署名済み `content_labels` へ書き戻さない
  // (`content_advisories_are_separate_from_signed_content_labels`)。
  test('content_advisories_are_separate_from_signed_content_labels', () => {
    const view = communityIndexPostCardView(advisoryEntry([blobAdvisory()]), {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved',
      resolvedEntry: resolvedImageEntry(),
      mediaObjectUrls: {},
      adultContentEnabled: false,
      locale: 'en',
    });

    expect(view.post.content_labels).toEqual([]);
    expect(view.actionPost?.content_labels).toEqual([]);
    expect(view.adultContentGated).toBe(true);
  });

  // #1055 / AC-1 / TR-1: canonical 解決前でも advisory だけで代替表示にする。隠すべき本文が
  // まだ無いので、待機文言を保ったまま gated にする。
  test('gates an advisory-labeled unresolved entry and keeps the resolving notice', () => {
    const view = communityIndexPostCardView(advisoryEntry([blobAdvisory()]), {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'loading',
      resolvedEntry: null,
      mediaObjectUrls: {},
      adultContentEnabled: false,
      locale: 'en',
    });

    expect(view.adultContentGated).toBe(true);
    expect(view.gatedBodyText).toBe(i18n.t('shell:communityIndex.contentResolving'));
    // 未解決 entry は添付を持たないので、メディアそのものが描画対象にならない。
    expect(view.media.kind).toBeNull();
    expect(view.contentAdvisory?.signalId).toBe('signal-1');
  });

  // #1055 / AC-1: 表示設定 ON では advisory があっても通常表示に戻る(ephemeral fetch)。
  test('renders advisory-labeled media once adult display is enabled', () => {
    const view = communityIndexPostCardView(advisoryEntry([blobAdvisory()]), {
      nodeBaseUrl: 'https://node.example',
      operation: 'search',
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved',
      resolvedEntry: resolvedImageEntry(),
      mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: 'blob:primary' },
      adultContentEnabled: true,
      locale: 'en',
    });

    expect(view.adultContentGated).toBe(false);
    expect(view.media.state).toBe('ready');
    expect(view.media.imagePreviewSrc).toBe('blob:primary');
    expect(view.contentAdvisory).toBeNull();
  });

  // #1055: 未知ラベルの advisory ではゲートしない(前方互換。node が語彙を増やしても
  // client が勝手に隠さない)。
  test('ignores advisories with unknown labels', () => {
    const view = communityIndexPostCardView(
      advisoryEntry([blobAdvisory({ label: 'experimental-future-label' })]),
      {
        nodeBaseUrl: 'https://node.example',
        operation: 'search',
        topicId: null,
        knownAuthor,
        resolutionStatus: 'resolved',
        resolvedEntry: resolvedImageEntry(),
        mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: 'blob:primary' },
        adultContentEnabled: false,
        locale: 'en',
      }
    );

    expect(view.adultContentGated).toBe(false);
    expect(view.contentAdvisory).toBeNull();
    expect(view.media.imagePreviewSrc).toBe('blob:primary');
  });

  // #1055: 説明に出す advisory は添付そのものへの判定(`blob_cid`)を優先する。
  test('prefers a blob advisory over a post advisory for the explanation', () => {
    const view = communityIndexPostCardView(
      advisoryEntry([
        blobAdvisory({
          subject_kind: 'post_id',
          subject_id: entry.object_id,
          category: 'objectionable',
          label: 'sensitive',
          signal_id: 'signal-post',
        }),
        blobAdvisory(),
      ]),
      {
        nodeBaseUrl: 'https://node.example',
        operation: 'search',
        topicId: null,
        knownAuthor,
        resolutionStatus: 'resolved',
        resolvedEntry: resolvedImageEntry(),
        mediaObjectUrls: {},
        adultContentEnabled: false,
        locale: 'en',
      }
    );

    expect(view.contentAdvisory?.subjectKind).toBe('blob_cid');
    expect(view.contentAdvisory?.signalId).toBe('signal-1');
  });

  // #1055: `post_id` だけの advisory でも投稿カード全体を代替表示にする(ADR 0046 §6.3)。
  test('gates the whole card for a post-level advisory', () => {
    const view = communityIndexPostCardView(
      advisoryEntry([
        blobAdvisory({
          subject_kind: 'post_id',
          subject_id: entry.object_id,
          category: 'objectionable',
          label: 'sensitive',
        }),
      ]),
      {
        nodeBaseUrl: 'https://node.example',
        operation: 'search',
        topicId: null,
        knownAuthor,
        resolutionStatus: 'resolved',
        resolvedEntry: resolvedImageEntry(),
        mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: 'blob:primary' },
        adultContentEnabled: false,
        locale: 'en',
      }
    );

    expect(view.adultContentGated).toBe(true);
    expect(view.media.state).toBe('gated');
    expect(view.contentAdvisory?.label).toBe('sensitive');
  });

  // #1107 / AC-6: advisory の無い解決済み投稿でも、同じ blob が別の投稿でゲートされていれば
  // メディアだけを代替表示にする(本文は伏せない)。
  test('gates only the media of a plain post whose blob is gated elsewhere', () => {
    const options = {
      nodeBaseUrl: 'https://node.example',
      operation: 'search' as const,
      topicId: null,
      knownAuthor,
      resolutionStatus: 'resolved' as const,
      resolvedEntry: resolvedImageEntry(),
      mediaObjectUrls: { [PRIMARY_IMAGE_HASH]: 'blob:primary' },
      locale: 'en',
      gatedMediaHashes: [PRIMARY_IMAGE_HASH],
    };
    const view = communityIndexPostCardView(advisoryEntry([]), {
      ...options,
      adultContentEnabled: false,
    });

    expect(view.adultContentGated).toBe(false);
    expect(view.contentAdvisory).toBeNull();
    expect(view.media.state).toBe('gated');
    expect(view.media.gatedBy).toBe('shared_media');
    expect(view.media.imagePreviewSrc).toBeNull();
    expect(view.media.imageGalleryItems).toEqual([]);
  });
});

import { renderHook } from '@testing-library/react';
import { describe, expect, test } from 'vitest';

import type { AttachmentView, PostView } from '@/lib/api';

import type {
  TimelineAdvisoryLookupState,
  TimelineContentAdvisoryIndex,
} from '@/shell/contentAdvisories';

import { usePreviewableMediaAttachments } from './usePreviewableMediaAttachments';

const INDEX_IMAGE_HASH = 'a'.repeat(64);
const TIMELINE_IMAGE_HASH = 'b'.repeat(64);

function imagePost(objectId: string, hash: string, contentLabels: string[] = []): PostView {
  const attachment: AttachmentView = {
    hash,
    mime: 'image/png',
    bytes: 2048,
    role: 'image_original',
    status: 'Available',
  };
  return {
    object_id: objectId,
    envelope_id: `envelope-${objectId}`,
    author_pubkey: 'f'.repeat(64),
    author_name: null,
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: 'caption',
    content_status: 'Available',
    content_labels: contentLabels,
    attachments: [attachment],
    created_at: 1,
    reply_to: null,
    root_id: objectId,
    channel_id: null,
    audience_label: 'Public',
  } as PostView;
}

function renderAttachments(
  overrides: {
    communityIndexResolvedPosts?: PostView[];
    activeTimeline?: PostView[];
    profileTimeline?: PostView[];
    gatedMediaHashes?: string[];
    demandedMediaHashes?: ReadonlySet<string>;
    adultContentEnabled?: boolean;
    timelineContentAdvisories?: TimelineContentAdvisoryIndex;
    timelineAdvisoryLookup?: TimelineAdvisoryLookupState;
  } = {}
) {
  const { result } = renderHook(() =>
    usePreviewableMediaAttachments({
      activeTimeline: overrides.activeTimeline ?? [],
      activePublicTimeline: [],
      gatedMediaHashes: overrides.gatedMediaHashes ?? [],
      demandedMediaHashes: overrides.demandedMediaHashes ?? new Set([INDEX_IMAGE_HASH, TIMELINE_IMAGE_HASH]),
      timelineContentAdvisories: overrides.timelineContentAdvisories,
      timelineAdvisoryLookup: overrides.timelineAdvisoryLookup,
      communityIndexResolvedPosts: overrides.communityIndexResolvedPosts ?? [],
      profileTimeline: overrides.profileTimeline ?? [],
      selectedAuthorTimeline: [],
      thread: [],
      selectedDirectMessageTimeline: [],
      directMessages: [],
      ownedReactionAssets: [],
      bookmarkedReactionAssets: [],
      recentReactions: [],
      localProfile: null,
      knownAuthorsByPubkey: {},
      notifications: [],
      adultContentEnabled: overrides.adultContentEnabled ?? false,
    })
  );
  return result.current.map((attachment) => attachment.hash);
}

describe('usePreviewableMediaAttachments', () => {
  test('fetches post image, poster, and video only while their card has demand', () => {
    const videoHash = 'c'.repeat(64);
    const posterHash = 'd'.repeat(64);
    const post = imagePost('video-post', posterHash);
    post.attachments = [
      { hash: videoHash, mime: 'video/mp4', bytes: 256 * 1024 * 1024, role: 'video_manifest', status: 'Available' },
      { hash: posterHash, mime: 'image/png', bytes: 2048, role: 'video_poster', status: 'Available' },
    ];
    expect(renderAttachments({ activeTimeline: [post] })).toEqual([]);
    expect(renderAttachments({ activeTimeline: [post], demandedMediaHashes: new Set([posterHash]) }))
      .toEqual([posterHash]);
    expect(renderAttachments({ activeTimeline: [post], demandedMediaHashes: new Set([posterHash, videoHash]) }))
      .toEqual([posterHash, videoHash]);
    expect(renderAttachments({ activeTimeline: [imagePost('image-post', INDEX_IMAGE_HASH)], demandedMediaHashes: new Set() }))
      .toEqual([]);
  });
  // #1052: 「見つける」の解決済み投稿もタイムラインと同じプリフェッチ対象にする。
  test('includes attachments from resolved community index posts', () => {
    expect(
      renderAttachments({
        communityIndexResolvedPosts: [imagePost('index-post', INDEX_IMAGE_HASH)],
        activeTimeline: [imagePost('timeline-post', TIMELINE_IMAGE_HASH)],
      })
    ).toEqual(expect.arrayContaining([INDEX_IMAGE_HASH, TIMELINE_IMAGE_HASH]));
  });

  // #858 / ADR 0046: 表示設定 OFF の間は、新しい経路でも成人向け添付を取得対象にしない。
  test('excludes adult-labeled community index attachments while adult display is off', () => {
    expect(
      renderAttachments({
        communityIndexResolvedPosts: [imagePost('index-post', INDEX_IMAGE_HASH, ['adult'])],
      })
    ).not.toContain(INDEX_IMAGE_HASH);
  });

  test('includes adult-labeled community index attachments once adult display is on', () => {
    expect(
      renderAttachments({
        communityIndexResolvedPosts: [imagePost('index-post', INDEX_IMAGE_HASH, ['adult'])],
        adultContentEnabled: true,
      })
    ).toContain(INDEX_IMAGE_HASH);
  });

  // #1055 / ADR 0046 §6.2: advisory でゲート中の hash は、どの表示経路から現れても
  // プリフェッチ対象に入れない(advisory は `PostView` に現れないため hash 単位で止める)。
  test('excludes advisory-gated hashes regardless of which surface carries them', () => {
    const hashes = renderAttachments({
      communityIndexResolvedPosts: [imagePost('index-post', INDEX_IMAGE_HASH)],
      profileTimeline: [imagePost('profile-post', INDEX_IMAGE_HASH)],
      activeTimeline: [imagePost('timeline-post', TIMELINE_IMAGE_HASH)],
      gatedMediaHashes: [INDEX_IMAGE_HASH],
    });
    expect(hashes).not.toContain(INDEX_IMAGE_HASH);
    expect(hashes).toContain(TIMELINE_IMAGE_HASH);
  });

  // 表示設定 ON では呼出元が除外集合を空にするため、通常どおり取得対象になる。
  test('includes previously gated hashes once the caller clears the gate set', () => {
    expect(
      renderAttachments({
        communityIndexResolvedPosts: [imagePost('index-post', INDEX_IMAGE_HASH)],
        gatedMediaHashes: [],
        adultContentEnabled: true,
      })
    ).toContain(INDEX_IMAGE_HASH);
  });

  // #1056: 照会中の投稿の添付は表示設定にかかわらず取得しない。「見つける」の投稿は対象外。
  test('holds timeline attachments while the advisory lookup is pending', () => {
    const hashes = renderAttachments({
      activeTimeline: [imagePost('timeline-post', TIMELINE_IMAGE_HASH)],
      communityIndexResolvedPosts: [imagePost('index-post', INDEX_IMAGE_HASH)],
      timelineAdvisoryLookup: { active: true, settled: {} },
      adultContentEnabled: true,
    });
    expect(hashes).not.toContain(TIMELINE_IMAGE_HASH);
    expect(hashes).toContain(INDEX_IMAGE_HASH);
  });

  test('releases settled attachments and keeps advisory ones gated while display is off', () => {
    const settled: Record<string, true> = {
      'post_id:timeline-post': true,
      [`blob_cid:${TIMELINE_IMAGE_HASH}`]: true,
      'post_id:other-post': true,
      [`blob_cid:${INDEX_IMAGE_HASH}`]: true,
    };
    const advisories: TimelineContentAdvisoryIndex = {
      [`blob_cid:${TIMELINE_IMAGE_HASH}`]: [
        {
          nodeBaseUrl: 'https://node.example',
          advisory: {
            issuer_node_id: 'd'.repeat(64),
            subject_kind: 'blob_cid',
            subject_id: TIMELINE_IMAGE_HASH,
            category: 'nsfw',
            label: 'adult',
            confidence: 84,
            signal_id: 'signal-1',
            basis: 'classifier_score',
          },
        },
      ],
    };
    const posts = [
      imagePost('timeline-post', TIMELINE_IMAGE_HASH),
      imagePost('other-post', INDEX_IMAGE_HASH),
    ];
    const off = renderAttachments({
      activeTimeline: posts,
      timelineContentAdvisories: advisories,
      timelineAdvisoryLookup: { active: true, settled },
    });
    expect(off).not.toContain(TIMELINE_IMAGE_HASH);
    expect(off).toContain(INDEX_IMAGE_HASH);
    const on = renderAttachments({
      activeTimeline: posts,
      timelineContentAdvisories: advisories,
      timelineAdvisoryLookup: { active: true, settled },
      adultContentEnabled: true,
    });
    expect(on).toContain(TIMELINE_IMAGE_HASH);
  });
});

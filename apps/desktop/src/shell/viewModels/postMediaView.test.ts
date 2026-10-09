import { describe, expect, test } from 'vitest';

import type { AttachmentView, PostView } from '@/lib/api';

import { buildPostMediaView } from './postMediaView';

const IMAGE_HASH = 'a'.repeat(64);
const SECOND_IMAGE_HASH = 'b'.repeat(64);
const MANIFEST_HASH = 'c'.repeat(64);
const POSTER_HASH = 'd'.repeat(64);

function post(attachments: AttachmentView[]): Pick<PostView, 'object_id' | 'attachments'> {
  return { object_id: 'post-1', attachments };
}

function attachment(overrides: Partial<AttachmentView> & { hash: string }): AttachmentView {
  return {
    mime: 'image/png',
    bytes: 2048,
    role: 'image_original',
    status: 'Available',
    ...overrides,
  };
}

function build(
  attachments: AttachmentView[],
  overrides: {
    mediaObjectUrls?: Record<string, string | null>;
    adultContentGated?: boolean;
    advisoryPending?: boolean;
    unsupportedVideoManifests?: Record<string, true>;
  } = {}
) {
  return buildPostMediaView(post(attachments), {
    mediaObjectUrls: overrides.mediaObjectUrls ?? {},
    adultContentGated: overrides.adultContentGated ?? false,
    advisoryPending: overrides.advisoryPending ?? false,
    unsupportedVideoManifests: overrides.unsupportedVideoManifests ?? {},
    locale: 'en',
  });
}

describe('buildPostMediaView', () => {
  test('reports no media kind for a post without attachments', () => {
    expect(build([])).toMatchObject({
      objectId: 'post-1',
      kind: null,
      extraAttachmentCount: 0,
      state: 'ready',
    });
  });

  test('builds an image gallery in attachment order and shows every image up to four', () => {
    const media = build(
      [attachment({ hash: IMAGE_HASH }), attachment({ hash: SECOND_IMAGE_HASH, bytes: 4096 })],
      {
        mediaObjectUrls: { [IMAGE_HASH]: 'blob:first', [SECOND_IMAGE_HASH]: 'blob:second' },
      }
    );

    expect(media).toMatchObject({
      kind: 'image',
      state: 'ready',
      extraAttachmentCount: 0,
      metaMime: 'image/png',
    });
    expect(media.imageGalleryItems?.map((item) => item.src)).toEqual(['blob:first', 'blob:second']);
  });

  // #1690: カードは先頭 4 枚を並べ、5 枚目以降を「+N」に数える。
  test('counts images after the first four as extra attachments', () => {
    const hashes = ['1', '2', '3', '4', '5'].map((digit) => digit.repeat(64));
    const media = build(hashes.map((hash) => attachment({ hash })));

    expect(media).toMatchObject({ kind: 'image', state: 'loading', extraAttachmentCount: 1 });
    expect(media.imageGalleryItems?.map((item) => item.hash)).toEqual(hashes);
  });

  test('stays loading until the object url settles and reports unavailable when it settles empty', () => {
    expect(build([attachment({ hash: IMAGE_HASH })])).toMatchObject({
      kind: 'image',
      state: 'loading',
    });
    expect(
      build([attachment({ hash: IMAGE_HASH })], { mediaObjectUrls: { [IMAGE_HASH]: null } })
    ).toMatchObject({ kind: 'image', state: 'unavailable', retryHashes: [IMAGE_HASH] });
  });

  // #1690: 並べた画像の一部の失敗は、その画像だけを失敗表示にし、枠ごとは置き換えない。
  test('replaces the whole frame only when every shown image is unavailable', () => {
    const images = [attachment({ hash: IMAGE_HASH }), attachment({ hash: SECOND_IMAGE_HASH })];
    const partial = build(images, {
      mediaObjectUrls: { [IMAGE_HASH]: null, [SECOND_IMAGE_HASH]: 'blob:second' },
    });

    expect(partial.state).toBe('ready');
    expect(partial.imageGalleryItems?.map((item) => item.failed)).toEqual([true, false]);
    expect(
      build(images, { mediaObjectUrls: { [IMAGE_HASH]: null, [SECOND_IMAGE_HASH]: null } })
    ).toMatchObject({ state: 'unavailable', retryHashes: [IMAGE_HASH, SECOND_IMAGE_HASH] });
  });

  test('gates every preview source while adult display is off', () => {
    const media = build([attachment({ hash: IMAGE_HASH })], {
      adultContentGated: true,
      mediaObjectUrls: { [IMAGE_HASH]: 'blob:first' },
    });

    expect(media).toMatchObject({ kind: 'image', state: 'gated' });
    expect(media.imageGalleryItems).toEqual([]);
  });

  test('prefers the video manifest for playback, meta and report identity', () => {
    const attachments = [
      attachment({ hash: MANIFEST_HASH, mime: 'video/mp4', role: 'video_manifest', bytes: 8192 }),
      attachment({ hash: POSTER_HASH, mime: 'image/jpeg', role: 'video_poster', bytes: 1024 }),
    ];

    expect(
      build(attachments, { mediaObjectUrls: { [MANIFEST_HASH]: 'blob:video' } })
    ).toMatchObject({
      kind: 'video',
      state: 'ready',
      videoPlaybackSrc: 'blob:video',
      videoReportHash: MANIFEST_HASH,
      metaMime: 'video/mp4',
      extraAttachmentCount: 0,
    });
    expect(
      build(attachments, {
        mediaObjectUrls: { [MANIFEST_HASH]: 'blob:video' },
        unsupportedVideoManifests: { [MANIFEST_HASH]: true },
      }).videoUnsupportedOnClient
    ).toBe(true);
  });

  test('keeps a video poster out of the image gallery', () => {
    const media = build(
      [
        attachment({ hash: MANIFEST_HASH, mime: 'video/mp4', role: 'video_manifest' }),
        attachment({ hash: POSTER_HASH, mime: 'image/jpeg', role: 'video_poster' }),
      ],
      { mediaObjectUrls: { [POSTER_HASH]: 'blob:poster' } }
    );

    expect(media.imageGalleryItems).toEqual([]);
    expect(media).toMatchObject({ kind: 'video', state: 'ready', videoPosterPreviewSrc: 'blob:poster' });
  });
  // #1056: 照会中は取得済み object URL があっても参照せず、代替表示とは別の pending にする。
  test('hides fetched media while a content advisory lookup is pending', () => {
    const media = build([attachment({ hash: IMAGE_HASH })], {
      mediaObjectUrls: { [IMAGE_HASH]: 'blob:first' },
      advisoryPending: true,
    });
    expect(media).toMatchObject({ kind: 'image', state: 'pending' });
    expect(media.imageGalleryItems).toEqual([]);
    expect(
      build([attachment({ hash: IMAGE_HASH })], { advisoryPending: true, adultContentGated: true })
        .state
    ).toBe('gated');
    expect(build([], { advisoryPending: true }).state).toBe('ready');
  });
});

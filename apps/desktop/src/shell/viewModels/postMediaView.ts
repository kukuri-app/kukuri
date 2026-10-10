import type { PostMediaView } from '@/components/core/types';
import type { AttachmentView, PostView } from '@/lib/api';
import { contentProvenanceFromView } from '@/lib/api/provenance';
import {
  POST_CARD_IMAGE_LIMIT,
  selectPostImages,
  selectVideoManifest,
  selectVideoPoster,
} from '@/shell/media';
import { formatBytes } from '@/shell/presentation';

/// 投稿カードのメディア表示データ。タイムライン・スレッドと「見つける」の解決済み投稿が
/// 同じ規則で組み立てるために共有する(#1052)。再生診断の `videoProps` は、イベントを
/// 購読する呼出元(タイムライン)が組み立てた結果へ付与する。
export type BuildPostMediaViewOptions = {
  mediaObjectUrls: Record<string, string | null>;
  /// #1207: 失敗が確定した hash のうち再取得を試行中のもの。
  mediaRetryingHashes?: Record<string, true>;
  /// #858: 表示許可前の成人向けラベル付き投稿。取得済み object URL があっても参照しない。
  adultContentGated: boolean;
  /// #1055: ゲートの判定元。Community Node の advisory 由来なら代替表示の文言を変える。
  /// #1107: `shared_media` は同じ blob が別の投稿でゲートされているため、メディアだけを伏せる。
  gatedBy?: 'self_label' | 'advisory' | 'shared_media';
  /// #1056: Community Node への advisory 照会が未決。取得済み object URL があっても参照せず、
  /// スケルトン(`pending`)にする。確定後の代替表示(`gated`)とは表示を分ける。
  advisoryPending?: boolean;
  unsupportedVideoManifests: Record<string, true>;
  locale?: string | null;
};

export function buildPostMediaView(
  post: Pick<PostView, 'object_id' | 'attachments'>,
  {
    mediaObjectUrls,
    mediaRetryingHashes = {},
    adultContentGated,
    gatedBy,
    advisoryPending = false,
    unsupportedVideoManifests,
    locale,
  }: BuildPostMediaViewOptions
): PostMediaView {
  const images = selectPostImages(post);
  // #1690: カードには先頭 4 枚を並べる。
  const cardImages = images.slice(0, POST_CARD_IMAGE_LIMIT);
  const videoPoster = selectVideoPoster(post);
  const videoManifest = selectVideoManifest(post);
  const imageGalleryItems = images.map((attachment) => ({
    hash: attachment.hash,
    src:
      typeof mediaObjectUrls[attachment.hash] === 'string'
        ? mediaObjectUrls[attachment.hash]
        : null,
    failed: mediaObjectUrls[attachment.hash] === null,
    retrying: mediaRetryingHashes[attachment.hash] === true,
    mime: attachment.mime,
    provenance: contentProvenanceFromView(attachment.provenance),
  }));
  const mediaKind = cardImages.length > 0 ? 'image' : videoManifest || videoPoster ? 'video' : null;
  // 照会中は取得済みでも表示しない(ゲートと同じく参照を止める)。
  const hidden = adultContentGated || advisoryPending;
  const mediaMetaAttachment =
    mediaKind === 'video' ? videoManifest ?? videoPoster : cardImages[0] ?? null;
  // カードに出す添付。全てが取得不可で確定したときだけ枠ごと失敗表示にする。
  const shownAttachments = (mediaKind === 'image' ? cardImages : [videoManifest, videoPoster])
    .filter((attachment): attachment is AttachmentView => attachment !== null);
  const reservedHashes = new Set(
    [...cardImages, videoPoster, videoManifest].map((attachment) => attachment?.hash)
  );
  const extraAttachmentCount = post.attachments.filter(
    (attachment) => !reservedHashes.has(attachment.hash)
  ).length;
  const sourceOf = (attachment: AttachmentView | null) =>
    !hidden && attachment && typeof mediaObjectUrls[attachment.hash] === 'string'
      ? mediaObjectUrls[attachment.hash]
      : null;
  const videoPosterPreviewSrc = sourceOf(videoPoster);
  const videoPlaybackSrc = sourceOf(videoManifest);
  const mediaReady =
    mediaKind === 'video'
      ? Boolean(videoPlaybackSrc || videoPosterPreviewSrc)
      : cardImages.some((attachment) => sourceOf(attachment) !== null);
  const hasSettledUnavailable = (hash: string) =>
    Object.prototype.hasOwnProperty.call(mediaObjectUrls, hash) &&
    mediaObjectUrls[hash] === null;
  const mediaUnavailable =
    shownAttachments.length > 0 &&
    shownAttachments.every((attachment) => hasSettledUnavailable(attachment.hash));
  // #1207: 明示再試行の対象。表示に使う hash のうち、取得不可が確定したものだけを取り直す。
  const retryHashes = hidden
    ? []
    : shownAttachments.map((attachment) => attachment.hash).filter(hasSettledUnavailable);
  const videoUnsupportedOnClient = Boolean(
    videoManifest && unsupportedVideoManifests[videoManifest.hash]
  );

  return {
    objectId: post.object_id,
    kind: mediaKind,
    extraAttachmentCount,
    gatedBy: adultContentGated && mediaKind !== null ? gatedBy ?? 'self_label' : undefined,
    state:
      mediaKind === null
        ? 'ready'
        : adultContentGated
          ? 'gated'
          : advisoryPending
            ? 'pending'
            : mediaReady
              ? 'ready'
              : mediaUnavailable
                ? 'unavailable'
                : 'loading',
    metaMime: mediaMetaAttachment?.mime ?? null,
    metaBytesLabel: mediaMetaAttachment
      ? formatBytes(mediaMetaAttachment.bytes, locale)
      : null,
    imageGalleryItems: hidden ? [] : imageGalleryItems,
    videoPosterPreviewSrc,
    videoPosterHash: videoPoster?.hash ?? null,
    videoPlaybackSrc,
    videoReportHash:
      mediaKind === 'video' ? (videoManifest?.hash ?? videoPoster?.hash ?? null) : null,
    videoUnsupportedOnClient,
    retryHashes,
    retrying: retryHashes.some((hash) => mediaRetryingHashes[hash] === true),
    provenance: contentProvenanceFromView(mediaMetaAttachment?.provenance),
  };
}

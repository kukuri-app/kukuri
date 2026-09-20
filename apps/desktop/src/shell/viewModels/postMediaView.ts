import type { PostMediaView } from '@/components/core/types';
import type { AttachmentView, PostView } from '@/lib/api';
import { contentProvenanceFromView } from '@/lib/api/provenance';
import { selectPrimaryImage, selectVideoManifest, selectVideoPoster } from '@/shell/media';
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
  const primaryImage = selectPrimaryImage(post);
  const videoPoster = selectVideoPoster(post);
  const videoManifest = selectVideoManifest(post);
  const imageGalleryItems = post.attachments
    .filter(
      (attachment) =>
        attachment.mime.startsWith('image/') && attachment.role !== 'video_poster'
    )
    .map((attachment) => ({
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
  const mediaKind = primaryImage ? 'image' : videoManifest || videoPoster ? 'video' : null;
  // 照会中は取得済みでも表示しない(ゲートと同じく参照を止める)。
  const hidden = adultContentGated || advisoryPending;
  const mediaMetaAttachment =
    mediaKind === 'video' ? videoManifest ?? videoPoster : primaryImage;
  const reservedHashes = new Set<string>();
  if (primaryImage) reservedHashes.add(primaryImage.hash);
  if (videoPoster) reservedHashes.add(videoPoster.hash);
  if (videoManifest) reservedHashes.add(videoManifest.hash);
  const extraAttachmentCount = post.attachments.filter(
    (attachment) => !reservedHashes.has(attachment.hash)
  ).length;
  const imagePreviewSrc =
    !hidden && primaryImage && typeof mediaObjectUrls[primaryImage.hash] === 'string'
      ? mediaObjectUrls[primaryImage.hash]
      : null;
  const videoPosterPreviewSrc =
    !hidden && videoPoster && typeof mediaObjectUrls[videoPoster.hash] === 'string'
      ? mediaObjectUrls[videoPoster.hash]
      : null;
  const videoPlaybackSrc =
    !hidden &&
    videoManifest &&
    typeof mediaObjectUrls[videoManifest.hash] === 'string'
      ? mediaObjectUrls[videoManifest.hash]
      : null;
  const hasSettledUnavailable = (hash: string) =>
    Object.prototype.hasOwnProperty.call(mediaObjectUrls, hash) &&
    mediaObjectUrls[hash] === null;
  const mediaUnavailable =
    mediaKind === 'image'
      ? Boolean(primaryImage && hasSettledUnavailable(primaryImage.hash))
      : mediaKind === 'video'
        ? [videoManifest, videoPoster]
            .filter((attachment): attachment is AttachmentView => attachment !== null)
            .every((attachment) => hasSettledUnavailable(attachment.hash))
        : false;
  // #1207: 明示再試行の対象。表示に使う hash のうち、取得不可が確定したものだけを取り直す。
  const retryHashes = hidden
    ? []
    : (mediaKind === 'image' ? [primaryImage] : mediaKind === 'video' ? [videoManifest, videoPoster] : [])
        .filter((attachment): attachment is AttachmentView => attachment !== null)
        .map((attachment) => attachment.hash)
        .filter((hash) => hasSettledUnavailable(hash));
  const videoUnsupportedOnClient = Boolean(
    videoManifest && unsupportedVideoManifests[videoManifest.hash]
  );

  return {
    objectId: post.object_id,
    kind: mediaKind,
    extraAttachmentCount,
    gatedBy: adultContentGated && mediaKind !== null ? gatedBy ?? 'self_label' : undefined,
    state:
      adultContentGated && mediaKind !== null
        ? 'gated'
        : advisoryPending && mediaKind !== null
          ? 'pending'
          : mediaKind === 'video'
          ? videoPlaybackSrc || videoPosterPreviewSrc
            ? 'ready'
            : mediaUnavailable
              ? 'unavailable'
              : 'loading'
          : mediaKind === 'image'
            ? imagePreviewSrc
              ? 'ready'
              : mediaUnavailable
                ? 'unavailable'
                : 'loading'
            : 'ready',
    metaMime: mediaMetaAttachment?.mime ?? null,
    metaBytesLabel: mediaMetaAttachment
      ? formatBytes(mediaMetaAttachment.bytes, locale)
      : null,
    imagePreviewSrc,
    imageGalleryItems: hidden ? [] : imageGalleryItems,
    currentImageIndex: primaryImage
      ? Math.max(
          0,
          imageGalleryItems.findIndex((item) => item.hash === primaryImage.hash)
        )
      : 0,
    videoPosterPreviewSrc,
    videoPlaybackSrc,
    videoReportHash:
      mediaKind === 'video' ? (videoManifest?.hash ?? videoPoster?.hash ?? null) : null,
    videoUnsupportedOnClient,
    retryHashes,
    retrying: retryHashes.some((hash) => mediaRetryingHashes[hash] === true),
    provenance: contentProvenanceFromView(mediaMetaAttachment?.provenance),
  };
}

import type { ReactionKeyView } from '@/lib/api';

/// リアクションの絵文字か custom reaction の画像。画像を取得できていなければ custom reaction の名前の先頭 2 文字を出す(#1232)。
export function ReactionGlyph({
  reaction,
  mediaObjectUrls,
}: {
  reaction: Pick<ReactionKeyView, 'emoji' | 'custom_asset'>;
  mediaObjectUrls: Record<string, string | null>;
}) {
  const asset = reaction.custom_asset;
  const previewUrl = asset ? mediaObjectUrls[asset.blob_hash] : null;
  if (previewUrl) return <img className='post-reaction-chip-image' src={previewUrl} alt='' />;
  if (asset) return <span aria-hidden='true'>{(asset.search_key.trim() || asset.asset_id).slice(0, 2)}</span>;
  return <span>{reaction.emoji ?? '?'}</span>;
}

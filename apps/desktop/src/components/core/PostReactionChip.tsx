import type { ReactionSummaryView } from '@/lib/api';
import {
  contextActionMenuPositionFromKeyboard,
  contextActionMenuPositionFromPointer,
  type ContextActionMenuPosition,
} from '@/components/ui/context-action-menu';

import { ReactionGlyph } from './ReactionGlyph';
import { ReactionTooltipButton } from './ReactionTooltipButton';

type PostReactionChipProps = {
  reaction: ReactionSummaryView;
  active: boolean;
  mediaObjectUrls: Record<string, string | null>;
  onToggle?: () => void;
  onOpenContextMenu?: (position: ContextActionMenuPosition) => void;
};

export function PostReactionChip({
  reaction,
  active,
  mediaObjectUrls,
  onToggle,
  onOpenContextMenu,
}: PostReactionChipProps) {
  const customReactionLabel = reaction.custom_asset
    ? reaction.custom_asset.search_key.trim() || reaction.custom_asset.asset_id
    : null;
  const chip = (
    <button
      className={`post-reaction-chip${active ? ' post-reaction-chip-active' : ''}`}
      type='button'
      aria-label={customReactionLabel ? `${customReactionLabel} ${reaction.count}` : undefined}
      data-tooltip={customReactionLabel ?? undefined}
      onClick={onToggle}
      onContextMenu={(event) => {
        if (!onOpenContextMenu) return;
        const position = contextActionMenuPositionFromPointer(event);
        if (position) onOpenContextMenu(position);
      }}
      onKeyDown={(event) => {
        if (!onOpenContextMenu) return;
        const position = contextActionMenuPositionFromKeyboard(event);
        if (position) onOpenContextMenu(position);
      }}
    >
      <ReactionGlyph reaction={reaction} mediaObjectUrls={mediaObjectUrls} />
      <span>{reaction.count}</span>
    </button>
  );

  return (
    <span className='post-reaction-chip-wrap'>
      {customReactionLabel ? (
        <ReactionTooltipButton label={customReactionLabel}>{chip}</ReactionTooltipButton>
      ) : (
        chip
      )}
    </span>
  );
}

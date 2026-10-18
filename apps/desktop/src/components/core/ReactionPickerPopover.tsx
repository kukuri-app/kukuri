import { useEffect, useMemo, useState, type CSSProperties } from 'react';

import { Search, SmilePlus } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { IconButton } from '@/components/ui/icon-button';
import { Input } from '@/components/ui/input';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import type {
  CustomReactionAssetView,
  ReactionKeyInput,
  ReactionKeyView,
  RecentReactionView,
} from '@/lib/api';

import { type PostCardView } from './types';
import { ReactionTooltipButton } from './ReactionTooltipButton';

const CURATED_EMOJI_OPTIONS = [
  { emoji: '👍', key: 'thumbs-up' },
  { emoji: '❤️', key: 'heart' },
  { emoji: '😂', key: 'laugh' },
  { emoji: '🔥', key: 'fire' },
  { emoji: '👏', key: 'clap' },
  { emoji: '🎉', key: 'party-popper' },
  { emoji: '😮', key: 'surprised' },
  { emoji: '😭', key: 'sob' },
  { emoji: '👀', key: 'eyes' },
  { emoji: '🙌', key: 'raised-hands' },
  { emoji: '🤝', key: 'handshake' },
  { emoji: '✨', key: 'sparkles' },
] as const;

const REACTION_PICKER_MAX_COLUMNS = 8;
const REACTION_PICKER_MIN_COLUMNS = 1;
const REACTION_PICKER_MIN_CELL_SIZE_PX = 32;
const REACTION_PICKER_TARGET_CELL_SIZE_PX = 44;
const REACTION_PICKER_GRID_GAP_PX = 6;
const REACTION_PICKER_INLINE_PADDING_PX = 16;
const REACTION_PICKER_VIEWPORT_GUTTER_PX = 16;

type ReactionPopoverLayout = {
  columns: number;
  width: number;
};

function getReactionPickerViewportWidth(): number {
  if (typeof window === 'undefined') {
    return 1024;
  }
  return Math.round(window.visualViewport?.width ?? window.innerWidth);
}

function computeReactionPopoverLayout(viewportWidth: number): ReactionPopoverLayout {
  const availableWidth = Math.max(
    REACTION_PICKER_MIN_CELL_SIZE_PX + REACTION_PICKER_INLINE_PADDING_PX,
    viewportWidth - REACTION_PICKER_VIEWPORT_GUTTER_PX * 2
  );
  const maxColumnsThatFit = Math.floor(
    (availableWidth - REACTION_PICKER_INLINE_PADDING_PX + REACTION_PICKER_GRID_GAP_PX) /
      (REACTION_PICKER_MIN_CELL_SIZE_PX + REACTION_PICKER_GRID_GAP_PX)
  );
  const columns = Math.max(
    REACTION_PICKER_MIN_COLUMNS,
    Math.min(REACTION_PICKER_MAX_COLUMNS, maxColumnsThatFit)
  );
  const preferredWidth =
    columns * REACTION_PICKER_TARGET_CELL_SIZE_PX +
    (columns - 1) * REACTION_PICKER_GRID_GAP_PX +
    REACTION_PICKER_INLINE_PADDING_PX;

  return {
    columns,
    width: Math.round(Math.min(availableWidth, preferredWidth)),
  };
}

type ReactionPickerPopoverProps = {
  post: PostCardView['post'];
  // 開く操作の名前(既定は「リアクション」)。
  label?: string;
  recentReactions?: RecentReactionView[];
  assets?: CustomReactionAssetView[];
  mediaObjectUrls?: Record<string, string | null>;
  onToggleReaction?: (post: PostCardView['post'], reactionKey: ReactionKeyInput) => void;
  onOpen?: () => void;
};

function reactionKeyInputFromView(reaction: ReactionKeyView): ReactionKeyInput | null {
  if (reaction.reaction_key_kind === 'emoji' && reaction.emoji?.trim()) {
    return { kind: 'emoji', emoji: reaction.emoji };
  }
  if (reaction.reaction_key_kind === 'custom_asset' && reaction.custom_asset) {
    return { kind: 'custom_asset', asset: reaction.custom_asset };
  }
  return null;
}

function assetSearchLabel(asset: CustomReactionAssetView): string {
  return asset.search_key.trim() || asset.asset_id;
}

function emojiSearchLabel(emoji: string): string {
  return CURATED_EMOJI_OPTIONS.find((option) => option.emoji === emoji)?.key ?? emoji;
}

export function ReactionPickerPopover({
  post,
  label,
  recentReactions = [],
  assets = [],
  mediaObjectUrls = {},
  onToggleReaction,
  onOpen,
}: ReactionPickerPopoverProps) {
  const { t } = useTranslation('common');
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [layout, setLayout] = useState<ReactionPopoverLayout>(() =>
    computeReactionPopoverLayout(getReactionPickerViewportWidth())
  );
  const normalizedQuery = query.trim().toLowerCase();

  const filteredRecentReactions = useMemo(
    () =>
      recentReactions.filter((reaction) => {
        if (!normalizedQuery) {
          return true;
        }
        const label = reaction.emoji
          ? emojiSearchLabel(reaction.emoji).toLowerCase()
          : (reaction.custom_asset?.search_key ?? reaction.normalized_reaction_key).toLowerCase();
        const normalizedReactionKey = reaction.normalized_reaction_key.toLowerCase();
        return (
          label.includes(normalizedQuery) ||
          normalizedReactionKey.includes(normalizedQuery) ||
          reaction.emoji?.includes(normalizedQuery) === true
        );
      }),
    [normalizedQuery, recentReactions]
  );

  const filteredEmojiReactions = useMemo(
    () =>
      CURATED_EMOJI_OPTIONS.filter((option) =>
        normalizedQuery
          ? option.key.includes(normalizedQuery) || option.emoji.includes(normalizedQuery)
          : true
      ),
    [normalizedQuery]
  );

  const filteredAssets = useMemo(
    () =>
      assets.filter((asset) => {
        if (!normalizedQuery) {
          return true;
        }
        return [asset.search_key, asset.asset_id].some((value) =>
          value.toLowerCase().includes(normalizedQuery)
        );
      }),
    [assets, normalizedQuery]
  );

  const handleSelect = (reactionKey: ReactionKeyInput) => {
    onToggleReaction?.(post, reactionKey);
    setOpen(false);
    setQuery('');
  };

  useEffect(() => {
    if (!open) {
      return undefined;
    }

    const updateLayout = () => {
      setLayout(computeReactionPopoverLayout(getReactionPickerViewportWidth()));
    };

    updateLayout();
    window.addEventListener('resize', updateLayout);
    window.visualViewport?.addEventListener('resize', updateLayout);

    return () => {
      window.removeEventListener('resize', updateLayout);
      window.visualViewport?.removeEventListener('resize', updateLayout);
    };
  }, [open]);

  const popoverStyle = useMemo(
    () =>
      ({
        width: `${layout.width}px`,
        '--reaction-grid-columns': String(layout.columns),
      }) as CSSProperties,
    [layout.columns, layout.width]
  );

  return (
    <Popover
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (nextOpen) {
          onOpen?.();
        } else {
          setQuery('');
        }
      }}
    >
      <PopoverTrigger asChild>
        <IconButton
          variant='secondary'
          className='post-action-button'
          type='button'
          label={label ?? t('actions.react')}
          disabled={!onToggleReaction}
        >
          <SmilePlus className='size-4' aria-hidden='true' />
        </IconButton>
      </PopoverTrigger>
      <PopoverContent
        align='end'
        className='post-reaction-popover post-reaction-popover-wide'
        style={popoverStyle}
      >
        <div className='post-reaction-search'>
          <Search className='post-reaction-search-icon size-4' aria-hidden='true' />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t('reactions.searchPlaceholder')}
            aria-label={t('reactions.searchPlaceholder')}
          />
        </div>

        {filteredRecentReactions.length > 0 ? (
          <section className='post-reaction-section'>
            <p className='post-reaction-section-title'>{t('reactions.recent')}</p>
            <div className='post-reaction-picker-grid post-reaction-picker-grid-compact post-reaction-picker-grid-8'>
              {filteredRecentReactions.map((reaction) => {
                const reactionKey = reactionKeyInputFromView(reaction);
                const asset = reaction.custom_asset ?? null;
                const previewUrl =
                  asset && typeof mediaObjectUrls[asset.blob_hash] === 'string'
                    ? mediaObjectUrls[asset.blob_hash]
                    : null;
                const label = asset
                  ? assetSearchLabel(asset)
                  : emojiSearchLabel(reaction.emoji ?? reaction.normalized_reaction_key);
                return (
                  <ReactionTooltipButton key={reaction.normalized_reaction_key} label={label}>
                    <button
                      className='post-reaction-picker-button post-reaction-picker-button-compact post-reaction-tooltip-anchor'
                      type='button'
                      onClick={() => {
                        if (reactionKey) {
                          handleSelect(reactionKey);
                        }
                      }}
                      disabled={!reactionKey}
                      aria-label={label}
                      data-tooltip={label}
                    >
                      {previewUrl ? (
                        <img className='post-reaction-chip-image' src={previewUrl} alt='' />
                      ) : (
                        <span className='post-reaction-picker-label'>{reaction.emoji ?? label}</span>
                      )}
                    </button>
                  </ReactionTooltipButton>
                );
              })}
            </div>
          </section>
        ) : null}

        <section className='post-reaction-section'>
          <p className='post-reaction-section-title'>{t('reactions.emoji')}</p>
          <div className='post-reaction-picker-grid post-reaction-picker-grid-compact post-reaction-picker-grid-8'>
            {filteredEmojiReactions.map((option) => (
              <ReactionTooltipButton key={option.emoji} label={option.key}>
                <button
                  className='post-reaction-picker-button post-reaction-picker-button-compact post-reaction-tooltip-anchor'
                  type='button'
                  aria-label={option.key}
                  data-tooltip={option.key}
                  onClick={() => handleSelect({ kind: 'emoji', emoji: option.emoji })}
                >
                  <span className='post-reaction-picker-label' aria-hidden='true'>
                    {option.emoji}
                  </span>
                </button>
              </ReactionTooltipButton>
            ))}
          </div>
        </section>

        <section className='post-reaction-section'>
          <p className='post-reaction-section-title'>{t('reactions.custom')}</p>
          <div className='post-reaction-picker-grid post-reaction-picker-grid-compact post-reaction-picker-grid-8'>
            {filteredAssets.map((asset) => {
              const previewUrl =
                typeof mediaObjectUrls[asset.blob_hash] === 'string'
                  ? mediaObjectUrls[asset.blob_hash]
                  : null;
              const label = assetSearchLabel(asset);
              return (
                <ReactionTooltipButton key={asset.asset_id} label={label}>
                  <button
                    className='post-reaction-picker-button post-reaction-picker-button-compact post-reaction-tooltip-anchor'
                    type='button'
                    onClick={() => handleSelect({ kind: 'custom_asset', asset })}
                    aria-label={label}
                    data-tooltip={label}
                  >
                    {previewUrl ? (
                      <img className='post-reaction-chip-image' src={previewUrl} alt='' />
                    ) : (
                      <span className='post-reaction-picker-fallback'>{label.slice(0, 2)}</span>
                    )}
                  </button>
                </ReactionTooltipButton>
              );
            })}
          </div>
          {filteredAssets.length === 0 ? (
            <p className='post-reaction-picker-empty'>{t('reactions.noResults')}</p>
          ) : null}
        </section>
      </PopoverContent>
    </Popover>
  );
}

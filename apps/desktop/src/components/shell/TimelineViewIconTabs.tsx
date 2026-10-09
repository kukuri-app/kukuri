import { Bookmark, Check, Funnel, List, LoaderPinwheel, UserRoundArrowLeft, UsersRound } from 'lucide-react';
import { useRef, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Popover, PopoverMenuContent, PopoverTrigger } from '@/components/ui/popover';
import type { ColumnTimelineFilter } from '@/shell/slices/workspace';

export type TimelineViewId = 'feed' | 'bookmarks';

const FILTERS = [undefined, 'mutual', 'following'] as const;
// 関係のアイコンは投稿カードの関係の表示(PostMetaIcons)と同じものを使う。
const FILTER_ICONS = { none: Funnel, mutual: UsersRound, following: UserRoundArrowLeft };

type TimelineViewIconTabsProps = {
  activeView: TimelineViewId;
  items: Array<{ id: TimelineViewId; label: string }>;
  onSelect: (view: TimelineViewId) => void;
  // 新着を常に反映する Flow モード(#1647)。表示中のフィードを押したときだけ切り替える。
  flow?: boolean;
  onToggleFlow?: () => void;
  // フィードの絞り込み(#1689)。ボタンはフィードとブックマークの間に置き、ブックマーク表示中も選べる。
  filter?: ColumnTimelineFilter;
  onSelectFilter: (filter: ColumnTimelineFilter | undefined) => void;
};

export function TimelineViewIconTabs({
  activeView,
  items,
  onSelect,
  flow = false,
  onToggleFlow,
  filter,
  onSelectFilter,
}: TimelineViewIconTabsProps) {
  const { t } = useTranslation('shell');
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [filterMenuOpen, setFilterMenuOpen] = useState(false);
  const filterLabel = (value: ColumnTimelineFilter | undefined) =>
    t(`workspace.timelineFilter.${value ?? 'none'}`);
  const FilterIcon = FILTER_ICONS[filter ?? 'none'];

  const moveSelection = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex: number | undefined;
    if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
      nextIndex = (index + 1) % items.length;
    } else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
      nextIndex = (index - 1 + items.length) % items.length;
    } else if (event.key === 'Home') {
      nextIndex = 0;
    } else if (event.key === 'End') {
      nextIndex = items.length - 1;
    }

    if (nextIndex === undefined) return;
    event.preventDefault();
    onSelect(items[nextIndex].id);
    tabRefs.current[nextIndex]?.focus();
  };

  return (
    // tablist は箱を持たず(CSS の display: contents)、フィルターのボタンを tab の間へ並べる。
    // ボタンは tab ではないので tablist の外に置く(tablist の子は tab だけ)。
    <div className='shell-column-view-tabs'>
      <div
        className='shell-column-view-tablist'
        role='tablist'
        aria-label={t('workspace.timelineViews')}
      >
        {items.map((item, index) => {
          const active = activeView === item.id;
          const flowIcon = item.id === 'feed' && flow;
          return (
            <IconButton
              key={item.id}
              variant={active ? 'secondary' : 'ghost'}
              className='shell-column-view-tab'
              role='tab'
              type='button'
              label={flowIcon ? t('workspace.feedFlow') : item.label}
              aria-selected={active}
              tabIndex={active ? 0 : -1}
              ref={(node) => {
                tabRefs.current[index] = node;
              }}
              onClick={() => (active && item.id === 'feed' && onToggleFlow ? onToggleFlow() : onSelect(item.id))}
              onKeyDown={(event) => moveSelection(event, index)}
            >
              {flowIcon ? (
                <LoaderPinwheel className='size-4 icon-spinning' aria-hidden='true' />
              ) : item.id === 'feed' ? (
                <List className='size-4' aria-hidden='true' />
              ) : (
                <Bookmark className='size-4' aria-hidden='true' />
              )}
            </IconButton>
          );
        })}
      </div>
      <Popover open={filterMenuOpen} onOpenChange={setFilterMenuOpen}>
        <PopoverTrigger asChild>
          <IconButton
            variant='ghost'
            className='shell-column-view-tab shell-column-view-filter'
            type='button'
            label={
              filter
                ? t('workspace.timelineFilter.active', { filter: filterLabel(filter) })
                : t('workspace.timelineFilter.label')
            }
            aria-haspopup='menu'
            data-active={filter ? 'true' : undefined}
          >
            <FilterIcon className='size-4' aria-hidden='true' />
          </IconButton>
        </PopoverTrigger>
        <PopoverMenuContent aria-label={t('workspace.timelineFilter.label')} align='start' className='space-y-1'>
          {FILTERS.map((value) => {
            const Icon = FILTER_ICONS[value ?? 'none'];
            return (
              <Button
                key={value ?? 'none'}
                role='menuitemradio'
                aria-checked={filter === value}
                variant='ghost'
                className='w-full justify-start'
                onClick={() => {
                  setFilterMenuOpen(false);
                  onSelectFilter(value);
                }}
              >
                <Icon className='size-4' aria-hidden='true' />
                {filterLabel(value)}
                {filter === value ? <Check className='ml-auto size-4' aria-hidden='true' /> : null}
              </Button>
            );
          })}
        </PopoverMenuContent>
      </Popover>
    </div>
  );
}

import { Bookmark, List, LoaderPinwheel } from 'lucide-react';
import { useRef, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { IconButton } from '@/components/ui/icon-button';

export type TimelineViewId = 'feed' | 'bookmarks';

type TimelineViewIconTabsProps = {
  activeView: TimelineViewId;
  items: Array<{ id: TimelineViewId; label: string }>;
  onSelect: (view: TimelineViewId) => void;
  // 新着を常に反映する Flow モード(#1647)。表示中のフィードを押したときだけ切り替える。
  flow?: boolean;
  onToggleFlow?: () => void;
};

export function TimelineViewIconTabs({
  activeView,
  items,
  onSelect,
  flow = false,
  onToggleFlow,
}: TimelineViewIconTabsProps) {
  const { t } = useTranslation('shell');
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);

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
    <div
      className='shell-column-view-tabs'
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
  );
}

import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';

import { ColumnCanvas } from './ColumnCanvas';
import { ColumnSurface } from './ColumnSurface';
import { TimelineViewIconTabs, type TimelineViewId } from './TimelineViewIconTabs';
import type { ColumnTimelineFilter } from '@/shell/slices/workspace';

const meta = {
  title: 'Shell/ColumnCanvas',
  parameters: { layout: 'fullscreen' },
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

type StoryColumn = {
  id: string;
  pinned: boolean;
  scope: string;
  title: string;
};

const columns: StoryColumn[] = [
  { id: 'timeline', pinned: true, scope: 'Public · kukuri:topic:demo', title: 'Timeline' },
  { id: 'thread', pinned: true, scope: 'Thread · Launch planning', title: 'Thread' },
  { id: 'profile', pinned: false, scope: 'Profile · bob', title: 'Profile' },
];

function CanvasStory({ count = 1, filter }: { count?: number; filter?: ColumnTimelineFilter }) {
  const visibleColumns = columns.slice(0, count);
  const [activeColumnId, setActiveColumnId] = useState(visibleColumns[0].id);
  const [timelineView, setTimelineView] = useState<TimelineViewId>('feed');
  const [timelineFlow, setTimelineFlow] = useState(false);
  const [timelineFilter, setTimelineFilter] = useState<ColumnTimelineFilter | undefined>(filter);
  return (
    <div className='shell-phase1 min-h-screen'>
      <ColumnCanvas activeColumnId={activeColumnId} onActivateColumn={setActiveColumnId}>
        {visibleColumns.map((column, index) => (
          <ColumnSurface
            key={column.id}
            columnId={column.id}
            title={column.title}
            scopeLabel={column.scope}
            position={index + 1}
            total={visibleColumns.length}
            span={1}
            active={column.id === activeColumnId}
            pinned={column.pinned}
            headerActions={
              column.id === 'timeline' ? (
                <TimelineViewIconTabs
                  activeView={timelineView}
                  items={[
                    { id: 'feed', label: 'Feed' },
                    { id: 'bookmarks', label: 'Bookmarks' },
                  ]}
                  onSelect={setTimelineView}
                  flow={timelineFlow}
                  onToggleFlow={() => setTimelineFlow((flow) => !flow)}
                  filter={timelineFilter}
                  onSelectFilter={setTimelineFilter}
                />
              ) : undefined
            }
          >
            <div className='shell-main-stack'>
              <div className='shell-column-content'>
                <h3>{column.title} surface</h3>
                <p className='lede'>Focus this Column to make its active state explicit.</p>
                <button type='button'>Focusable action</button>
              </div>
            </div>
          </ColumnSurface>
        ))}
      </ColumnCanvas>
    </div>
  );
}

export const SingleTimeline: Story = {
  render: () => <CanvasStory />,
};

// #1689: フィルターが有効な間は、ボタンのアイコンが替わりハイライトされる。
export const FilteredTimeline: Story = {
  render: () => <CanvasStory filter='mutual' />,
};

export const ActivePinnedTransient: Story = {
  render: () => <CanvasStory count={3} />,
};

export const HorizontalCanvas: Story = {
  render: () => (
    <div style={{ width: '760px' }}>
      <CanvasStory count={3} />
    </div>
  ),
};

import type { Meta, StoryObj } from '@storybook/react-vite';

import { createStoryTimelinePosts } from '@/components/storyFixtures';
import i18n from '@/i18n';

import { TimelineFeed } from './TimelineFeed';

const timelinePosts = createStoryTimelinePosts();

const meta = {
  title: 'Core/TimelineFeed',
  component: TimelineFeed,
  args: {
    posts: timelinePosts,
    emptyCopy: i18n.t('shell:workspace.noPosts'),
    onOpenAuthor: () => undefined,
    onOpenThread: () => undefined,
    onReply: () => undefined,
  },
  render: (args) => (
    <div style={{ width: 'min(44rem, calc(100vw - 2rem))' }}>
      <TimelineFeed {...args} />
    </div>
  ),
} satisfies Meta<typeof TimelineFeed>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Default: Story = {};

/** #1239 AC-4: 遡った範囲に、まだ取得できていない投稿がある。続きを読む操作は残る。 */
export const UnavailablePosts: Story = {
  args: {
    unavailableCount: 3,
    hasMore: true,
    onLoadMore: () => undefined,
  },
};

/** #1239 AC-4: 読んだ範囲の投稿が、どれもまだ取得できていない(行が 0 件でも空の文言にしない)。 */
export const OnlyUnavailablePosts: Story = {
  args: {
    posts: [],
    unavailableCount: 12,
    hasMore: true,
    onLoadMore: () => undefined,
  },
};

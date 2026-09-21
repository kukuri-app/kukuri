import type { Meta, StoryObj } from '@storybook/react-vite';

import { UnavailablePostsNotice } from './UnavailablePostsNotice';

/** #1239 AC-4: 遡った範囲に、まだ取得できていない投稿があることを示す。0 件なら何も描かない。 */
const meta = {
  title: 'Core/UnavailablePostsNotice',
  component: UnavailablePostsNotice,
  args: { count: 3 },
  render: (args) => (
    <div style={{ width: 'min(44rem, calc(100vw - 2rem))' }}>
      <UnavailablePostsNotice {...args} />
    </div>
  ),
} satisfies Meta<typeof UnavailablePostsNotice>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Several: Story = {};

export const One: Story = { args: { count: 1 } };

export const Many: Story = { args: { count: 1234 } };

/** 0 件のときは何も描かない。 */
export const None: Story = { args: { count: 0 } };

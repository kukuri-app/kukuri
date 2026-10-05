import type { Meta, StoryObj } from '@storybook/react-vite';

import { RuntimeInUseView } from './RuntimeInUseView';

const meta = {
  title: 'Startup/RuntimeInUse',
  component: RuntimeInUseView,
  parameters: { layout: 'fullscreen' },
  args: { pending: false, failed: false, onTakeOver: () => undefined },
} satisfies Meta<typeof RuntimeInUseView>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
export const Pending: Story = { args: { pending: true } };
export const TakeOverFailed: Story = { args: { failed: true } };

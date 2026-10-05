import type { Meta, StoryObj } from '@storybook/react-vite';

import { BrowserStorageNotice } from './BrowserStorageNotice';

const meta = {
  title: 'Settings/BrowserStorageNotice',
  component: BrowserStorageNotice,
  args: { persisted: false },
} satisfies Meta<typeof BrowserStorageNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

export const NotPersisted: Story = {};
export const Persisted: Story = { args: { persisted: true } };

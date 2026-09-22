import { useState, type ComponentProps } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';

import { WindowClosePromptView } from './WindowClosePrompt';

const meta = {
  title: 'Shell/WindowClosePrompt',
  component: WindowClosePromptView,
  args: {
    open: true,
    remember: false,
    pending: false,
    error: null,
    onRememberChange: () => undefined,
    onChoose: () => undefined,
    onCancel: () => undefined,
  },
} satisfies Meta<typeof WindowClosePromptView>;

export default meta;
type Story = StoryObj<typeof meta>;

function InteractivePrompt(args: ComponentProps<typeof WindowClosePromptView>) {
  const [remember, setRemember] = useState(false);
  return <WindowClosePromptView {...args} remember={remember} onRememberChange={setRemember} />;
}

export const Default: Story = {
  render: (args) => <InteractivePrompt {...args} />,
};

export const SaveError: Story = {
  args: { error: 'failed' },
};

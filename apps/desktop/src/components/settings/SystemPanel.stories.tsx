import { useState, type ComponentProps } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';

import { SettingsStoryFrame } from './SettingsStoryFrame';
import { SystemPanelView, type WindowCloseSetting } from './SystemPanel';

const meta = {
  title: 'Settings/SystemPanel',
  component: SystemPanelView,
  args: {
    value: 'ask',
    pending: false,
    error: false,
    onChange: () => undefined,
  },
  decorators: [(Story) => <SettingsStoryFrame><Story /></SettingsStoryFrame>],
} satisfies Meta<typeof SystemPanelView>;

export default meta;
type Story = StoryObj<typeof meta>;

function InteractiveSystemPanel(args: ComponentProps<typeof SystemPanelView>) {
  const [value, setValue] = useState<WindowCloseSetting>(args.value);
  return <SystemPanelView {...args} value={value} onChange={setValue} />;
}

export const Default: Story = {
  render: (args) => <InteractiveSystemPanel {...args} />,
};

export const SaveError: Story = {
  args: { value: 'quit', error: true },
};

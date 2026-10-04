import type { Meta, StoryObj } from '@storybook/react-vite';

import { AccountKeyPanel } from './AccountKeyPanel';
import { SettingsStoryFrame } from './SettingsStoryFrame';

// #1211 AC-5: 別の端末で使う 3 つの方法の差。desktop は backup・restore の行を出し、Web は出さない。
const meta = {
  title: 'Settings/AccountKeyPanel',
  component: AccountKeyPanel,
  render: (args) => (
    <SettingsStoryFrame width='narrow'>
      <AccountKeyPanel {...args} />
    </SettingsStoryFrame>
  ),
} satisfies Meta<typeof AccountKeyPanel>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Desktop: Story = { args: { onOpenDeviceBackup: () => undefined } };

export const Web: Story = { args: { showBrowserStorage: true } };

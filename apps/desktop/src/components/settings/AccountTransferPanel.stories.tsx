import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { AccountTransferPanel } from './AccountTransferPanel';
import { SettingsStoryFrame } from './SettingsStoryFrame';

// #1211: QR・専用リンクの移行。mock の API は接続と相手の承認を即座に済ませる。
const meta = {
  title: 'Settings/AccountTransferPanel',
  component: AccountTransferPanel,
  args: { role: 'source' },
  render: (args) => (
    <SettingsStoryFrame width='narrow'>
      {/* 実際は「アカウント追加」Dialog の本文に置く。frame の角の丸めに掛からない余白を付ける。 */}
      <div className='p-6'><AccountTransferPanel {...args} /></div>
    </SettingsStoryFrame>
  ),
} satisfies Meta<typeof AccountTransferPanel>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Source: Story = {};

export const Target: Story = { args: { role: 'target' } };

export const TargetConfirming: Story = {
  args: { role: 'target', initialLink: 'kukuri://transfer#v1.bW9jay1hY2NvdW50LXRyYW5zZmVyLWludml0ZQ' },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: /Connect|接続する/ }));
    await expect(await canvas.findByText('482 915', {}, { timeout: 3000 })).toBeVisible();
  },
};

export const TargetConfirmed: Story = {
  args: TargetConfirming.args,
  play: async (context) => {
    await TargetConfirming.play?.(context);
    const canvas = within(context.canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: /Codes match|一致する/ }));
    await expect(await canvas.findByTestId('account-transfer-confirmed')).toBeVisible();
  },
};

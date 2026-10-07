import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { AccountTransferPanel } from './AccountTransferPanel';
import { SettingsStoryFrame } from './SettingsStoryFrame';

// #1211: QR・専用リンクの移行。mock の API は接続と相手の承認を即座に済ませ、1.5 秒の転送中の後に完了にする。
// 履歴を選んだときは、完了の前に 2 秒の履歴の受信を挟む（AC-3）。
// #1650: 本人の端末どうしの同期は、1.5 秒待ってからもう一方の端末とつながり、同じ流れで投稿まで受けて完了する。
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

/** #1628: Web 版はカメラで QR を読む（ブラウザがカメラの許可を求める。使えないときは理由を示す）。 */
export const TargetScanning: Story = {
  args: { role: 'target' },
  play: async ({ canvasElement }) => {
    await userEvent.click(within(canvasElement).getByRole('button', { name: /Scan QR code|QR コードを読み込む/ }));
  },
};

export const TargetConfirming: Story = {
  args: { role: 'target', initialLink: 'kukuri://transfer#v1.bW9jay1hY2NvdW50LXRyYW5zZmVyLWludml0ZQ' },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: /Connect|接続する/ }));
    await expect(await canvas.findByText('482 915', {}, { timeout: 3000 })).toBeVisible();
  },
};

export const TargetTransferring: Story = {
  args: TargetConfirming.args,
  play: async (context) => {
    await TargetConfirming.play?.(context);
    const canvas = within(context.canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: /Codes match|一致する/ }));
    await expect(await canvas.findByTestId('account-transfer-transferring')).toBeVisible();
  },
};

export const TargetCompleted: Story = {
  args: TargetConfirming.args,
  play: async (context) => {
    await TargetTransferring.play?.(context);
    const canvas = within(context.canvasElement);
    await expect(await canvas.findByTestId('account-transfer-completed', {}, { timeout: 4000 })).toBeVisible();
  },
};

/** 履歴の範囲を選んで接続し、承認する（AC-3）。 */
const chooseHistoryAndConfirm: NonNullable<Story['play']> = async ({ canvasElement }) => {
  const canvas = within(canvasElement);
  await userEvent.selectOptions(canvas.getByRole('combobox'), 'month');
  await userEvent.click(canvas.getByRole('button', { name: /Connect|接続する/ }));
  await expect(await canvas.findByText('482 915', {}, { timeout: 3000 })).toBeVisible();
  await userEvent.click(canvas.getByRole('button', { name: /Codes match|一致する/ }));
};

export const TargetHistory: Story = {
  args: TargetConfirming.args,
  play: async (context) => {
    await chooseHistoryAndConfirm(context);
    const canvas = within(context.canvasElement);
    await expect(await canvas.findByTestId('account-transfer-history', {}, { timeout: 4000 })).toBeVisible();
  },
};

export const TargetCompletedWithHistory: Story = {
  args: TargetConfirming.args,
  play: async (context) => {
    await chooseHistoryAndConfirm(context);
    const canvas = within(context.canvasElement);
    await expect(await canvas.findByTestId('account-transfer-history-result', {}, { timeout: 6000 })).toBeVisible();
  },
};

/** #1650: アカウントメニューの使用中の行から開く同期。もう一方の端末を待つ間の手順・残り時間・合わせるもの。 */
export const SyncWaiting: Story = { args: { role: 'sync' } };

export const SyncCompleted: Story = {
  args: { role: 'sync' },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByTestId('account-transfer-history-result', {}, { timeout: 8000 })).toBeVisible();
  },
};

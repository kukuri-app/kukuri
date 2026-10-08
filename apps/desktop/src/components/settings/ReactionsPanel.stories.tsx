import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, waitFor, within } from 'storybook/test';

import i18n from '@/i18n';
import type { CustomReactionSetView } from '@/lib/api';
import { ReactionsPanel } from './ReactionsPanel';
import { SettingsStoryFrame } from './SettingsStoryFrame';

const meta = {
  title: 'Settings/ReactionsPanel',
  component: ReactionsPanel,
  args: {
    view: { status: 'ready', summaryLabel: '', ownedAssets: [], bookmarkedAssets: [] },
    creating: false,
    onCreateAsset: () => undefined,
    onRemoveBookmark: async () => undefined,
    onListSets: async (): Promise<CustomReactionSetView[]> => [],
    onCreateSet: async (name, assets) => ({
      set_hash: '0'.repeat(64),
      name,
      item_count: assets.length,
      created_at: 0,
    }),
  },
  render: args => <SettingsStoryFrame width='narrow'><ReactionsPanel {...args} /></SettingsStoryFrame>,
} satisfies Meta<typeof ReactionsPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Empty: Story = {};
export const Saving: Story = { args: { creating: true } };
export const SelectedLongName: Story = {
  play: async ({ canvasElement }) => {
    const png = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a/A8AAAAASUVORK5CYII='), c => c.charCodeAt(0));
    const name = 'リアクション用の長いファイル名'.repeat(4) + '.png';
    const input = canvasElement.querySelector<HTMLInputElement>('input[type=file]')!;
    const selection = new DataTransfer();
    selection.items.add(new File([png], name, { type: 'image/png' }));
    input.files = selection.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    const page = within(canvasElement.ownerDocument.body);
    const crop = await page.findByRole('dialog');
    const save = within(crop).getByRole('button', { name: i18n.t('common:actions.save') });
    await waitFor(() => expect(save).toBeEnabled());
    await userEvent.click(save);
    await waitFor(() => expect(crop).not.toBeInTheDocument());
    await expect(within(canvasElement).getByText(name)).toBeVisible();
  },
};

const reaction = (id: string, owner: string) => ({
  asset_id: id,
  owner_pubkey: owner.repeat(64),
  blob_hash: `blob-${id}`,
  search_key: id,
  mime: 'image/png',
  bytes: 128,
  width: 128,
  height: 128,
});

// #1232 AC-4: 自作・保存済みから選んで名前を付けたセットと、作ったセットの一覧。
export const WithSets: Story = {
  args: {
    view: {
      status: 'ready',
      summaryLabel: '',
      ownedAssets: [reaction('party-parrot', 'a')],
      bookmarkedAssets: [reaction('saved-cat', 'b'), reaction('wave-dog', 'c')],
    },
    onListSets: async () => [
      { set_hash: '1'.repeat(64), name: 'いつものリアクション', item_count: 12, created_at: 2 },
      { set_hash: '2'.repeat(64), name: 'Cats', item_count: 100, created_at: 1 },
    ],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText('Cats');
    await userEvent.click(
      canvas.getByRole('checkbox', {
        name: i18n.t('settings:reactions.selectReaction', { key: 'party-parrot' }),
      })
    );
    await userEvent.click(
      canvas.getByRole('checkbox', {
        name: i18n.t('settings:reactions.selectReaction', { key: 'saved-cat' }),
      })
    );
    await userEvent.type(canvas.getByLabelText(i18n.t('settings:reactions.setName')), 'Party');
    await expect(
      canvas.getByRole('button', { name: i18n.t('settings:reactions.createSet', { count: 2 }) })
    ).toBeEnabled();
  },
};

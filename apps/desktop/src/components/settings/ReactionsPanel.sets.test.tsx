import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import { ReactionsPanel } from './ReactionsPanel';

// #1232 AC-4: 自作・保存済みから選んだリアクションに名前を付けてセットを作り、共有用の文字列をコピーする。
test('reactions panel makes a named set from own and saved reactions and copies its share text', async () => {
  const user = userEvent.setup();
  const clipboardWriteText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
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
  const owned = reaction('parrot', 'a');
  const saved = Array.from({ length: 101 }, (_, index) => reaction(`saved-${index}`, 'b'));
  const created = { set_hash: '1'.repeat(64), name: 'party', item_count: 2, created_at: 2 };
  const onCreateSet = vi.fn().mockResolvedValue(created);

  render(
    <ReactionsPanel
      view={{ status: 'ready', summaryLabel: '', ownedAssets: [owned], bookmarkedAssets: saved }}
      creating={false}
      onCreateAsset={() => {}}
      onRemoveBookmark={async () => {}}
      onListSets={async () => [{ set_hash: '2'.repeat(64), name: 'older', item_count: 5, created_at: 1 }]}
      onCreateSet={onCreateSet}
    />
  );

  expect(await screen.findByText('older')).toBeInTheDocument();
  await user.type(screen.getByLabelText('Set name'), 'party');
  expect(screen.getByRole('button', { name: 'Create a set from 0 selected' })).toBeDisabled();

  // 1 つのセットは 100 件まで。
  await user.click(screen.getByRole('checkbox', { name: 'Select all' }));
  expect(screen.getByText('A set can hold up to 100 reactions (101 selected).')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Create a set from 101 selected' })).toBeDisabled();
  await user.click(screen.getByRole('checkbox', { name: 'Select all' }));

  // 自作だけを選んでいる間は、保存済みの解除はできない。
  await user.click(screen.getByRole('checkbox', { name: 'Select parrot' }));
  expect(screen.getByRole('button', { name: 'Clear selected' })).toBeDisabled();
  await user.click(screen.getByRole('checkbox', { name: 'Select saved-0' }));
  await user.click(screen.getByRole('button', { name: 'Create a set from 2 selected' }));

  expect(onCreateSet).toHaveBeenCalledWith('party', [owned, saved[0]]);
  const sets = await screen.findAllByRole('listitem');
  expect(sets.map((item) => item.querySelector('strong')?.textContent)).toEqual(['party', 'older']);
  expect(screen.getByLabelText('Set name')).toHaveValue('');
  expect(screen.getByRole('checkbox', { name: 'Select parrot' })).not.toBeChecked();

  await user.click(screen.getByRole('button', { name: 'Copy the share text of “older”' }));
  expect(clipboardWriteText).toHaveBeenLastCalledWith(`kukuri:reaction-set:${'2'.repeat(64)}`);
});

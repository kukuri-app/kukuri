import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createInstance } from 'i18next';
import { I18nextProvider } from 'react-i18next';
import { expect, test, vi } from 'vitest';

import { LocaleSelect } from '@/components/LocaleSelect';
import { resources } from '@/i18n';
import { ReactionsPanel } from './ReactionsPanel';

async function translations() {
  const i18n = createInstance();
  await i18n.init({ resources, lng: 'ja', fallbackLng: 'en' });
  return i18n;
}

test('the shared language field uses Japanese copy while preserving language autonyms', async () => {
  const i18n = await translations();
  const change = vi.fn();
  const view = render(<I18nextProvider i18n={i18n}>
    <LocaleSelect value='ja' onChange={change} />
  </I18nextProvider>);
  expect(screen.getByRole('combobox', { name: '言語' })).toBeVisible();
  expect(screen.queryByText(/Language/)).not.toBeInTheDocument();
  expect(screen.getByRole('option', { name: 'English' })).toBeInTheDocument();
  expect(screen.getByRole('option', { name: '简体中文' })).toBeInTheDocument();
  await userEvent.setup().selectOptions(screen.getByRole('combobox'), 'en');
  expect(change).toHaveBeenCalledWith('en');
  await act(() => i18n.changeLanguage('en'));
  expect(screen.getByRole('combobox', { name: 'Language' })).toBeVisible();
  await act(() => i18n.changeLanguage('ja'));
  view.rerender(<I18nextProvider i18n={i18n}>
    <LocaleSelect value='ja' onChange={change} disabled saveFailed />
  </I18nextProvider>);
  expect(screen.getByRole('combobox', { name: '言語' })).toBeDisabled();
  expect(screen.getByRole('button', { name: '言語の保存を再試行' })).toBeDisabled();
});

test('reaction selection owns localized visible copy and cancellation never registers an asset', async () => {
  const i18n = await translations();
  const create = vi.fn();
  const view = render(<I18nextProvider i18n={i18n}>
    <ReactionsPanel
      view={{ status: 'ready', summaryLabel: '', panelError: null, ownedAssets: [], bookmarkedAssets: [] }}
      mediaObjectUrls={{}}
      creating={false}
      onCreateAsset={create}
      onRemoveBookmark={vi.fn()}
      onListSets={async () => []}
      onCreateSet={vi.fn()}
    />
  </I18nextProvider>);
  const button = screen.getByRole('button', { name: 'ファイルを選択' });
  const input = view.container.querySelector('input[type=file]')!;
  expect(input).not.toBeVisible();
  expect(input).not.toHaveAttribute('multiple');
  expect(screen.getByText('ファイル未選択')).toBeVisible();
  const click = vi.fn();
  input.addEventListener('click', click);
  for (const key of ['{Enter}', ' ']) {
    button.focus();
    await userEvent.setup().keyboard(key);
    fireEvent(input, new Event('cancel'));
    expect(button).toHaveFocus();
  }
  expect(click).toHaveBeenCalledTimes(2);
  expect(create).not.toHaveBeenCalled();
  await act(() => i18n.changeLanguage('zh-CN'));
  expect(screen.getByRole('button', { name: '选择文件' })).toBeVisible();
  expect(screen.getByText('未选择文件')).toBeVisible();
});

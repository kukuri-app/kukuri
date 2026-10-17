// #1840 AC-4: 公開スレッドの詳細の上部の「ライブ」から、スレッドの「ライブ」の列で Live を始める。始めた Live は詳細の上部と
// 一覧の行の印に出て、上部から列で開ける。終えると上部から消え、開始 post はスレッドに残る。
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type { ColumnState } from '@/shell/slices/workspace';
import { WORKSPACE_LAYOUT_STORAGE_KEY } from '@/shell/workspacePersistence';

import { getActiveColumn, renderAtHash, setViewportWidth } from './DesktopShellPage.testHelpers';

const TOPIC = 'kukuri:topic:general';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

function persistedColumns(): ColumnState[] {
  return (JSON.parse(window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY) ?? '{}') as { columns?: ColumnState[] })
    .columns ?? [];
}

function footer(column: HTMLElement) {
  return within(column.querySelector<HTMLElement>('.shell-column-footer') ?? column);
}

test('a live started in a public thread shows in the detail header and the list until it ends', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createLiveSession = vi.spyOn(api, 'createLiveSession');
  const thread = await api.createPublicChannel(TOPIC, 'Watch party', '');
  renderAtHash(`#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=${thread.channel_id}`, api);
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  expect(within(detail).queryByRole('list', { name: 'Live now' })).toBeNull();

  // 上部の「ライブ」から、詳細の列を親にしたスレッドの「ライブ」の列を開き、下部から始める(判断 15)。
  await user.click(within(detail).getByRole('button', { name: 'Live' }));
  const liveColumn = await waitFor(() => getActiveColumn('Live'));
  expect(within(liveColumn).getAllByText('Watch party · general').length).toBeGreaterThan(0);
  const opened = persistedColumns().find((column) => column.kind === 'stream');
  const detailColumn = persistedColumns().find((column) => column.scope?.channelId === thread.channel_id);
  expect(opened?.scope).toEqual({ topicId: TOPIC, channelId: thread.channel_id });
  expect(opened?.parentColumnId).toBe(detailColumn?.id);
  await user.click(footer(liveColumn).getByRole('button', { name: 'Start Live' }));
  const dialog = await screen.findByRole('dialog', { name: 'Start Live' });
  await user.type(within(dialog).getByPlaceholderText('Friday stream'), 'Night stream');
  await user.click(within(dialog).getByRole('button', { name: 'Start Live' }));
  await waitFor(() => expect(createLiveSession).toHaveBeenCalled());
  expect(createLiveSession.mock.calls[0]?.slice(0, 4)).toEqual([
    TOPIC,
    'Night stream',
    '',
    { kind: 'public_channel', channel_id: thread.channel_id },
  ]);
  const sessionId = await createLiveSession.mock.results[0]!.value;
  expect(await within(liveColumn).findByText('Night stream')).toBeInTheDocument();

  // 列を閉じると詳細の列へ戻り、上部に配信中の行が出る。開始 post はスレッドに入る。
  await user.click(within(liveColumn).getByRole('button', { name: 'Close Live' }));
  const back = await waitFor(() => getActiveColumn('Public thread'));
  const row = await within(back).findByRole('button', { name: /^Live Night stream / });
  expect(back.querySelector('.post-live')).not.toBeNull();

  // 紹介の出し先は始めたスレッドが先頭(既定。判断 17)。スレッドへ紹介しても、上部の行は 1 件のまま。
  const createPost = vi.spyOn(api, 'createPost');
  await user.click(within(back.querySelector<HTMLElement>('.post-live')!).getByRole('button', { name: 'Share' }));
  const share = await screen.findByRole('dialog', { name: 'Share live session' });
  const target0 = within(share).getByRole('combobox', { name: 'Post to' }) as HTMLSelectElement;
  expect(target0.options[0]?.textContent).toBe('general / Watch party');
  expect(target0.value).toBe('0');
  await user.click(within(share).getByRole('button', { name: 'Share' }));
  await waitFor(() =>
    expect(createPost).toHaveBeenLastCalledWith(
      TOPIC,
      '',
      null,
      [],
      { kind: 'public_channel', channel_id: thread.channel_id },
      [],
      { session_id: sessionId, topic_id: TOPIC, channel_id: thread.channel_id }
    )
  );
  await waitFor(() => expect(back.querySelectorAll('.post-live')).toHaveLength(2));
  expect(within(within(back).getByRole('list', { name: 'Live now' })).getAllByRole('button')).toHaveLength(1);

  // 上部の行から、その Live を示す列を開く。
  await user.click(row);
  const target = await waitFor(() => getActiveColumn('Live'));
  await waitFor(() =>
    expect(target.querySelector(`[data-live-session-id="${sessionId}"]`)).not.toBeNull()
  );

  // 終えると上部から消える。開始 post は残る。
  await user.click(footer(target).getByRole('button', { name: 'End' }));
  await waitFor(() => expect(within(target).getByText('Ended')).toBeInTheDocument());
  await user.click(within(target).getByRole('button', { name: 'Close Live' }));
  const after = await waitFor(() => getActiveColumn('Public thread'));
  await waitFor(() => expect(within(after).queryByRole('list', { name: 'Live now' })).toBeNull());
  expect(after.querySelector('.post-live')).not.toBeNull();
}, 30_000);

test('the list row of a public thread is marked while a live is on', async () => {
  const api = createDesktopMockApi();
  const thread = await api.createPublicChannel(TOPIC, 'Watch party', '');
  await api.createLiveSession(TOPIC, 'Night stream', '', { kind: 'public_channel', channel_id: thread.channel_id });
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&context=publicThreads', api);
  const list = await waitFor(() => getActiveColumn('Public threads'));
  expect(await within(list).findByRole('button', { name: /^Watch party .* Live$/ })).toBeInTheDocument();
});

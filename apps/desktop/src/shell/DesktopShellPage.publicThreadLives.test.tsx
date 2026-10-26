// #1840 AC-4: 公開スレッドの詳細の上部の「ライブ」から、スレッドの「ライブ」の列で Live を始める。始めた Live は詳細の上部と
// 一覧の行の印に出て、上部から列で開ける。終えると上部から消え、開始 post はスレッドに残る。
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';

import { getActiveColumn, renderAtHash, setViewportWidth } from './DesktopShellPage.testHelpers';

const TOPIC = 'kukuri:topic:general';

beforeEach(() => {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

function footer(column: HTMLElement) {
  return within(column.querySelector<HTMLElement>('.shell-column-footer') ?? column);
}

test('creating a public thread can also start a live in that thread without capturing media', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createLive = vi.spyOn(api, 'createLiveSession');
  const capture = vi.spyOn(api, 'setLiveBroadcast');
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&context=publicThreads', api);
  const list = await waitFor(() => getActiveColumn('Public threads'));
  await user.click(footer(list).getByRole('button', { name: 'Create public thread' }));
  const dialog = await screen.findByRole('dialog', { name: 'Create a public thread' });
  await user.type(within(dialog).getByRole('textbox', { name: 'Title' }), 'Live together');
  await user.type(within(dialog).getByRole('textbox', { name: 'First post (optional)' }), 'Welcome');
  await user.click(within(dialog).getByRole('checkbox', { name: 'Start Live' }));
  await user.click(within(dialog).getByRole('button', { name: 'Create' }));
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  expect(await within(within(detail).getByRole('list', { name: 'Live now' })).findByRole('button', { name: /^Live Live together / })).toBeInTheDocument();
  expect(within(detail).getByText('Welcome')).toBeInTheDocument();
  expect(createLive).toHaveBeenCalledWith(TOPIC, 'Live together', '', { kind: 'public_channel', channel_id: expect.stringMatching(/^pc-/) });
  expect(capture).not.toHaveBeenCalled();
});

test('a live started in a public thread shows in the detail header and the list until it ends', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createLiveSession = vi.spyOn(api, 'createLiveSession');
  const thread = await api.createPublicChannel(TOPIC, 'Watch party', '');
  renderAtHash(`#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=${thread.channel_id}`, api);
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  expect(within(detail).queryByRole('list', { name: 'Live now' })).toBeNull();

  // 投稿欄からその公開スレッドで直接開始する。
  expect(within(detail).queryByRole('button', { name: 'Live' })).toBeNull();
  await user.click(footer(detail).getByRole('button', { name: /^Post to / }));
  await user.click(within(detail).getByRole('button', { name: 'Start Live' }));
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
  const back = await waitFor(() => getActiveColumn('Public thread'));
  const row = await within(within(back).getByRole('list', { name: 'Live now' })).findByRole('button', { name: /^Live Night stream / });
  expect(back.querySelector('.post-live')).not.toBeNull();

  // 紹介の出し先は始めたスレッドが先頭(既定。判断 17)。スレッドへ紹介しても、上部の行は 1 件のまま。
  const createPost = vi.spyOn(api, 'createPost');
  await user.click(within(back.querySelector<HTMLElement>('.post-live')!).getByRole('button'));
  await user.click(within(await screen.findByRole('dialog', { name: 'Night stream' })).getByRole('button', { name: 'Share' }));
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
  await user.click(within(await screen.findByRole('dialog', { name: 'Night stream' })).getByRole('button', { name: 'Show large' }));
  const target = await waitFor(() => getActiveColumn('Live'));
  await waitFor(() =>
    expect(target.querySelector(`[data-live-session-id="${sessionId}"]`)).not.toBeNull()
  );

  // 終えると上部から消える。開始 post は残る。
  await user.click(footer(target).getByRole('button', { name: 'End' }));
  await waitFor(() => expect(within(target).getByText('This live session has ended')).toBeInTheDocument());
  await user.click(within(target).getByRole('button', { name: 'Close Live' }));
  const after = await waitFor(() => getActiveColumn('Public thread'));
  await waitFor(() => expect(within(after).queryByRole('list', { name: 'Live now' })).toBeNull());
  expect(after.querySelector('.post-live')).not.toBeNull();
}, 30_000);

// 一覧の行の印と詳細の上部の行を出すだけでは、Live に参加せず、映像・音声を受信も送信もしない。
test('the list row is marked and the detail header lists the live without receiving or sending media', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const media = [
    vi.spyOn(api, 'joinLiveSession'),
    vi.spyOn(api, 'watchLiveMedia'),
    vi.spyOn(api, 'setLiveBroadcast'),
  ];
  const thread = await api.createPublicChannel(TOPIC, 'Watch party', '');
  await api.createLiveSession(TOPIC, 'Night stream', '', { kind: 'public_channel', channel_id: thread.channel_id });
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&context=publicThreads', api);
  const list = await waitFor(() => getActiveColumn('Public threads'));
  await user.click(await within(list).findByRole('button', { name: /^Watch party .* Live$/ }));
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  expect(await within(within(detail).getByRole('list', { name: 'Live now' })).findByRole('button', { name: /^Live Night stream / })).toBeInTheDocument();
  for (const call of media) expect(call).not.toHaveBeenCalled();
});

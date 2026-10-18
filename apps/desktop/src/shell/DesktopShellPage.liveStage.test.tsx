// #1841 AC-3: ライブの列の左側に、列の Live を大きく出して会話の投稿とリアクションを重ねる。一覧のカードの「大きく表示」は列の Live を
// 切り替え(前の Live のリアクションを混ぜない)、投稿の中の Live の「大きく表示」は投稿の場所を会話にした列を開く(Live の場所は列に
// 持たせ、保存と URL に載せる)。列の下部から参加でき、列を閉じると Live の場所の購読の需要を外す。
import { act, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { announceReactionReceived } from '@/shell/data/useRuntimeEventBridge';
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

const card = (column: HTMLElement, sessionId: string) =>
  column.querySelector<HTMLElement>(`article[data-live-session-id="${sessionId}"]`);

function reaction(target: string, id: string) {
  act(() =>
    announceReactionReceived({
      target_object_id: target,
      reaction_id: id,
      reaction: { reaction_key_kind: 'emoji', normalized_reaction_key: '🎉', emoji: '🎉', custom_asset: null },
      updated_at: Date.now(),
    })
  );
}

test('a card shows its live large, and another card switches the live without mixing the reactions', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const morning = await api.createLiveSession(TOPIC, 'Morning stream', '', { kind: 'public' }, 'morning start');
  const evening = await api.createLiveSession(TOPIC, 'Evening stream', '', { kind: 'public' }, 'evening start');
  const morningStart = (await api.getLiveSession(TOPIC, morning))!.start_post_id!;
  const eveningStart = (await api.getLiveSession(TOPIC, evening))!.start_post_id!;
  await api.toggleReaction(TOPIC, eveningStart, { kind: 'emoji', emoji: '🔥' });
  renderAtHash('#/live?topic=kukuri%3Atopic%3Ageneral', api);
  const list = await waitFor(() => getActiveColumn('Live'));
  await waitFor(() => expect(card(list, morning)).not.toBeNull());
  expect(list.querySelector('.live-stage')).toBeNull();

  await user.click(within(card(list, morning)!).getByRole('button', { name: 'Show large' }));
  const column = await waitFor(() => getActiveColumn('Live'));
  const stage = await waitFor(() => within(column.querySelector<HTMLElement>('.live-stage')!).getByText('Morning stream'));
  expect(stage).toBeVisible();
  // 左側に出している Live のカードには「大きく表示」を出さない。
  await waitFor(() => expect(card(column, evening)).not.toBeNull());
  expect(within(card(column, morning)!).queryByRole('button', { name: 'Show large' })).toBeNull();

  // 別のカードで Live を切り替えると、列を置き換える(列は 1 つのまま)。
  await user.click(within(card(column, evening)!).getByRole('button', { name: 'Show large' }));
  const switched = await waitFor(() => getActiveColumn('Live'));
  await waitFor(() => expect(within(switched.querySelector<HTMLElement>('.live-stage')!).getByText('Evening stream')).toBeVisible());
  await waitFor(() => expect(persistedColumns().filter((item) => item.kind === 'stream').map((item) => item.entityId)).toEqual([evening]));
  // 数は切り替えた Live の開始 post のもの。前の Live の開始 post への新着は浮かべない。
  await waitFor(() => expect(switched.querySelector('.live-stage-count')).toHaveTextContent('🔥1'));
  reaction(morningStart, 'r-morning');
  expect(switched.querySelectorAll('.live-stage-float')).toHaveLength(0);
  reaction(eveningStart, 'r-evening');
  expect(switched.querySelectorAll('.live-stage-float')).toHaveLength(1);
}, 30_000);

test("a post's live opens large in the post's place, keeps the live place, joins from the footer, and closing releases its demand", async () => {
  const user = userEvent.setup();
  // 別の人が topic で始めた Live を、スレッドで紹介した投稿から開く。
  const sessionId = 'live-night';
  const api = createDesktopMockApi({
    seedLiveSessions: {
      [TOPIC]: [
        {
          session_id: sessionId,
          host_pubkey: 'b'.repeat(64),
          title: 'Night stream',
          description: '',
          status: 'Live',
          started_at: 1,
          viewer_count: 1,
          joined_by_me: false,
          channel_id: null,
          audience_label: 'Public',
        },
      ],
    },
  });
  const setScopeDisplay = vi.spyOn(api, 'setScopeDisplay');
  const joinLiveSession = vi.spyOn(api, 'joinLiveSession');
  const thread = await api.createPublicChannel(TOPIC, 'Watch party', '');
  await api.createPost(TOPIC, 'come and watch', null, [], { kind: 'public_channel', channel_id: thread.channel_id }, [], {
    session_id: sessionId,
    topic_id: TOPIC,
    channel_id: null,
  });
  renderAtHash(`#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=${thread.channel_id}`, api);
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  const intro = await waitFor(() => within(detail.querySelector<HTMLElement>('.post-live')!).getByRole('button', { name: 'Show large' }));

  await user.click(intro);
  const column = await waitFor(() => getActiveColumn('Live'));
  const stage = await waitFor(() => column.querySelector<HTMLElement>('.live-stage')!);
  await waitFor(() => expect(within(stage).getByText('Night stream')).toBeVisible());
  expect(within(stage).getByText('Conversation: Watch party')).toBeVisible();
  // 会話はスレッド(列の scope)で、紹介 post を重ねる。Live の場所は列に持たせ、保存と URL に載せる。
  await waitFor(() => expect(within(stage).getByText('come and watch')).toBeInTheDocument());
  const opened = persistedColumns().find((item) => item.kind === 'stream');
  expect(opened).toMatchObject({
    scope: { topicId: TOPIC, channelId: thread.channel_id },
    entityId: sessionId,
    live: { topicId: TOPIC, channelId: null },
  });
  expect(window.location.hash).toContain('liveTopic=kukuri%3Atopic%3Ageneral');
  // Live の場所も購読する(列の id に #live を付けた observer)。
  await waitFor(() =>
    expect(setScopeDisplay).toHaveBeenCalledWith({
      observer: `${opened!.id}#live`,
      target: { kind: 'timeline', topic: TOPIC, scope: { kind: 'public' } },
      visible: true,
    })
  );

  // 列の下部から、Live の場所の Live に参加する。
  const footer = within(column.querySelector<HTMLElement>('.shell-column-footer')!);
  await user.click(await footer.findByRole('button', { name: 'Join' }));
  await waitFor(() => expect(joinLiveSession).toHaveBeenCalledWith(TOPIC, sessionId));

  // 列を閉じると、Live の場所の購読の需要を外す。
  await user.click(within(column).getByRole('button', { name: 'Close Live' }));
  await waitFor(() =>
    expect(setScopeDisplay).toHaveBeenCalledWith({
      observer: `${opened!.id}#live`,
      target: { kind: 'timeline', topic: TOPIC, scope: { kind: 'public' } },
      visible: false,
    })
  );
}, 30_000);

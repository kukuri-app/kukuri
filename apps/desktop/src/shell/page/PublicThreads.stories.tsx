import type { Meta, StoryObj } from '@storybook/react-vite';
import { useContext, useEffect, useState } from 'react';

import type {
  AuthorSocialView,
  DesktopApi,
  LiveSessionView,
  PostView,
  PublicChannelListView,
  PublicChannelView,
} from '@/lib/api';
import type { ColumnState } from '@/shell/slices/workspace';
import { createDesktopShellStore, DesktopShellStoreContext, timelineStorageKeyForChannel } from '@/shell/store';

import { PublicThreadActionsContext } from './publicThreadActions';
import { PublicThreadLinkCard } from './PublicThreadLinkCard';
import { PublicThreadList, PublicThreadOrderMenu } from './PublicThreadList';
import { PublicThreadLiveRows } from './PublicThreadLive';
import { PublicThreadProviders } from './PublicThreadProviders';

const TOPIC = 'kukuri:topic:general';
const STORY_TIMESTAMP = 1_742_860_800_000;
const CREATOR = 'b'.repeat(64);

function thread(index: number, title: string, firstPostId?: string): PublicChannelView {
  return {
    channel_id: `pc-${STORY_TIMESTAMP - index * 3_600_000}-0123456789abcdef`,
    topic_id: TOPIC,
    title,
    creator_pubkey: CREATOR,
    created_at: STORY_TIMESTAMP - index * 3_600_000,
    last_post_at: STORY_TIMESTAMP - index * 600_000,
    live_now: false,
    ...(firstPostId ? { first_post_id: firstPostId } : {}),
  };
}

const THREADS = [
  thread(1, 'v0.5 の配布候補の確認（Windows・Linux・Android）', 'first-post'),
  thread(2, 'Weekly sync: what shipped this week and what comes next'),
  thread(
    3,
    '長いタイトルの折り返しの確認: コミュニティノードの接続が切れたときに、タイムラインとスレッドの表示がどう変わるかを、端末ごとに書き出していくスレッド'
  ),
  thread(4, 'Questions about relay fallback'),
];

const FIRST_POST = {
  object_id: 'first-post',
  content:
    '配布候補を入れた端末で、起動・投稿・Live の視聴を確かめた結果をここに書いてください。気になった点は、端末と OS の版も一緒に。',
} as PostView;

// #1840 AC-4: スレッドで配信中の Live(新しく始まった順)。
const LIVES: LiveSessionView[] = [
  ['Windows 11 で通しの確認', 'b'],
  ['Android 15 の通知の再現', 'c'],
  ['Starting the Linux AppImage', 'd'],
  ['Q&A', 'e'],
].map(([title, host], index) => ({
  session_id: `live-${index}`,
  host_pubkey: host!.repeat(64),
  title: title!,
  description: '',
  status: 'Live',
  started_at: STORY_TIMESTAMP - index * 600_000,
  viewer_count: 3,
  joined_by_me: false,
  channel_id: THREADS[0]!.channel_id,
  audience_label: 'Public thread',
}));
const HOSTS: Record<string, AuthorSocialView> = Object.fromEntries(
  ['みな', 'Ken', 'あおい', 'Sora'].map((name, index) => {
    const pubkey = 'bcde'[index]!.repeat(64);
    return [pubkey, { author_pubkey: pubkey, display_name: name } as AuthorSocialView];
  })
);

type StoryProps = {
  surface: 'list' | 'card' | 'create' | 'lives';
  lives?: LiveSessionView[];
  page?: PublicChannelListView;
  order?: ColumnState['publicThreadOrder'];
  pending?: boolean;
  failed?: boolean;
  shown?: PublicChannelView | null;
  firstPost?: PostView | null;
};

function storyApi({ page, pending = false, failed = false, shown = null, firstPost = null }: StoryProps): DesktopApi {
  const wait = <T,>(value: () => T) =>
    pending ? new Promise<T>(() => undefined) : failed ? Promise.reject(new Error('offline')) : Promise.resolve(value());
  return {
    listPublicChannels: () => wait(() => page ?? { items: [], next_cursor: null, unavailable_count: 0 }),
    getPublicChannel: () => wait(() => shown),
    listThread: () => wait(() => ({ items: firstPost ? [firstPost] : [], next_cursor: null })),
    createPublicChannel: () => Promise.reject(new Error('offline')),
  } as unknown as DesktopApi;
}

function OpenCreateDialog() {
  const actions = useContext(PublicThreadActionsContext);
  useEffect(() => actions?.create(TOPIC), [actions]);
  return null;
}

function PublicThreadsStory(props: StoryProps) {
  const [store] = useState(() => {
    const created = createDesktopShellStore();
    // 配信中の Live の表示は開発者モードでだけ出す。
    created.getState().setField('developerModeEnabled', true);
    created.getState().setField('knownAuthorsByPubkey', HOSTS);
    created.getState().setField('liveSessionsByScopeKey', {
      [timelineStorageKeyForChannel(TOPIC, THREADS[0]!.channel_id)]: props.lives ?? [],
    });
    return created;
  });
  // 描画のたびに api を作り直すと、読み込みの effect がやり直しを繰り返す。
  const [api] = useState(() => storyApi(props));
  const column: ColumnState = {
    id: 'column:publicThreads:story',
    kind: 'publicThreads',
    scope: { topicId: TOPIC, channelId: null },
    pinned: true,
    preferredDesktopSpan: 1,
    ...(props.order ? { publicThreadOrder: props.order } : {}),
  };
  return (
    <DesktopShellStoreContext.Provider value={store}>
      <PublicThreadProviders api={api} open={() => undefined} openLive={() => undefined} copyLink={() => undefined}>
        <div className='shell-phase1' style={{ maxWidth: 560 }}>
          {props.surface === 'list' ? (
            <>
              <PublicThreadOrderMenu column={column} />
              <PublicThreadList column={column} />
            </>
          ) : props.surface === 'card' ? (
            <article className='post-card'>
              <PublicThreadLinkCard topic={TOPIC} channelId={THREADS[0]!.channel_id} />
            </article>
          ) : props.surface === 'lives' ? (
            <PublicThreadLiveRows topic={TOPIC} channelId={THREADS[0]!.channel_id} />
          ) : (
            <OpenCreateDialog />
          )}
        </div>
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
}

const meta = {
  title: 'Shell/PublicThreads',
  component: PublicThreadsStory,
} satisfies Meta<typeof PublicThreadsStory>;

export default meta;

type Story = StoryObj<typeof meta>;

/** 一覧の列。最後の投稿の新しい順(既定)。取得できなかったスレッドの数と、続きを読む操作がある。 */
export const List: Story = {
  args: {
    surface: 'list',
    page: { items: THREADS, next_cursor: { at: THREADS[3]!.last_post_at, channel_id: THREADS[3]!.channel_id }, unavailable_count: 2 },
  },
};

/** 配信中の Live がある行は、時刻の横に「配信中」の印を出す(#1840 AC-4)。 */
export const ListLiveMark: Story = {
  args: {
    surface: 'list',
    page: { items: [{ ...THREADS[0]!, live_now: true }, ...THREADS.slice(1)], next_cursor: null, unavailable_count: 0 },
  },
};

export const ListCreatedOrder: Story = {
  args: { surface: 'list', order: 'created', page: { items: THREADS, next_cursor: null, unavailable_count: 0 } },
};

export const ListLoading: Story = { args: { surface: 'list', pending: true } };

/** 0 件。作り方の案内と「スレッドを作る」を出す。 */
export const ListEmpty: Story = { args: { surface: 'list' } };

export const ListFailed: Story = { args: { surface: 'list', failed: true } };

/** 本文の中のリンクの card。タイトルと作成時の最初の投稿。 */
export const CardWithFirstPost: Story = { args: { surface: 'card', shown: THREADS[0], firstPost: FIRST_POST } };

/** 本文を空にして作ったスレッドは、タイトルだけ。 */
export const CardTitleOnly: Story = { args: { surface: 'card', shown: THREADS[1] } };

export const CardLoading: Story = { args: { surface: 'card', pending: true } };

export const CardUnavailable: Story = { args: { surface: 'card', shown: null } };

/** 作成の Dialog。この story の作成は失敗する(失敗の表示の確認)。 */
export const CreateDialog: Story = { args: { surface: 'create' } };

/** 詳細の列の上部の配信中の Live(#1840 AC-4)。タイトルと配信者を 1 行ずつ出し、押すとスレッドの「ライブ」の列を開く。 */
export const ThreadLives: Story = { args: { surface: 'lives', lives: LIVES.slice(0, 2) } };

/** 4 件以上は、新しく始まった 3 件と「すべての配信」。 */
export const ThreadLivesMore: Story = { args: { surface: 'lives', lives: LIVES } };

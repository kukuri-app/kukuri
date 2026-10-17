import { CalendarPlus, Check, Link2, MessageSquareText } from 'lucide-react';
import { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { EmptyStateGuidance } from '@/components/core/EmptyStateGuidance';
import { useInfiniteScrollSentinel } from '@/components/core/useInfiniteScrollSentinel';
import { useWindowScrollAnchor } from '@/components/core/useWindowScrollAnchor';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Notice } from '@/components/ui/notice';
import { Popover, PopoverMenuContent, PopoverTrigger } from '@/components/ui/popover';
import { formatLocalizedDateTime } from '@/i18n/format';
import type { PublicChannelOrder, PublicChannelView } from '@/lib/api';
import { useColumnRuntime } from '@/shell/ColumnRuntimeContext';
import { VISIBLE_TIMELINE_LIMIT } from '@/shell/pagination';
import { authorDisplayLabel } from '@/shell/presentation';
import { isHiddenAuthor, knownPublicThreadTitle } from '@/shell/slices/publicThreads';
import { setColumnPublicThreadOrder, type ColumnState } from '@/shell/slices/workspace';
import { REFRESH_INTERVAL_MS, useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';

import { PublicThreadActionsContext } from './publicThreadActions';
import { INITIAL_PUBLIC_THREAD_LIST, withHeadPage, withNextPage } from './publicThreadListState';

const ORDERS = [
  { value: 'last_post', icon: MessageSquareText },
  { value: 'created', icon: CalendarPlus },
] as const;

/** topic の公開スレッドの一覧の列の本体(#1840)。表示中の間だけ先頭のページを読み直す。並びを変えたら作り直す(key)。 */
export function PublicThreadList({ column }: { column: ColumnState }) {
  const { t } = useTranslation(['shell', 'common']);
  const actions = useContext(PublicThreadActionsContext);
  const api = actions?.api;
  const remember = actions?.remember;
  const { visible } = useColumnRuntime();
  const knownAuthorsByPubkey = useDesktopShellStore((state) => state.knownAuthorsByPubkey);
  const topic = column.scope?.topicId ?? '';
  const order: PublicChannelOrder = column.publicThreadOrder ?? 'last_post';
  const createdHere = actions?.lastCreated?.topic_id === topic ? actions.lastCreated.channel_id : null;
  const [state, setState] = useState(INITIAL_PUBLIC_THREAD_LIST);
  const headRequest = useRef(0);
  const nextRequest = useRef(0);

  // 先頭のページを読む。`reset` は初めてと作った直後と最新に戻るときで、読み込んだ続きを捨てる。
  const loadHead = useCallback(
    async (reset: boolean) => {
      if (!api || !remember || !topic) return;
      const request = ++headRequest.current;
      if (reset) nextRequest.current += 1;
      try {
        const page = await api.listPublicChannels(topic, order, null, VISIBLE_TIMELINE_LIMIT);
        if (request !== headRequest.current) return;
        remember(page.items);
        setState((current) => {
          if (!reset && (current.slid || current.loadingMore)) return current;
          const next = withHeadPage(reset ? INITIAL_PUBLIC_THREAD_LIST : current, page, order);
          return JSON.stringify(next) === JSON.stringify(current) ? current : next;
        });
      } catch {
        if (request !== headRequest.current) return;
        setState((current) =>
          reset || current.status !== 'ready' ? { ...INITIAL_PUBLIC_THREAD_LIST, status: 'error' } : current
        );
      }
    },
    [api, remember, topic, order]
  );
  const loaded = state.status !== 'loading';
  useEffect(() => {
    if (visible && !loaded) void loadHead(true);
  }, [visible, loaded, loadHead]);
  // 作った直後だけ先頭から読み直す(並びの切替で作り直した列は、最初の読み込みだけにする)。
  const handledCreated = useRef(createdHere);
  useEffect(() => {
    if (!createdHere || createdHere === handledCreated.current) return;
    handledCreated.current = createdHere;
    void loadHead(true);
  }, [createdHere, loadHead]);
  useEffect(() => {
    if (!visible) return;
    const intervalId = window.setInterval(() => {
      if (document.visibilityState !== 'hidden') void loadHead(false);
    }, REFRESH_INTERVAL_MS);
    return () => window.clearInterval(intervalId);
  }, [visible, loadHead]);

  const cursor = state.next;
  const loadingMore = state.loadingMore;
  const loadNext = useCallback(async () => {
    if (!api || !remember || !cursor || loadingMore) return;
    const request = ++nextRequest.current;
    setState((current) => ({ ...current, loadingMore: true, loadMoreFailed: false }));
    try {
      const page = await api.listPublicChannels(topic, order, cursor, VISIBLE_TIMELINE_LIMIT);
      if (request !== nextRequest.current) return;
      remember(page.items);
      setState((current) => withNextPage(current, page));
    } catch {
      if (request !== nextRequest.current) return;
      setState((current) => ({ ...current, loadingMore: false, loadMoreFailed: true }));
    }
  }, [api, remember, topic, order, cursor, loadingMore]);
  // 作成者を非表示にした行(mute・block)は、次に読んだページが除く前でも出さない(読み込んだ続きの行を含む)。
  const rows = useMemo(
    () => state.rows.filter((row) => !isHiddenAuthor(knownAuthorsByPubkey[row.creator_pubkey])),
    [state.rows, knownAuthorsByPubkey]
  );
  // 行の位置の保持は投稿の一覧の hook を使う(行の id を投稿の id の形で渡す)。
  const anchorRows = useMemo(() => rows.map((row) => ({ post: { object_id: row.channel_id } })), [rows]);
  const { listRef, loadMore } = useWindowScrollAnchor(anchorRows, loadNext, loadingMore);
  const { sentinelRef, canAutoLoad, manualFallback } = useInfiniteScrollSentinel({
    hasMore: Boolean(cursor) && !state.loadMoreFailed,
    loadingMore,
    onLoadMore: loadMore,
  });

  if (!actions) return null;
  // 名前は行の作成者の profile(手元に無ければ背景で読まれ、次の読み直しで付く)、無ければ投稿などで知った名前。
  const creatorLabel = (row: PublicChannelView) => {
    const author = knownAuthorsByPubkey[row.creator_pubkey];
    return authorDisplayLabel(
      row.creator_pubkey,
      row.creator_display_name ?? author?.display_name,
      row.creator_name ?? author?.name
    );
  };
  return (
    <div className='shell-column-content'>
      {state.status === 'loading' ? <Notice>{t('shell:publicThreads.list.loading')}</Notice> : null}
      {state.status === 'error' ? <Notice tone='destructive'>{t('shell:publicThreads.list.failed')}</Notice> : null}
      {state.status === 'ready' && rows.length === 0 && !cursor && state.unavailable === 0 ? (
        <EmptyStateGuidance
          title={t('shell:publicThreads.list.empty')}
          steps={[{ id: 'create', content: t('shell:publicThreads.list.emptyHint') }]}
          actions={[{ id: 'create', label: t('shell:publicThreads.create.action'), onClick: () => actions.create(topic) }]}
        />
      ) : null}
      <ul ref={listRef} className='public-thread-list' aria-label={t('shell:publicThreads.listTitle')}>
        {state.slid ? (
          <li>
            <Button
              variant='secondary'
              type='button'
              className='timeline-feed-refresh-banner'
              onClick={() => {
                listRef.current?.closest('.shell-column-body')?.scrollTo({ top: 0 });
                void loadHead(true);
              }}
            >
              {t('common:feed.backToLatest')}
            </Button>
          </li>
        ) : null}
        {rows.map((row) => (
          <li key={row.channel_id} data-post-id={row.channel_id}>
            {/* 文言の間の空白は、読み上げの名前を区切る(grid・flex の中なので見た目は変えない)。 */}
            <button type='button' className='public-thread-row' onClick={() => actions.open(topic, row.channel_id)}>
              <strong className='public-thread-row-title'>{row.title}</strong>{' '}
              <span className='public-thread-row-meta'>
                <span>{creatorLabel(row)}</span>{' '}
                <span>
                  {t(order === 'created' ? 'shell:publicThreads.list.createdAt' : 'shell:publicThreads.list.lastPostAt', {
                    time: formatLocalizedDateTime(order === 'created' ? row.created_at : row.last_post_at),
                  })}
                </span>
              </span>
            </button>
          </li>
        ))}
        {state.unavailable > 0 ? (
          <li>
            <p className='empty' role='status'>
              {t('shell:publicThreads.list.unavailable', { count: state.unavailable })}
            </p>
          </li>
        ) : null}
        {cursor ? (
          <li>
            {state.loadMoreFailed ? <p className='error'>{t('shell:publicThreads.list.loadMoreFailed')}</p> : null}
            {canAutoLoad && !state.loadMoreFailed ? <div ref={sentinelRef} aria-hidden='true' /> : null}
            {!canAutoLoad || manualFallback || state.loadMoreFailed ? (
              <Button variant='secondary' type='button' disabled={loadingMore} onClick={loadMore}>
                {loadingMore
                  ? t('common:fallbacks.loadingMore')
                  : state.loadMoreFailed
                    ? t('common:actions.retry')
                    : t('common:fallbacks.loadMore')}
              </Button>
            ) : null}
          </li>
        ) : null}
      </ul>
    </div>
  );
}

/** 一覧の列の上部の並び順の menu(タイムラインの表示の切替と同じ形)。並びは列ごとに保存する(判断 2)。 */
export function PublicThreadOrderMenu({ column }: { column: ColumnState }) {
  const { t } = useTranslation('shell');
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const [open, setOpen] = useState(false);
  const order: PublicChannelOrder = column.publicThreadOrder ?? 'last_post';
  const Current = ORDERS.find((item) => item.value === order)?.icon ?? MessageSquareText;
  return (
    <div className='shell-column-view-tabs'>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <IconButton
            variant='ghost'
            className='shell-column-view-tab'
            type='button'
            label={t('publicThreads.order.active', { order: t(`publicThreads.order.${order}`) })}
            aria-haspopup='menu'
          >
            <Current className='size-4' aria-hidden='true' />
          </IconButton>
        </PopoverTrigger>
        <PopoverMenuContent
          aria-label={t('publicThreads.order.label')}
          align='start'
          className='public-thread-order-menu space-y-1'
        >
          {ORDERS.map(({ value, icon: Icon }) => (
            <Button
              key={value}
              role='menuitemradio'
              aria-checked={order === value}
              variant='ghost'
              className='w-full justify-start'
              onClick={() => {
                setOpen(false);
                setWorkspaceState((current) => setColumnPublicThreadOrder(current, column.id, value));
              }}
            >
              <Icon className='size-4 shrink-0' aria-hidden='true' />
              {t(`publicThreads.order.${value}`)}
              {order === value ? <Check className='ml-auto size-4 shrink-0' aria-hidden='true' /> : null}
            </Button>
          ))}
        </PopoverMenuContent>
      </Popover>
    </div>
  );
}

/** 公開スレッドの詳細の列の上部。リンクをコピーし(判断 5)、タイトルを知らなければ表示中に ID から読む。読めなければその旨を出す。 */
export function PublicThreadScopeActions({ topic, channelId }: { topic: string; channelId: string }) {
  const { t } = useTranslation(['common', 'shell']);
  const actions = useContext(PublicThreadActionsContext);
  const load = actions?.load;
  const { visible } = useColumnRuntime();
  const known = useDesktopShellStore(
    (state) => knownPublicThreadTitle(state.publicThreadsById, state.knownAuthorsByPubkey, channelId) !== undefined
  );
  const [unavailable, setUnavailable] = useState(false);
  useEffect(() => {
    if (!load || known || !visible) return;
    let cancelled = false;
    void load(topic, channelId).then((thread) => {
      if (!cancelled) setUnavailable(!thread);
    });
    return () => {
      cancelled = true;
    };
  }, [load, known, visible, topic, channelId]);
  if (!actions) return null;
  return (
    <div className='shell-column-scope-actions'>
      {unavailable && !known ? (
        <span className='shell-column-header-summary'>{t('shell:publicThreads.titleUnavailable')}</span>
      ) : null}
      <Button
        variant='ghost'
        size='sm'
        type='button'
        className='shell-column-scope-action'
        onClick={() => actions.copyLink(topic, channelId)}
      >
        <Link2 className='size-4' aria-hidden='true' />
        {t('common:actions.copyLink')}
      </Button>
    </div>
  );
}

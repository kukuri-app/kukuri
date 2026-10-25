import { CalendarPlus, Check, EyeOff, Link2, MessageSquareText, MoreHorizontal, Pencil, Radio, Trash2 } from 'lucide-react';
import { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { EmptyStateGuidance } from '@/components/core/EmptyStateGuidance';
import { MediaAssetActions } from '@/shell/page/MediaAssetActions';
import { MediaDemandObserver } from '@/components/core/MediaDemandObserver';
import type { PostMediaView } from '@/components/core/types';
import { useInfiniteScrollSentinel } from '@/components/core/useInfiniteScrollSentinel';
import { useOnScreen } from '@/components/core/useOnScreen';
import { useWindowScrollAnchor } from '@/components/core/useWindowScrollAnchor';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Notice } from '@/components/ui/notice';
import { Popover, PopoverMenuContent, PopoverTrigger } from '@/components/ui/popover';
import { formatLocalizedDateTime } from '@/i18n/format';
import type { PostView, PublicChannelOrder, PublicChannelView } from '@/lib/api';
import { useColumnRuntime } from '@/shell/ColumnRuntimeContext';
import { VISIBLE_TIMELINE_LIMIT } from '@/shell/pagination';
import { authorDisplayLabel, translateLiveStatus } from '@/shell/presentation';
import { isHiddenAuthor, knownPublicThreadTitle } from '@/shell/slices/publicThreads';
import { setColumnPublicThreadOrder, type ColumnState } from '@/shell/slices/workspace';
import { REFRESH_INTERVAL_MS, useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';

import { PublicThreadActionsContext, useLiveSurfacesEnabled } from './publicThreadActions';
import { INITIAL_PUBLIC_THREAD_LIST, withHeadPage, withNextPage } from './publicThreadListState';

const ORDERS = [
  { value: 'last_post', icon: MessageSquareText },
  { value: 'created', icon: CalendarPlus },
] as const;

/** 投稿の画像の表示(投稿 card と同じ判定。成人向けとして伏せる間は `gated`)。 */
type PostMediaBuilder = (post: PostView) => PostMediaView;

/**
 * topic の公開スレッドの一覧の列の本体(#1840)。表示中の間だけ先頭のページを読み直す。並びを変えたら作り直す(key)。
 * `postMedia` があれば、行に最初の投稿のサムネイルを出す(#1900)。
 */
export function PublicThreadList({ column, postMedia }: { column: ColumnState; postMedia?: PostMediaBuilder }) {
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
  // 作成者を非表示にした行(mute・block)と、削除を知った行(#1888)は、次に読んだページが除く前でも出さない(読み込んだ
  // 続きの行を含む)。
  const threadsById = useDesktopShellStore((state) => state.publicThreadsById);
  const rows = useMemo(
    () =>
      state.rows.filter(
        (row) => !isHiddenAuthor(knownAuthorsByPubkey[row.creator_pubkey]) && !threadsById[row.channel_id]?.deleted
      ),
    [state.rows, knownAuthorsByPubkey, threadsById]
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
          <PublicThreadRow key={row.channel_id} topic={topic} order={order} row={row} postMedia={postMedia} />
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

/**
 * 一覧の 1 行。名前は行の作成者の profile(手元に無ければ背景で読まれ、次の読み直しで付く)、無ければ投稿などで知った名前。
 * 右に最初の投稿の 1 枚目の画像(無ければ動画の静止画)を出す。最初の投稿が手元に無い行は、画面に出たら ID から読み、
 * 読めたら覚えたスレッドから出す(#1900)。
 */
function PublicThreadRow({
  topic,
  order,
  row,
  postMedia,
}: {
  topic: string;
  order: PublicChannelOrder;
  row: PublicChannelView;
  postMedia?: PostMediaBuilder;
}) {
  const { t } = useTranslation('shell');
  const liveSurfaces = useLiveSurfacesEnabled();
  const actions = useContext(PublicThreadActionsContext);
  const load = actions?.load;
  const author = useDesktopShellStore((state) => state.knownAuthorsByPubkey[row.creator_pubkey]);
  const firstPost = useDesktopShellStore((state) => state.publicThreadsById[row.channel_id]?.first_post) ?? row.first_post;
  const ref = useRef<HTMLLIElement>(null);
  const onScreen = useOnScreen(ref, row.channel_id);
  const missing = Boolean(postMedia && row.first_post_id && !firstPost);
  useEffect(() => {
    if (load && missing && onScreen) void load(topic, row.channel_id);
  }, [load, missing, onScreen, topic, row.channel_id]);
  if (!actions) return null;
  const media = firstPost && postMedia ? postMedia(firstPost) : null;
  return (
    <li ref={ref} data-post-id={row.channel_id} className='public-thread-row-item'>
      {/* 文言の間の空白は、読み上げの名前を区切る(grid・flex の中なので見た目は変えない)。 */}
      <button type='button' className='public-thread-row' onClick={() => actions.open(topic, row.channel_id)}>
        <strong className='public-thread-row-title'>{row.title}</strong>{' '}
        <span className='public-thread-row-meta'>
          <span>
            {authorDisplayLabel(
              row.creator_pubkey,
              row.creator_display_name ?? author?.display_name,
              row.creator_name ?? author?.name
            )}
          </span>{' '}
          <span>
            {t(order === 'created' ? 'publicThreads.list.createdAt' : 'publicThreads.list.lastPostAt', {
              time: formatLocalizedDateTime(order === 'created' ? row.created_at : row.last_post_at),
            })}
          </span>
          {/* 配信中の印(判断 2。#1840 AC-4)。 */}
          {liveSurfaces && row.live_now ? (
            <>
              {' '}
              <span className='public-thread-row-live'>
                <Radio className='size-3.5 shrink-0' aria-hidden='true' />
                {translateLiveStatus('Live')}
              </span>
            </>
          ) : null}
        </span>
        {media ? <span className='public-thread-thumbnail-space' aria-hidden='true' /> : null}
      </button>
      {media ? <PublicThreadThumbnail media={media} onOpen={() => actions.open(topic, row.channel_id)} /> : null}
    </li>
  );
}

/** 行の右のサムネイル。成人向けとして伏せる間は印だけを出し、画像を参照しない(#1900 判断 2)。照会中は枠だけを出す。 */
function PublicThreadThumbnail({ media, onOpen }: { media: PostMediaView; onOpen: () => void }) {
  const { t } = useTranslation('shell');
  if (media.state === 'gated') {
    return (
      <span className='public-thread-row-thumbnail' role='img' aria-label={t('publicThreads.list.thumbnailGated')}>
        <EyeOff className='size-5' aria-hidden='true' />
      </span>
    );
  }
  if (media.state === 'pending') return <span className='public-thread-row-thumbnail' aria-hidden='true' />;
  const shown =
    media.kind === 'image'
      ? media.imageGalleryItems?.[0]
      : media.videoPosterHash
        ? { hash: media.videoPosterHash, src: media.videoPosterPreviewSrc }
        : undefined;
  if (!shown) return null;
  const attachment = media.kind === 'image' ? media.imageGalleryItems?.[0]?.attachment : media.posterAttachment;
  const preview = <><MediaDemandObserver hash={shown.hash} />{shown.src ? <img src={shown.src} alt='' /> : null}</>;
  if (attachment && media.bookmarkTarget) return <span className='public-thread-row-thumbnail'>
    <MediaAssetActions attachment={attachment} target={media.bookmarkTarget} bookmarkHash={media.kind === 'video' ? media.videoAttachment?.hash : undefined}><button type='button' className='media-image-trigger' aria-label={t('common:media.imageAlt')} onClick={onOpen}>{preview}</button></MediaAssetActions>
  </span>;
  return (
    <span className='public-thread-row-thumbnail' aria-hidden='true'>
      <MediaDemandObserver hash={shown.hash} />
      {shown.src ? <img src={shown.src} alt='' /> : null}
    </span>
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

/**
 * 公開スレッドの詳細の列の上部。リンクをコピーし(判断 5)、表示中は同期の時刻が変わるたびに ID から読み直す(タイトルの変更と
 * 削除を取り込む。#1888)。タイトルを知らず、読めなければその旨を出す。「ライブ」はスレッドの「ライブ」の列を開く(判断 15。
 * #1840 AC-4)。作成者には、タイトルの変更と削除の menu を出す(#1888)。削除したスレッドには操作を出さない。
 */
export function PublicThreadScopeActions({ topic, channelId }: { topic: string; channelId: string }) {
  const { t } = useTranslation(['common', 'shell']);
  const actions = useContext(PublicThreadActionsContext);
  const refresh = actions?.refresh;
  const load = actions?.load;
  const { visible } = useColumnRuntime();
  const lastSync = useDesktopShellStore((state) => state.syncStatus.last_sync_ts);
  const localPubkey = useDesktopShellStore((state) => state.syncStatus.local_author_pubkey);
  const thread = useDesktopShellStore((state) => state.publicThreadsById[channelId]);
  const known = useDesktopShellStore(
    (state) => knownPublicThreadTitle(state.publicThreadsById, state.knownAuthorsByPubkey, channelId) !== undefined
  );
  const [unavailable, setUnavailable] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const read = useCallback(
    (reader: typeof load) => {
      if (!reader || !visible) return;
      let cancelled = false;
      void reader(topic, channelId).then((value) => {
        if (!cancelled) setUnavailable(!value);
      });
      return () => {
        cancelled = true;
      };
    },
    [visible, topic, channelId]
  );
  // 先に読み直し、知らない(store から外れた)ときは、その読み込みか使い回しの値で覚え直す。
  useEffect(() => read(refresh), [read, refresh, lastSync]);
  useEffect(() => (known ? undefined : read(load)), [read, load, known]);
  if (!actions || thread?.deleted) return null;
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
      {thread && thread.creator_pubkey === localPubkey ? (
        <Popover open={menuOpen} onOpenChange={setMenuOpen}>
          <PopoverTrigger asChild>
            <IconButton
              variant='ghost'
              type='button'
              className='shell-column-scope-action'
              label={t('shell:publicThreads.manage')}
              aria-haspopup='menu'
            >
              <MoreHorizontal className='size-4' aria-hidden='true' />
            </IconButton>
          </PopoverTrigger>
          <PopoverMenuContent aria-label={t('shell:publicThreads.manage')} align='end' className='space-y-1'>
            {(
              [
                ['rename', Pencil, () => actions.rename(thread)],
                ['delete', Trash2, () => actions.remove(thread)],
              ] as const
            ).map(([key, Icon, run]) => (
              <Button
                key={key}
                variant='ghost'
                size='sm'
                type='button'
                className='w-full justify-start'
                onClick={() => {
                  setMenuOpen(false);
                  run();
                }}
              >
                <Icon className='size-4 shrink-0' aria-hidden='true' />
                {t(`shell:publicThreads.${key}.action`)}
              </Button>
            ))}
          </PopoverMenuContent>
        </Popover>
      ) : null}
    </div>
  );
}

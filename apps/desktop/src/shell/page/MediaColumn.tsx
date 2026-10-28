import { useContext, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { Bookmark, CircleUserRound, HardDrive, Image, Trash2 } from 'lucide-react';
import type { BookmarkedMediaView, MediaBookmarkCursor, PostView, TimelineCursor } from '@/lib/api';
import { IconButton } from '@/components/ui/icon-button';
import { MediaBookmarksContext, MEDIA_BOOKMARK_CHANGED } from '@/components/core/mediaBookmarksContext';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { setColumnMediaShelf, type ColumnMediaShelf, type ColumnState } from '@/shell/slices/workspace';
import { MediaShelfView } from '@/shell/page/MediaShelfView';
import { useMediaShelf } from '@/shell/page/useMediaShelf';

const SHELVES = [
  { id: 'cache', icon: HardDrive },
  { id: 'bookmarks', icon: Bookmark },
  { id: 'posts', icon: CircleUserRound },
] as const satisfies ReadonlyArray<{ id: ColumnMediaShelf; icon: unknown }>;
/** 「投稿」の 1 ページは、自分のプロフィールの投稿を 20 件ずつ最大 5 回読んで、画像・動画を 20 件まで集める。 */
const POST_READ_LIMIT = 20;
const POST_READS_PER_PAGE = 5;
const PAGE_ITEMS = 20;

/** ヘッダーの切替(タイムラインの表示の切替と同じ形の tab)。選んだ表示は列ごとに保存する。 */
export function MediaShelfTabs({ column }: { column: ColumnState }) {
  const { t } = useTranslation('shell');
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const tabs = useRef<Array<HTMLButtonElement | null>>([]);
  const active = column.mediaShelf ?? 'cache';
  const select = (index: number) => {
    setWorkspaceState((current) => setColumnMediaShelf(current, column.id, SHELVES[index].id));
    tabs.current[index]?.focus();
  };
  const move = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const next = { ArrowRight: index + 1, ArrowDown: index + 1, ArrowLeft: index - 1, ArrowUp: index - 1, Home: 0, End: SHELVES.length - 1 }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    select((next + SHELVES.length) % SHELVES.length);
  };
  return <div className='shell-column-view-tabs'>
    <div className='shell-column-view-tablist' role='tablist' aria-label={t('mediaColumn.shelves')}>
      {SHELVES.map(({ id, icon: Icon }, index) => <IconButton key={id} variant={active === id ? 'secondary' : 'ghost'} className='shell-column-view-tab' role='tab' type='button'
        label={t(`mediaColumn.${id}.title`)} aria-selected={active === id} tabIndex={active === id ? 0 : -1} ref={(node) => { tabs.current[index] = node; }}
        onClick={() => select(index)} onKeyDown={(event) => move(event, index)}><Icon className='size-4' aria-hidden='true' /></IconButton>)}
    </div>
  </div>;
}

/** 自分の投稿の画像・動画。画像は 1 枚ずつ、動画は本体と poster の組(ブックマークの行と同じ形)。 */
function postMediaItems(post: PostView): BookmarkedMediaView[] {
  const row = (attachments: PostView['attachments']) => ({ hash: attachments[0].hash, attachments, content_labels: post.content_labels ?? [], bookmarked_at: post.created_at,
    bookmarked_from_object_id: post.object_id, source: { topic_id: post.published_topic_id ?? '', object_id: post.object_id, author_pubkey: post.author_pubkey, created_at: post.created_at } });
  const posters = post.attachments.filter((asset) => asset.role === 'video_poster');
  return [
    ...post.attachments.filter((asset) => asset.role === 'image_original' && asset.mime.startsWith('image/')).map((asset) => row([asset])),
    ...post.attachments.filter((asset) => asset.role === 'video_manifest').map((video, index) => row(posters[index] ? [video, posters[index]] : [video])),
  ];
}

function MediaShelfBody({ column, shelfId }: { column: ColumnState; shelfId: ColumnMediaShelf }) {
  const { t } = useTranslation(['shell', 'common']);
  const context = useContext(MediaBookmarksContext);
  const api = context?.api;
  const localAuthor = useDesktopShellStore((state) => state.syncStatus.local_author_pubkey);
  const [busy, setBusy] = useState(false);
  // 「投稿」の各ページの読み始めの位置(プロフィールの読む位置と、その 1 回分の画像・動画の何件目からか)。ページの位置は
  // 1 から数えたこの配列の番号。
  const starts = useRef<Array<{ at: TimelineCursor | null; skip: number } | null>>([{ at: null, skip: 0 }]);
  // 表示中の「投稿」の行の元の投稿(取り下げに使う)。
  const posts = useRef(new Map<string, PostView>());
  const shelf = useMediaShelf<MediaBookmarkCursor | number>(column.id, true, async (cursor, before) => {
    if (shelfId !== 'posts') return api!.listBookmarkedMedia(cursor as MediaBookmarkCursor | null, before, shelfId);
    const page = (cursor as number | null) ?? 1;
    let { at, skip } = starts.current[page - 1] ?? { at: null, skip: 0 };
    const items: BookmarkedMediaView[] = [];
    let next = null;
    for (let read = 0; read < POST_READS_PER_PAGE; read += 1) {
      const timeline = await api!.listProfileTimeline(localAuthor, at, POST_READ_LIMIT);
      const media = timeline.items.filter((post) => post.author_pubkey === localAuthor && !post.repost_of && !post.withdrawal).flatMap((post) => {
        posts.current.set(post.object_id, post);
        return postMediaItems(post);
      });
      const taken = media.slice(skip, skip + PAGE_ITEMS - items.length);
      items.push(...taken);
      // 20 件で切り、残りは同じ読む位置の続きから次のページにする。
      next = skip + taken.length < media.length ? { at, skip: skip + taken.length } : timeline.next_cursor ? { at: timeline.next_cursor, skip: 0 } : null;
      if (!next || items.length === PAGE_ITEMS) break;
      ({ at, skip } = next);
    }
    starts.current[page] = next;
    return { items, newer_cursor: page > 1 ? page - 1 : null, older_cursor: next ? page + 1 : null };
  });
  useEffect(() => {
    if (api) void shelf.load();
    // 表示を切り替えたときは、この部品を作り直して最初のページから読む。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const remove = async (item: BookmarkedMediaView) => {
    if (!api || busy) return;
    if (shelfId === 'posts' && !window.confirm(t('common:actions.confirmWithdrawPost'))) return;
    setBusy(true);
    try {
      if (shelfId === 'posts') {
        const post = posts.current.get(item.bookmarked_from_object_id);
        if (post && await context?.withdrawPost?.(post)) {
          shelf.drop(shelf.page?.items.filter((row) => row.bookmarked_from_object_id === post.object_id).map((row) => row.hash) ?? []);
        }
      } else {
        await api.removeBookmarkedMedia(item.hash, shelfId);
        if (shelfId === 'bookmarks') window.dispatchEvent(new CustomEvent(MEDIA_BOOKMARK_CHANGED, { detail: { hash: item.hash, saved: false } }));
        shelf.drop([item.hash]);
      }
    } catch { shelf.setFailed(true); }
    finally { setBusy(false); }
  };
  if (!api) return null;
  const removeLabel = { cache: t('mediaColumn.cache.remove'), bookmarks: t('common:mediaBookmarks.remove'), posts: t('common:actions.withdrawPost') }[shelfId];
  return <MediaShelfView shelf={shelf} labels={{ loading: t('mediaColumn.loading'), empty: t(`mediaColumn.${shelfId}.empty`), loadFailed: t('mediaColumn.loadFailed') }} placeholder={<Image aria-hidden='true' />}
    tile={(item, thumbnail) => <>
      <span className='media-shelf-thumbnail' role='img' aria-label={t(item.attachments.some((asset) => asset.role === 'video_manifest') ? 'common:media.video' : 'common:media.imageAlt')}>{thumbnail}</span>
      <IconButton className='media-bookmark-button' type='button' variant='ghost' label={removeLabel} disabled={shelf.loading || busy} onClick={() => void remove(item)}>
        {shelfId === 'bookmarks' ? <Bookmark className='size-4 fill-current' aria-hidden='true' /> : <Trash2 className='size-4' aria-hidden='true' />}
      </IconButton>
    </>} />;
}

/** 手元の画像・動画の列(#1949)。キャッシュ・ブックマーク・自分の投稿を 20 件ずつ置き換えて表示し、そこから消せる。 */
export function MediaColumn({ column }: { column: ColumnState }) {
  const shelfId = column.mediaShelf ?? 'cache';
  return <div className='shell-column-content media-column'><MediaShelfBody key={shelfId} column={column} shelfId={shelfId} /></div>;
}

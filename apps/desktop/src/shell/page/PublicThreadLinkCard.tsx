import { useContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useOnScreen } from '@/components/core/useOnScreen';
import type { PublicChannelView } from '@/lib/api';
import { isHiddenAuthor } from '@/shell/slices/publicThreads';
import { useDesktopShellStore } from '@/shell/store';

import { PublicThreadActionsContext } from './publicThreadActions';

type CardState =
  | { status: 'loading' }
  | { status: 'available'; thread: PublicChannelView; firstPost: string | null }
  | { status: 'unavailable' };

/** 本文の中の公開スレッドのリンクの card。タイトルと作成時の最初の投稿を、画面内にある間だけ読む(#1840 判断 10)。 */
export function PublicThreadLinkCard({ topic, channelId }: { topic: string; channelId: string }) {
  const { t } = useTranslation('shell');
  const actions = useContext(PublicThreadActionsContext);
  const api = actions?.api;
  const load = actions?.load;
  const ref = useRef<HTMLDivElement>(null);
  const onScreen = useOnScreen(ref, channelId);
  const [loaded, setState] = useState<CardState>({ status: 'loading' });
  // 読めた後は、覚えた最新の値(タイトルの変更と削除。#1888)で出す。
  const remembered = useDesktopShellStore((store) => store.publicThreadsById[channelId]);
  const current: CardState = loaded.status === 'available' && remembered ? { ...loaded, thread: remembered } : loaded;
  // 作成者を非表示にしたら(mute・block)、読めたスレッドも出さない(一覧と同じ)。
  const creator = current.status === 'available' ? current.thread.creator_pubkey : null;
  const creatorHidden = useDesktopShellStore((store) =>
    creator ? isHiddenAuthor(store.knownAuthorsByPubkey[creator]) : false
  );
  const state: CardState = creatorHidden ? { status: 'unavailable' } : current;
  const deleted = state.status === 'available' && state.thread.deleted;
  useEffect(() => {
    if (!api || !load || !onScreen) return;
    let cancelled = false;
    void (async () => {
      try {
        const thread = await load(topic, channelId);
        if (cancelled) return;
        if (!thread) {
          setState({ status: 'unavailable' });
          return;
        }
        // 最初の投稿は、スレッドの取得が手元へ読む(作成者を非表示にしていれば返信ツリーが返さない)。
        const root = thread.first_post_id
          ? (await api.listThread(topic, thread.first_post_id, null, 1)).items.find(
              (post) => post.object_id === thread.first_post_id
            )
          : undefined;
        if (cancelled) return;
        setState({ status: 'available', thread, firstPost: root?.content ?? null });
      } catch {
        if (!cancelled) setState({ status: 'unavailable' });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [api, load, onScreen, topic, channelId]);
  if (!actions) return null;
  return (
    <div ref={ref} className='link-preview-slot' data-state={state.status}>
      <button
        type='button'
        className='link-preview-card public-thread-card'
        onClick={(event) => {
          event.stopPropagation();
          actions.open(topic, channelId);
        }}
        onKeyDown={(event) => event.stopPropagation()}
      >
        {/* 文言の間の空白は、読み上げの名前を区切る(flex の中なので見た目は変えない)。 */}
        <span className='link-preview-copy'>
          <span className='link-preview-source'>{t('publicThreads.title')}</span>{' '}
          <strong className='link-preview-title'>
            {state.status === 'available'
              ? deleted
                ? t('publicThreads.deleted')
                : state.thread.title
              : t(state.status === 'loading' ? 'publicThreads.card.loading' : 'publicThreads.card.unavailable')}
          </strong>{' '}
          {state.status === 'available' && state.firstPost && !deleted ? (
            <span className='link-preview-description'>{state.firstPost}</span>
          ) : null}
        </span>
      </button>
    </div>
  );
}

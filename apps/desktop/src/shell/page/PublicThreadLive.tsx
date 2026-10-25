import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useContext, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import { IconButton } from '@/components/ui/icon-button';
import { isHiddenAuthor } from '@/shell/slices/publicThreads';
import { timelineStorageKeyForChannel, useDesktopShellStore } from '@/shell/store';

import { LiveCard } from './LiveCard';
import { PublicThreadActionsContext } from './publicThreadActions';

/** スレッドの配信中ライブ。カルーセル内で画像を選び、詳細を確認してから参加する。 */
export function PublicThreadLiveRows({ topic, channelId }: { topic: string; channelId: string }) {
  const { t } = useTranslation(['shell', 'live']);
  const actions = useContext(PublicThreadActionsContext);
  const track = useRef<HTMLUListElement>(null);
  const { sessions, gates, authors } = useDesktopShellStore(useShallow((state) => ({
    sessions: state.liveSessionsByScopeKey[timelineStorageKeyForChannel(topic, channelId)],
    gates: state.authorTrustGates,
    authors: state.knownAuthorsByPubkey,
  })));
  const live = (sessions ?? []).filter((session) => session.status === 'Live' && !gates[session.host_pubkey]?.hidden && !isHiddenAuthor(authors[session.host_pubkey]));
  if (!actions || live.length === 0) return null;
  const move = (direction: number) => track.current?.scrollBy({ left: direction * track.current.clientWidth, behavior: 'smooth' });
  return <section className='public-thread-live-carousel'>
    <div className='live-carousel-heading'>
      <strong>{t('shell:publicThreads.live.label')}</strong>
      <IconButton className='min-h-11 min-w-11' type='button' variant='ghost' label={t('live:carousel.previous')} onClick={() => move(-1)}><ChevronLeft className='size-4' aria-hidden='true' /></IconButton>
      <IconButton className='min-h-11 min-w-11' type='button' variant='ghost' label={t('live:carousel.next')} onClick={() => move(1)}><ChevronRight className='size-4' aria-hidden='true' /></IconButton>
    </div>
    <ul ref={track} className='live-carousel-track' aria-label={t('shell:publicThreads.live.label')}>
      {live.map((session) => <li key={session.session_id}>
        <LiveCard topic={topic} session={session} onStage={() => actions.openLive(topic, channelId, session.session_id)} />
      </li>)}
    </ul>
  </section>;
}

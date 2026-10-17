import { Radio } from 'lucide-react';
import { useContext } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import { authorDisplayLabel, translateLiveStatus } from '@/shell/presentation';
import { isHiddenAuthor, SHOWN_THREAD_LIVE_SESSIONS } from '@/shell/slices/publicThreads';
import { timelineStorageKeyForChannel, useDesktopShellStore } from '@/shell/store';

import { PublicThreadActionsContext } from './publicThreadActions';

/**
 * #1840 AC-4: 公開スレッドの詳細の列の上部の、そのスレッドで始めた配信中の Live(判断 6・14)。タイトルと配信者を 1 行ずつ、
 * 新しく始まった順に 3 件まで出し、押すとスレッドの「ライブ」の列でその Live を示す。行は「ライブ」の列と同じく、公開
 * スレッドの scope の `list_live_sessions` から作る。主催者を非表示にしたもの(mute・block、CN の評価で非表示を勧めるもの)
 * は出さない(CN の評価は「ライブ」の列で確かめて表示できる)。
 */
export function PublicThreadLiveRows({ topic, channelId }: { topic: string; channelId: string }) {
  const { t } = useTranslation('shell');
  const actions = useContext(PublicThreadActionsContext);
  const { sessions, gates, authors, me } = useDesktopShellStore(
    useShallow((state) => ({
      sessions: state.liveSessionsByScopeKey[timelineStorageKeyForChannel(topic, channelId)],
      gates: state.authorTrustGates,
      authors: state.knownAuthorsByPubkey,
      me: state.localProfile,
    }))
  );
  const live = (sessions ?? []).filter(
    (session) =>
      session.status === 'Live' && !gates[session.host_pubkey]?.hidden && !isHiddenAuthor(authors[session.host_pubkey])
  );
  if (!actions || live.length === 0) return null;
  return (
    <ul className='public-thread-lives' aria-label={t('publicThreads.live.label')}>
      {live.slice(0, SHOWN_THREAD_LIVE_SESSIONS).map((session) => {
        // 名前は手元の作者の情報から(上部に出す主催者は shell が読む)。自分の Live は自分の profile。
        const host = session.host_pubkey === me?.pubkey ? me : authors[session.host_pubkey];
        return (
          <li key={session.session_id}>
            {/* 文言の間の空白は、読み上げの名前を区切る。 */}
            <button
              type='button'
              className='public-thread-live'
              onClick={() => actions.openLive(topic, channelId, session.session_id)}
            >
              <Radio className='size-4 shrink-0' aria-hidden='true' />
              <span className='public-thread-live-status'>{translateLiveStatus(session.status)}</span>{' '}
              <span className='public-thread-live-title'>{session.title}</span>{' '}
              <span className='public-thread-live-host'>
                {authorDisplayLabel(session.host_pubkey, host?.display_name, host?.name)}
              </span>
            </button>
          </li>
        );
      })}
      {live.length > SHOWN_THREAD_LIVE_SESSIONS ? (
        <li>
          <button type='button' className='public-thread-live-all' onClick={() => actions.openLive(topic, channelId, null)}>
            {t('publicThreads.live.all')}
          </button>
        </li>
      ) : null}
    </ul>
  );
}

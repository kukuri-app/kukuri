import { type ReactNode, useContext, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import { AuthorTrustGateNotice } from '@/components/core/AuthorTrustGateNotice';
import { useAuthorTrustGateReveal } from '@/components/core/useAuthorTrustGateReveal';
import { Button } from '@/components/ui/button';
import { Notice } from '@/components/ui/notice';
import type { ColumnState } from '@/shell/slices/workspace';
import { timelineStorageKeyForChannel, useDesktopShellStore } from '@/shell/store';

import { LiveCard } from './LiveCard';
import { LiveActionsContext } from './liveActions';

/** 個別ライブの拡大表示。補助ペインは利用者が半分表示を選んだときだけ描画する。 */
export function LiveColumn({ column, children, thread, composer }: {
  column: ColumnState;
  children: ReactNode;
  thread: ReactNode;
  composer: ReactNode;
}) {
  const { t } = useTranslation('live');
  const [split, setSplit] = useState(false);
  const [tab, setTab] = useState<'lives' | 'thread'>('lives');
  if (!column.entityId) return <Notice>{t('stage.selectLive')}</Notice>;
  return <div className='live-column' data-layout={split ? 'split' : 'full'}>
    <div className='live-column-layout-controls' role='group' aria-label={t('stage.layout')}>
      <Button type='button' variant='secondary' size='sm' aria-pressed={!split} onClick={() => setSplit(false)}>{t('stage.full')}</Button>
      <Button type='button' variant='secondary' size='sm' aria-pressed={split} onClick={() => setSplit(true)}>{t('stage.split')}</Button>
    </div>
    <div className='shell-stream-layout'>
      {children}
      {split ? <section className='live-column-side'>
        <div className='live-column-tabs' role='tablist' aria-label={t('stage.sidePanel')}>
          {(['lives', 'thread'] as const).map((value) => <Button key={value} id={`${column.id}-${value}-tab`}
            role='tab' type='button' aria-selected={tab === value} aria-controls={`${column.id}-live-panel`}
            tabIndex={tab === value ? 0 : -1}
            onKeyDown={(event) => {
              if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
              event.preventDefault();
              const next = event.key === 'Home' ? 'lives' : event.key === 'End' ? 'thread' : tab === 'lives' ? 'thread' : 'lives';
              setTab(next);
              document.getElementById(`${column.id}-${next}-tab`)?.focus();
            }}
            variant={tab === value ? 'primary' : 'secondary'} onClick={() => setTab(value)}>
            {t(value === 'lives' ? 'stage.otherLives' : 'stage.publicThread')}
          </Button>)}
        </div>
        <div className='live-column-side-content' id={`${column.id}-live-panel`} role='tabpanel' aria-labelledby={`${column.id}-${tab}-tab`}>
          {tab === 'lives' ? <OtherThreadLives column={column} /> : <>{thread}<div className='live-column-composer'>{composer}</div></>}
        </div>
      </section> : null}
    </div>
  </div>;
}

function OtherThreadLives({ column }: { column: ColumnState }) {
  const { t } = useTranslation('live');
  const actions = useContext(LiveActionsContext);
  const scope = column.scope!;
  const key = timelineStorageKeyForChannel(scope.topicId, scope.channelId);
  const { sessions, panel, gates, error } = useDesktopShellStore(useShallow((state) => ({
    sessions: state.liveSessionsByScopeKey[key],
    panel: state.livePanelStateByScopeKey[key],
    gates: state.authorTrustGates,
    error: state.liveError,
  })));
  const trust = useAuthorTrustGateReveal(gates);
  const others = (sessions ?? []).filter((session) => session.status === 'Live' && session.session_id !== column.entityId);
  if (!actions) return null;
  return <div className='shell-column-content'>
    {panel?.status === 'loading' ? <Notice>{t('loading')}</Notice> : null}
    {panel?.status === 'error' ? <Notice tone='destructive'>{error ?? panel.error}</Notice> : null}
    {panel?.status === 'ready' && others.length === 0 ? <p role='status'>{t('stage.noOtherLives')}</p> : null}
    <ul className='post-list'>
      {others.map((session) => {
        const gate = trust.gateFor(session.host_pubkey);
        return <li key={session.session_id}>{gate ? <AuthorTrustGateNotice gate={gate}
          onReveal={() => trust.reveal(gate.authorPubkey)} onOpenAuthor={() => undefined} /> :
          <LiveCard topic={scope.topicId} session={session}
            onStage={() => actions.stage({ session_id: session.session_id, topic_id: scope.topicId, channel_id: scope.channelId }, scope, column)} />
        }</li>;
      })}
    </ul>
  </div>;
}

import { Gamepad2, LogIn, LogOut, Radio, Square, SquarePen } from 'lucide-react';
import { useContext } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { PublicThreadActionsContext } from '@/shell/page/publicThreadActions';
import { timelineStorageKeyForChannel, useDesktopShellStore } from '@/shell/store';
import type { ColumnState } from '@/shell/slices/workspace';

type ColumnDomainActionFooterProps = {
  active: boolean;
  column: ColumnState;
  onActivate: () => void;
  onEndLiveSession: (sessionId: string, topic: string) => Promise<void>;
  onJoinLiveSession: (sessionId: string, topic: string) => Promise<void>;
  onLeaveLiveSession: (sessionId: string, topic: string) => Promise<void>;
  onOpenGameCreate: () => void;
  onOpenLiveCreate: () => void;
};

export function ColumnDomainActionFooter({
  active,
  column,
  onActivate,
  onEndLiveSession,
  onJoinLiveSession,
  onLeaveLiveSession,
  onOpenGameCreate,
  onOpenLiveCreate,
}: ColumnDomainActionFooterProps) {
  const { t } = useTranslation(['common', 'live', 'game', 'shell']);
  const publicThreads = useContext(PublicThreadActionsContext);
  const liveSessionsByScopeKey = useDesktopShellStore((state) => state.liveSessionsByScopeKey);
  const localAuthorPubkey = useDesktopShellStore(
    (state) => state.syncStatus.local_author_pubkey
  );
  if (!column.scope) return null;
  const button = (
    label: string,
    Icon: typeof Radio,
    onClick: () => void,
    disabled = false
  ) => (
    <Button
      className='shell-column-primary-action min-h-11 min-w-11'
      variant='primary'
      size={active ? 'default' : 'icon'}
      type='button'
      aria-label={label}
      disabled={disabled}
      onClick={() => {
        onActivate();
        onClick();
      }}
    >
      <Icon className='size-4' aria-hidden='true' />
      {active ? <span>{label}</span> : null}
    </Button>
  );

  if (column.kind === 'stream') {
    if (!column.entityId) {
      return button(t('live:actions.start'), Radio, onOpenLiveCreate);
    }
    // 別の場所の紹介 post から開いた Live は、Live の場所の一覧で探す(#1841)。
    const place = column.live ?? column.scope;
    const session = liveSessionsByScopeKey[timelineStorageKeyForChannel(place.topicId, place.channelId)]?.find(
      (candidate) => candidate.session_id === column.entityId
    );
    if (!session || session.status === 'Ended') return null;
    if (session.host_pubkey === localAuthorPubkey) {
      return button(t('common:actions.end'), Square, () => {
        void onEndLiveSession(session.session_id, place.topicId);
      });
    }
    if (session.joined_by_me) {
      return button(t('common:actions.leave'), LogOut, () => {
        void onLeaveLiveSession(session.session_id, place.topicId);
      });
    }
    return button(t('common:actions.join'), LogIn, () => {
      void onJoinLiveSession(session.session_id, place.topicId);
    });
  }

  if (column.kind === 'game' && !column.entityId) {
    return button(t('game:actions.createRoom'), Gamepad2, onOpenGameCreate);
  }
  // 公開スレッドの一覧の列(#1840)。作った後は詳細の列を開く。
  if (column.kind === 'publicThreads' && publicThreads) {
    const topic = column.scope.topicId;
    return button(t('shell:publicThreads.create.action'), SquarePen, () => publicThreads.create(topic));
  }
  return null;
}

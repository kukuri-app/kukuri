import { useTranslation } from 'react-i18next';

import { VerifiedAuthorName } from '@/components/core/VerifiedAuthorName';
import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';

type ProfileOverviewPanelProps = {
  authorLabel: string;
  pubkey?: string;
  nip05?: string | null;
  username: string | null;
  about: string | null;
  picture: string | null;
  status: 'loading' | 'ready' | 'error';
  error: string | null;
  postCount: number | null;
  followingCount: number;
  followedCount: number;
  mutedCount: number;
  blockingCount: number;
  onEdit: () => void;
  onOpenFollowing: () => void;
  onOpenFollowed: () => void;
  onOpenMuted: () => void;
  onOpenBlocking: () => void;
};

export function ProfileOverviewPanel({
  authorLabel,
  pubkey,
  nip05,
  username,
  about,
  picture,
  status,
  error,
  postCount,
  followingCount,
  followedCount,
  mutedCount,
  blockingCount,
  onEdit,
  onOpenFollowing,
  onOpenFollowed,
  onOpenMuted,
  onOpenBlocking,
}: ProfileOverviewPanelProps) {
  const { t } = useTranslation('profile');

  return (
    <Card className='panel-subsection'>
      <CardHeader className='profile-overview-header'>
        <div className='profile-overview-summary'>
          <div className='profile-overview-avatar'>
            {picture ? (
              <img
                src={picture}
                alt={t('overview.avatarAlt', { name: authorLabel })}
                className='profile-overview-image'
              />
            ) : (
              <span>{authorLabel.slice(0, 1).toUpperCase()}</span>
            )}
          </div>
          <div className='profile-overview-names'>
            <h3>
              {pubkey && nip05 ? (
                <VerifiedAuthorName label={authorLabel} pubkey={pubkey} nip05={nip05} />
              ) : (
                authorLabel
              )}
            </h3>
            <small>{username?.trim() || t('overview.noUsername')}</small>
          </div>
        </div>
        <div className='post-actions'>
          <Button variant='secondary' type='button' onClick={onEdit}>
            {t('overview.edit')}
          </Button>
        </div>
      </CardHeader>

      {status === 'loading' ? <Notice>{t('overview.loading')}</Notice> : null}
      {status === 'error' && error ? <Notice tone='destructive'>{error}</Notice> : null}

      <div className='shell-main-stack'>
        <div className='profile-overview-connections'>
          <Button variant='secondary' type='button' onClick={onOpenFollowing}>
            {t('overview.followingCount', { count: followingCount })}
          </Button>
          <Button variant='secondary' type='button' onClick={onOpenFollowed}>
            {t('overview.followedCount', { count: followedCount })}
          </Button>
          <Button variant='secondary' type='button' onClick={onOpenMuted}>
            {t('overview.mutedCount', { count: mutedCount })}
          </Button>
          <Button variant='secondary' type='button' onClick={onOpenBlocking}>
            {t('overview.blockingCount', { count: blockingCount })}
          </Button>
        </div>
        <p className='lede'>{about?.trim() || t('overview.noBio')}</p>
        <div className='topic-diagnostic topic-diagnostic-secondary'>
          <span>{t('overview.postCount')}</span>
          <span>{postCount ?? '—'}</span>
        </div>
      </div>
    </Card>
  );
}

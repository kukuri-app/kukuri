import { type FormEvent, type ReactNode, useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { PublicThreadSlot, type PublicThreadSlotValue } from '@/components/core/publicThreadSlot';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import type { DesktopApi, PublicChannelView } from '@/lib/api';
import { buildPublicThreadLink } from '@/lib/internalLinks';
import { topicDisplayName } from '@/lib/topicId';
import { rememberPublicThreads } from '@/shell/slices/publicThreads';
import { useDesktopShellStore, useDesktopShellStoreApi } from '@/shell/store';

import { PublicThreadActionsContext, type PublicThreadActions } from './publicThreadActions';
import { PublicThreadLinkCard } from './PublicThreadLinkCard';

/// タイトルの上限(core の `PUBLIC_CHANNEL_MAX_TITLE_CHARS`)。
const TITLE_MAX_CHARS = 200;

type PublicThreadProvidersProps = {
  api: DesktopApi;
  open: (topic: string, channelId: string | null) => void;
  copyLink: (link: string) => void;
  children: ReactNode;
};

/** #1840: 公開スレッドの操作と本文のリンクの表示、作成の Dialog を shell に 1 か所で置く。 */
export function PublicThreadProviders({ api, open, copyLink, children }: PublicThreadProvidersProps) {
  const storeApi = useDesktopShellStoreApi();
  const [creating, setCreating] = useState<string | null>(null);
  const [lastCreated, setLastCreated] = useState<PublicChannelView | null>(null);
  const remember = useCallback(
    (threads: PublicChannelView[]) =>
      storeApi.getState().setField('publicThreadsById', (current) => rememberPublicThreads(current, threads)),
    [storeApi]
  );
  const actions = useMemo<PublicThreadActions>(
    () => ({
      api,
      open,
      create: setCreating,
      copyLink: (topic, channelId) => copyLink(buildPublicThreadLink(topic, channelId)),
      remember,
      lastCreated,
    }),
    [api, open, copyLink, remember, lastCreated]
  );
  return (
    <PublicThreadActionsContext.Provider value={actions}>
      <PublicThreadSlot.Provider value={SLOT}>
        {children}
        {creating ? (
          <CreatePublicThreadDialog
            key={creating}
            api={api}
            topic={creating}
            onClose={() => setCreating(null)}
            onCreated={(thread) => {
              remember([thread]);
              setLastCreated(thread);
              setCreating(null);
              open(thread.topic_id, thread.channel_id);
            }}
          />
        ) : null}
      </PublicThreadSlot.Provider>
    </PublicThreadActionsContext.Provider>
  );
}

// 本文の部品は値を変えずに受け取り、タイトルは chip ごとに store から読む(覚えたスレッドが増えても投稿を描き直さない)。
const SLOT: PublicThreadSlotValue = {
  withTitle: (channelId, render) => <PublicThreadTitle channelId={channelId} render={render} />,
  renderCard: (topic, channelId) => <PublicThreadLinkCard topic={topic} channelId={channelId} />,
};

function PublicThreadTitle({
  channelId,
  render,
}: {
  channelId: string;
  render: (title: string | undefined) => ReactNode;
}) {
  return render(useDesktopShellStore((state) => state.publicThreadsById[channelId]?.title));
}

/** 一覧の列の「スレッドを作る」。タイトル(1〜200 文字)と最初の投稿(空でもよい)を書く(判断 3)。 */
function CreatePublicThreadDialog({
  api,
  topic,
  onClose,
  onCreated,
}: {
  api: DesktopApi;
  topic: string;
  onClose: () => void;
  onCreated: (thread: PublicChannelView) => void;
}) {
  const { t } = useTranslation(['shell', 'common']);
  const [title, setTitle] = useState('');
  const [firstPost, setFirstPost] = useState('');
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (pending || !title.trim()) return;
    setPending(true);
    setFailed(false);
    try {
      onCreated(await api.createPublicChannel(topic, title.trim(), firstPost.trim()));
    } catch {
      setPending(false);
      setFailed(true);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => (open ? undefined : onClose())}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('shell:publicThreads.create.title')}</DialogTitle>
          <DialogDescription>
            {t('shell:publicThreads.create.description', { topic: topicDisplayName(topic) })}
          </DialogDescription>
        </DialogHeader>
        <DialogBody>
          <form className='composer composer-compact' onSubmit={(event) => void submit(event)} aria-busy={pending}>
            <Label>
              <span>{t('shell:publicThreads.create.titleLabel')}</span>
              <Input
                value={title}
                maxLength={TITLE_MAX_CHARS}
                required
                onChange={(event) => setTitle(event.target.value)}
                disabled={pending}
              />
            </Label>
            <Label>
              <span>{t('shell:publicThreads.create.firstPostLabel')}</span>
              <Textarea value={firstPost} onChange={(event) => setFirstPost(event.target.value)} disabled={pending} />
            </Label>
            {failed ? <p className='error error-inline'>{t('shell:publicThreads.create.failed')}</p> : null}
            <Button type='submit' disabled={pending || !title.trim()}>
              {t('shell:publicThreads.create.submit')}
            </Button>
          </form>
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

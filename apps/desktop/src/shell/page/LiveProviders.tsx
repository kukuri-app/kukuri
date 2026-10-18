import { type FormEvent, type ReactNode, useCallback, useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import { PostLiveSlot } from '@/components/core/postLiveSlot';
import { LiveBroadcastBar } from '@/components/extended/LiveBroadcastBar';
import { liveBroadcastFor } from '@/components/extended/liveBroadcast';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Label } from '@/components/ui/label';
import { Select } from '@/components/ui/select';
import { Textarea } from '@/components/ui/textarea';
import type { SupportedLocale } from '@/i18n';
import type { DesktopApi, PostLiveView, PostView } from '@/lib/api';
import type { InternalSmartReference } from '@/lib/internalLinks';
import { isPublicChannelId } from '@/lib/publicChannels';
import { topicDisplayName } from '@/lib/topicId';
import { knownPublicThreadTitle } from '@/shell/slices/publicThreads';
import { useDesktopShellStore } from '@/shell/store';

import { LiveActionsContext, type LiveActions } from './liveActions';
import { PostLive } from './PostLive';

type LiveProvidersProps = {
  api: DesktopApi;
  locale: SupportedLocale;
  join: (sessionId: string, topic: string) => Promise<void>;
  leave: (sessionId: string, topic: string) => Promise<void>;
  end: (sessionId: string, topic: string) => Promise<void>;
  publishLiveIntro: (topic: string, channelId: string | null, content: string, live: PostLiveView) => Promise<boolean>;
  openThread: (threadId: string, options: { topic: string }) => void;
  copyLink: (link: string) => void;
  activateReference: (reference: InternalSmartReference) => void;
  stage: LiveActions['stage'];
  children: ReactNode;
};

type IntroTarget = { live: PostLiveView; title: string };

/** #1839: Live の一覧・投稿の中の Live の操作と、紹介 post の出し先を選ぶ dialog を shell に 1 か所で置く。 */
export function LiveProviders({
  api,
  locale,
  join,
  leave,
  end,
  publishLiveIntro,
  openThread,
  copyLink,
  activateReference,
  stage,
  children,
}: LiveProvidersProps) {
  const { localPubkey, pendingBySessionId } = useDesktopShellStore(
    useShallow((state) => ({
      localPubkey: state.syncStatus.local_author_pubkey,
      pendingBySessionId: state.livePendingBySessionId,
    }))
  );
  const [intro, setIntro] = useState<IntroTarget | null>(null);
  const broadcast = liveBroadcastFor(api);
  const broadcastingSessionId = useSyncExternalStore(broadcast.subscribe, () => broadcast.getState()?.sessionId ?? null);
  // shell を離れたら配信を止める(アカウントの切替は window の reload で止まる)。
  useEffect(() => () => broadcast.stop(), [broadcast]);
  const actions = useMemo<LiveActions>(
    () => ({
      api,
      locale,
      localPubkey,
      pendingBySessionId,
      join: (topic, sessionId) => join(sessionId, topic),
      leave: (topic, sessionId) => leave(sessionId, topic),
      end: (topic, sessionId) => end(sessionId, topic),
      introduce: (live, title) => setIntro({ live, title }),
      broadcastingSessionId,
      startBroadcast: (target, source) => broadcast.start(target, source),
      openPost: (topic, postId) => openThread(postId, { topic }),
      copyLink,
      activateReference,
      stage,
    }),
    [api, locale, localPubkey, pendingBySessionId, join, leave, end, broadcast, broadcastingSessionId, openThread, copyLink, activateReference,
      stage]
  );
  const renderLive = useCallback((post: PostView) => <PostLive post={post} />, []);
  return (
    <LiveActionsContext.Provider value={actions}>
      <PostLiveSlot.Provider value={renderLive}>
        {children}
        {intro ? (
          <LiveIntroDialog
            key={intro.live.session_id}
            target={intro}
            onClose={() => setIntro(null)}
            onPublish={publishLiveIntro}
          />
        ) : null}
      </PostLiveSlot.Provider>
    </LiveActionsContext.Provider>
  );
}

/** #1839 AC-3: shell の上部に置く配信中の操作。 */
export function LiveBroadcastSlot({ api }: { api: DesktopApi }) {
  return <LiveBroadcastBar controller={liveBroadcastFor(api)} />;
}

/**
 * 紹介の出し先。限定 Live はその channel だけ、公開 Live は追跡中の topic の公開の場所と参加中の channel。公開スレッドで始めた
 * Live は公開の Live なので、始めたスレッドを先頭(既定)にして、公開 Live と同じ出し先も選べる(#1840 判断 17)。
 */
function LiveIntroDialog({
  target,
  onClose,
  onPublish,
}: {
  target: IntroTarget;
  onClose: () => void;
  onPublish: LiveProvidersProps['publishLiveIntro'];
}) {
  const { t } = useTranslation(['common', 'live', 'shell']);
  const { trackedTopics, joinedChannelsByTopic } = useDesktopShellStore(
    useShallow((state) => ({
      trackedTopics: state.trackedTopics,
      joinedChannelsByTopic: state.joinedChannelsByTopic,
    }))
  );
  const { live } = target;
  const thread = isPublicChannelId(live.channel_id) ? live.channel_id : null;
  const threadTitle = useDesktopShellStore((state) =>
    thread ? knownPublicThreadTitle(state.publicThreadsById, state.knownAuthorsByPubkey, thread) : undefined
  );
  const options = useMemo(() => {
    const limited = thread ? null : (live.channel_id ?? null);
    if (limited) {
      const label = joinedChannelsByTopic[live.topic_id]?.find((channel) => channel.channel_id === limited)?.label;
      return [{ topic: live.topic_id, channelId: limited, label: `${topicDisplayName(live.topic_id)} / ${label ?? limited}` }];
    }
    const started = thread
      ? [{ topic: live.topic_id, channelId: thread as string | null, label: `${topicDisplayName(live.topic_id)} / ${threadTitle ?? t('shell:publicThreads.title')}` }]
      : [];
    return started.concat(
      trackedTopics.flatMap((topic) => [
        { topic, channelId: null as string | null, label: topicDisplayName(topic) },
        ...(joinedChannelsByTopic[topic] ?? []).map((channel) => ({
          topic,
          channelId: channel.channel_id as string | null,
          label: `${topicDisplayName(topic)} / ${channel.label}`,
        })),
      ])
    );
  }, [live, thread, threadTitle, t, trackedTopics, joinedChannelsByTopic]);
  const [selected, setSelected] = useState(0);
  const [content, setContent] = useState('');
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const option = options[selected];
    if (!option) return;
    setPending(true);
    setFailed(false);
    const published = await onPublish(option.topic, option.channelId, content.trim(), live);
    setPending(false);
    if (published) onClose();
    else setFailed(true);
  };
  return (
    <Dialog open onOpenChange={(open) => (open ? undefined : onClose())}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('live:intro.title')}</DialogTitle>
          <DialogDescription>{target.title}</DialogDescription>
        </DialogHeader>
        <DialogBody>
          <form className='composer composer-compact' onSubmit={(event) => void submit(event)} aria-busy={pending}>
            <Label>
              <span>{t('live:intro.target')}</span>
              <Select
                value={selected}
                disabled={pending || options.length < 2}
                onChange={(event) => setSelected(Number(event.target.value))}
              >
                {options.map((option, index) => (
                  <option key={`${option.topic}:${option.channelId ?? ''}`} value={index}>
                    {option.label}
                  </option>
                ))}
              </Select>
            </Label>
            <Label>
              <span>{t('live:intro.content')}</span>
              <Textarea
                value={content}
                onChange={(event) => setContent(event.target.value)}
                placeholder={t('live:intro.placeholder')}
                disabled={pending}
              />
            </Label>
            {failed ? <p className='error error-inline'>{t('common:errors.failedToPublish')}</p> : null}
            <Button type='submit' disabled={pending || options.length === 0}>
              {t('live:actions.introduce')}
            </Button>
          </form>
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

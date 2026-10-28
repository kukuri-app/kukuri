import { type FormEvent, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ComposerAdultLabelToggle } from '@/components/core/ComposerAdultLabelToggle';
import { ComposerAttachmentField } from '@/components/core/ComposerAttachmentField';
import { ComposerDraftPreviewList } from '@/components/core/ComposerDraftPreviewList';
import { PublicThreadSlot, type PublicThreadSlotValue } from '@/components/core/publicThreadSlot';
import { useOnScreen } from '@/components/core/useOnScreen';
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
import { clipboardImageFiles } from '@/lib/attachments';
import { buildPublicThreadLink } from '@/lib/internalLinks';
import { topicDisplayName } from '@/lib/topicId';
import {
  ADULT_CONTENT_LABEL,
  buildDraftMediaItems,
  buildImageDraftItem,
  buildVideoDraftItem,
  composerDraftMediaViews,
} from '@/shell/media';
import {
  KNOWN_PUBLIC_THREAD_LIMIT,
  isHiddenAuthor,
  knownPublicThreadTitle,
  rememberPublicThreads,
} from '@/shell/slices/publicThreads';
import { type DraftMediaItem, useDesktopShellStore, useDesktopShellStoreApi } from '@/shell/store';

import { PublicThreadActionsContext, type PublicThreadActions } from './publicThreadActions';
import { PublicThreadLinkCard } from './PublicThreadLinkCard';
import { LiveThumbnailField } from './LiveThumbnailField';

/// タイトルの上限(core の `PUBLIC_CHANNEL_MAX_TITLE_CHARS`)。
const TITLE_MAX_CHARS = 200;

type PublicThreadProvidersProps = {
  api: DesktopApi;
  open: (topic: string, channelId: string | null) => void;
  openLive: PublicThreadActions['openLive'];
  copyLink: (link: string) => void;
  children: ReactNode;
};

/** #1840: 公開スレッドの操作と本文のリンクの表示、作成の Dialog を shell に 1 か所で置く。 */
export function PublicThreadProviders({ api, open, openLive, copyLink, children }: PublicThreadProvidersProps) {
  const storeApi = useDesktopShellStoreApi();
  const { t } = useTranslation('shell');
  const [creating, setCreating] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<PublicChannelView | null>(null);
  const [lastCreated, setLastCreated] = useState<PublicChannelView | null>(null);
  // 読めたスレッドは覚える数と同じだけ使い回す(古いものから外す)。使い回すときも覚え直す(一覧が多くのスレッドを
  // 覚えて、store から外れていても戻す)。読めなかったものは外し、次に表示したときに読む。
  const loads = useRef(new Map<string, Promise<PublicChannelView | null>>());
  // 新しく覚えた値は、使い回す値も置き換える(変更・削除の前の値で覚え直さない。#1888)。
  const remember = useCallback(
    (threads: PublicChannelView[]) => {
      for (const thread of threads) {
        if (loads.current.has(thread.channel_id)) loads.current.set(thread.channel_id, Promise.resolve(thread));
      }
      storeApi.getState().setField('publicThreadsById', (current) => rememberPublicThreads(current, threads));
    },
    [storeApi]
  );
  const load = useCallback(
    (topic: string, channelId: string) => {
      const remembered = (thread: PublicChannelView | null) => {
        if (thread) remember([thread]);
        return thread;
      };
      const pending = loads.current.get(channelId);
      if (pending) return pending.then(remembered);
      // 外すのは自分の読み込みだけ(上限で先に外れた後の、同じスレッドの新しい読み込みは残す)。
      const forget = () => {
        if (loads.current.get(channelId) === request) loads.current.delete(channelId);
        return null;
      };
      const request: Promise<PublicChannelView | null> = api
        .getPublicChannel(topic, channelId)
        .then((thread) => (thread ? remembered(thread) : forget()), forget);
      loads.current.set(channelId, request);
      for (const key of loads.current.keys()) {
        if (loads.current.size <= KNOWN_PUBLIC_THREAD_LIMIT) break;
        loads.current.delete(key);
      }
      return request;
    },
    [api, remember]
  );
  const refresh = useCallback(
    (topic: string, channelId: string) => {
      loads.current.delete(channelId);
      return load(topic, channelId);
    },
    [load]
  );
  const remove = useCallback(
    (thread: PublicChannelView) => {
      if (!window.confirm(t('publicThreads.delete.confirm', { title: thread.title }))) return;
      const setError = (error: string | null) => storeApi.getState().setField('error', error);
      void api.deletePublicChannel(thread.topic_id, thread.channel_id).then(
        (deleted) => {
          remember([deleted]);
          setError(null);
        },
        () => setError(t('publicThreads.delete.failed'))
      );
    },
    [api, remember, storeApi, t]
  );
  const actions = useMemo<PublicThreadActions>(
    () => ({
      api,
      open,
      create: setCreating,
      copyLink: (topic, channelId) => copyLink(buildPublicThreadLink(topic, channelId)),
      openLive,
      remember,
      load,
      refresh,
      rename: setRenaming,
      remove,
      lastCreated,
    }),
    [api, open, openLive, copyLink, remember, load, refresh, remove, lastCreated]
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
        {renaming ? (
          <RenamePublicThreadDialog
            key={renaming.channel_id}
            api={api}
            thread={renaming}
            onClose={() => setRenaming(null)}
            onRenamed={(thread) => {
              remember([thread]);
              setRenaming(null);
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
  renderChipTitle: (topic, channelId, fallback) => (
    <PublicThreadChipTitle topic={topic} channelId={channelId} fallback={fallback} />
  ),
  renderCard: (topic, channelId) => <PublicThreadLinkCard topic={topic} channelId={channelId} />,
};

function useKnownTitle(channelId: string) {
  return useDesktopShellStore((state) =>
    knownPublicThreadTitle(state.publicThreadsById, state.knownAuthorsByPubkey, channelId)
  );
}

function PublicThreadTitle({
  channelId,
  render,
}: {
  channelId: string;
  render: (title: string | undefined) => ReactNode;
}) {
  return render(useKnownTitle(channelId));
}

// 読めた値は chip が持つ(card と同じ)。画面内のリンクが覚える上限より多く、覚えたスレッドから外れても、表示を保ち、
// 読み直しを繰り返さない(読み込みは chip ごとに最大 1 回)。作成者を非表示にしたら、削除されたら(#1888)出さない(一覧と
// 同じ)。
function PublicThreadChipTitle({ topic, channelId, fallback }: { topic: string; channelId: string; fallback: string }) {
  const load = useContext(PublicThreadActionsContext)?.load;
  const remembered = useDesktopShellStore((state) => state.publicThreadsById[channelId]);
  const [loaded, setLoaded] = useState<PublicChannelView | null>(null);
  const thread = remembered ?? (loaded?.channel_id === channelId ? loaded : null);
  const hidden = useDesktopShellStore((state) =>
    thread ? isHiddenAuthor(state.knownAuthorsByPubkey[thread.creator_pubkey]) : false
  );
  const ref = useRef<HTMLSpanElement>(null);
  const onScreen = useOnScreen(ref, channelId);
  const have = thread !== null;
  useEffect(() => {
    if (!load || !onScreen || have) return;
    void load(topic, channelId).then((value) => {
      if (value) setLoaded(value);
    });
  }, [load, onScreen, have, topic, channelId]);
  return <span ref={ref}>{thread && !hidden && !thread.deleted ? thread.title : fallback}</span>;
}

/** 一覧の列の「スレッドを作る」。タイトル(1〜200 文字)と最初の投稿(空でもよい)を書く(判断 3)。最初の投稿には
 * タイムラインの投稿と同じ添付と成人向けの自己申告を付けられる(#1889)。閉じると下書きは残さない。 */
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
  const { t } = useTranslation(['shell', 'common', 'live']);
  const [title, setTitle] = useState('');
  const [firstPost, setFirstPost] = useState('');
  const [media, setMedia] = useState<DraftMediaItem[]>([]);
  const [attachmentInputKey, setAttachmentInputKey] = useState(0);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [adultLabeled, setAdultLabeled] = useState(false);
  const [startLive, setStartLive] = useState(false);
  const [thumbnail, setThumbnail] = useState<string | null>(null);
  const [createdLiveId, setCreatedLiveId] = useState<string | null>(null);
  const [createdThread, setCreatedThread] = useState<PublicChannelView | null>(null);
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const draftSequence = useRef(0);
  // 閉じたときに、まだ持っている添付の preview を解放する。
  const mediaRef = useRef(media);
  useEffect(() => {
    mediaRef.current = media;
  }, [media]);
  useEffect(() => () => mediaRef.current.forEach((item) => URL.revokeObjectURL(item.preview_url)), []);
  const addFiles = async (files: File[]) => {
    if (files.length === 0) return;
    const nextDraftId = () => `public-thread-draft-${(draftSequence.current += 1)}`;
    const { items, error } = await buildDraftMediaItems(
      files,
      { image: (file) => buildImageDraftItem(file, nextDraftId), video: (file) => buildVideoDraftItem(file, nextDraftId) },
      t
    );
    setMedia((current) => [...current, ...items]);
    setAttachmentError(error);
  };
  const removeFile = (itemId: string) => {
    const item = media.find((candidate) => candidate.id === itemId);
    if (item) URL.revokeObjectURL(item.preview_url);
    setMedia((current) => current.filter((candidate) => candidate.id !== itemId));
  };
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (pending || !title.trim()) return;
    setPending(true);
    setFailed(false);
    try {
      const thread = createdThread ?? await api.createPublicChannel(
          topic,
          title.trim(),
          firstPost.trim(),
          media.flatMap((item) => item.attachments),
          adultLabeled ? [ADULT_CONTENT_LABEL] : []
        );
      setCreatedThread(thread);
      if (startLive) {
        const id = createdLiveId ?? await api.createLiveSession(topic, thread.title, '', { kind: 'public_channel', channel_id: thread.channel_id });
        setCreatedLiveId(id);
        if (thumbnail) await api.setLiveThumbnail({ topic, session_id: id, data_url: thumbnail, custom: true });
      }
      onCreated(thread);
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
              <Textarea
                value={firstPost}
                onChange={(event) => setFirstPost(event.target.value)}
                onPaste={(event) => {
                  const images = clipboardImageFiles(event.clipboardData);
                  if (images.length === 0) return;
                  event.preventDefault();
                  void addFiles(images);
                }}
                disabled={pending}
              />
            </Label>
            <ComposerAttachmentField
              onBookmarkSelection={(item) => setMedia((current) => [...current, item])}
              inputKey={attachmentInputKey}
              count={media.length}
              disabled={pending}
              onChange={(event) => {
                const files = Array.from(event.target.files ?? []);
                setAttachmentInputKey((key) => key + 1);
                void addFiles(files);
              }}
            />
            {attachmentError ? (
              <p className='error error-inline' role='alert'>
                {attachmentError}
              </p>
            ) : null}
            <ComposerDraftPreviewList items={composerDraftMediaViews(media)} onRemove={removeFile} />
            <ComposerAdultLabelToggle checked={adultLabeled} onChange={setAdultLabeled} />
            <label className='flex items-center gap-2'>
              <input type='checkbox' checked={startLive} disabled={pending || Boolean(createdThread)} onChange={(event) => setStartLive(event.target.checked)} />
              <span>{t('live:actions.start')}</span>
            </label>
            {startLive ? <LiveThumbnailField value={thumbnail} onChange={setThumbnail} disabled={pending} /> : null}
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

/** 作成者のタイトルの変更(#1888)。作成と同じ 1〜200 文字。 */
function RenamePublicThreadDialog({
  api,
  thread,
  onClose,
  onRenamed,
}: {
  api: DesktopApi;
  thread: PublicChannelView;
  onClose: () => void;
  onRenamed: (thread: PublicChannelView) => void;
}) {
  const { t } = useTranslation(['shell', 'common', 'live']);
  const [title, setTitle] = useState(thread.title);
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const changed = title.trim() !== '' && title.trim() !== thread.title;
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (pending || !changed) return;
    setPending(true);
    setFailed(false);
    try {
      onRenamed(await api.updatePublicChannelTitle(thread.topic_id, thread.channel_id, title.trim()));
    } catch {
      setPending(false);
      setFailed(true);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => (open ? undefined : onClose())}>
      <DialogContent aria-describedby={undefined}>
        <DialogHeader>
          <DialogTitle>{t('shell:publicThreads.rename.title')}</DialogTitle>
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
            {failed ? <p className='error error-inline'>{t('shell:publicThreads.rename.failed')}</p> : null}
            <Button type='submit' disabled={pending || !changed}>
              {t('shell:publicThreads.rename.submit')}
            </Button>
          </form>
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

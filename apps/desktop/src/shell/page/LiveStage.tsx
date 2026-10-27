import { type ReactNode, useCallback, useContext, useEffect, useId, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { Check, Reply, SlidersHorizontal } from 'lucide-react';

import { ReactionGlyph } from '@/components/core/ReactionGlyph';
import type { PostMediaView } from '@/components/core/types';
import { ReactionPickerPopover } from '@/components/core/ReactionPickerPopover';
import { LiveViewer } from '@/components/extended/LiveSessionMedia';
import { LiveBroadcastPreview } from '@/components/extended/LiveBroadcastPreview';
import { liveBroadcastFor } from '@/components/extended/liveBroadcast';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Notice } from '@/components/ui/notice';
import { Popover, PopoverMenuContent, PopoverTrigger } from '@/components/ui/popover';
import type {
  CustomReactionAssetView,
  PostView,
  ReactionKeyInput,
  ReactionKeyView,
  ReactionSummaryView,
  RecentReactionView,
} from '@/lib/api';
import { MENTION_PATTERN } from '@/lib/internalLinks';
import { prefersReducedMotion } from '@/lib/reducedMotion';
import { authorDisplayLabel, translateLiveStatus } from '@/shell/presentation';
import { removeRecordEntry, setRecordEntry } from '@/shell/stateUpdates';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';

import { LiveBroadcastStartButtons } from './LiveCard';
import { LiveOverlayImage } from './LiveOverlayImage';
import { useLiveOverlayPushes, type OverlayPush } from './liveOverlayPush';
import { LiveActionsContext } from './liveActions';
import { type LiveOverlayFloat, type LiveOverlayPost, type LiveOverlaySource, useLiveOverlay } from './liveOverlay';
import {
  LIVE_OVERLAY_POSITIONS,
  LIVE_OVERLAY_SIZES,
  type LiveOverlaySettings,
  useLiveOverlaySettings,
} from './liveOverlaySettings';

/// 1 件の投稿に添えるリアクションの種類の数と、開始 post の数の表示に出す種類の数(多い順。#1841 D1)。
const POST_CHIP_LIMIT = 3;
const COUNT_LIMIT = 8;

type LiveStageProps = {
  source: LiveOverlaySource;
  // 会話の場所の名前(スレッドのタイトル・topic・channel)。
  conversationLabel: string;
  // 列が見えているか(見えない間は浮かべない)。
  visible: boolean;
  // 通常の投稿の表示と同じ判定で隠す投稿(成人向け・信頼値の評価)。
  hidden?: (post: PostView) => boolean;
  mediaForPost: (post: PostView) => PostMediaView;
  recentReactions: RecentReactionView[];
  reactionAssets: CustomReactionAssetView[];
  onReactionPickerOpen: () => void;
  onToggleReaction: (post: PostView, reactionKey: ReactionKeyInput) => void;
  onReply: (post: PostView) => void;
};

/**
 * #1841 AC-2: 選んだ Live を大きく出し、会話の場所の投稿と、開始 post のリアクションを映像に重ねる。重ねる層は画面の DOM だけで、
 * 映像・post・リアクションは変えない。リアクションと返信は開始 post に付ける(既存の picker と返信の入力)。層の切替・設定の変更では
 * 何も送らない。
 */
export function LiveStage({
  source,
  conversationLabel,
  visible,
  hidden,
  mediaForPost,
  recentReactions,
  reactionAssets,
  onReactionPickerOpen,
  onToggleReaction,
  onReply,
}: LiveStageProps) {
  const { t } = useTranslation(['live', 'common']);
  const actions = useContext(LiveActionsContext);
  const [settings, updateSettings] = useLiveOverlaySettings();
  const [foreground, setForeground] = useState(() => !document.hidden);
  useEffect(() => {
    const changed = () => setForeground(!document.hidden);
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, []);
  const mediaObjectUrls = useDesktopShellStore((state) => state.mediaObjectUrls);
  const { element, session, startPost, conversation, posts, floats } = useLiveOverlay(actions?.api, source, {
    hidden,
    animate: visible && !prefersReducedMotion(),
  });
  const overlayActive = visible && foreground && session?.status === 'Live';
  let body;
  if (session === undefined) body = <Notice>{t('live:loading')}</Notice>;
  else if (session === null) body = <Notice>{t('live:unavailable')}</Notice>;
  else if (session.status === 'Ended') body = <Notice>{t('live:stage.ended')}</Notice>;
  else {
    const isOwner = session.host_pubkey === actions?.localPubkey;
    const layers = (
      <OverlayLayers
        posts={posts}
        counts={startPost?.reaction_summary ?? []}
        floats={floats}
        settings={settings}
        mediaObjectUrls={mediaObjectUrls}
        mediaForPost={mediaForPost}
        imagesEnabled={visible && foreground && session.status === 'Live'}
        pushes={<LivePushImages source={source} hidden={hidden} mediaForPost={mediaForPost} />}
      />
    );
    body = (
      <>
        <div className='live-stage-header'>
          <strong className='post-copy-wrap'>{session.title}</strong>
          <span>{translateLiveStatus(session.status)}</span>
          <span className='live-stage-context'>{t('live:stage.conversation', { place: conversationLabel })}</span>
        </div>
        {actions && isOwner ? (
          <LiveBroadcastPreview controller={liveBroadcastFor(actions.api)} sessionId={session.session_id} overlay={layers} />
        ) : actions && session.joined_by_me ? (
          <LiveViewer
            api={actions.api}
            request={{ topic: source.live.topic_id, session_id: session.session_id }}
            overlay={layers}
          />
        ) : (
          <div className='live-viewer-screen live-stage-placeholder'>
            {layers}
            <p>{t('live:stage.joinToWatch')}</p>
          </div>
        )}
        {conversation === 'unreadable' ? <Notice>{t('live:stage.unreadable')}</Notice> : null}
        <div className='live-stage-controls'>
          <LiveBroadcastStartButtons topic={source.live.topic_id} session={session} />
          <div className='live-stage-controls' role='group' aria-label={t('live:stage.layers')}>
            {(['posts', 'reactions'] as const).map((layer) => (
              <Button
                key={layer}
                variant='secondary'
                size='sm'
                type='button'
                aria-pressed={settings[layer]}
                onClick={() => updateSettings({ [layer]: !settings[layer] })}
              >
                {settings[layer] ? <Check className='size-4' aria-hidden='true' /> : null}
                {t(`live:stage.${layer}`)}
              </Button>
            ))}
          </div>
          <LiveOverlaySettingsMenu settings={settings} onChange={updateSettings} />
          {startPost ? (
            <>
              <ReactionPickerPopover
                post={startPost}
                label={t('live:stage.react')}
                recentReactions={recentReactions}
                assets={reactionAssets}
                mediaObjectUrls={mediaObjectUrls}
                onToggleReaction={onToggleReaction}
                onOpen={onReactionPickerOpen}
              />
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('live:stage.reply')}
                onClick={() => onReply(startPost)}
              >
                <Reply className='size-4' aria-hidden='true' />
              </IconButton>
            </>
          ) : null}
        </div>
      </>
    );
  }
  return (
    <section
      ref={element}
      className='live-stage'
      data-live-session-id={session?.session_id}
      aria-label={t('live:stage.label', { title: session?.title ?? '' })}
    >
      {startPost && overlayActive ? <OverlayPostDemand post={startPost} /> : null}
      {body}
    </section>
  );
}

type OverlayLayersProps = {
  posts: LiveOverlayPost[];
  counts: ReactionSummaryView[];
  floats: LiveOverlayFloat[];
  settings: LiveOverlaySettings;
  mediaObjectUrls: Record<string, string | null>;
  mediaForPost: (post: PostView) => PostMediaView;
  imagesEnabled: boolean;
  pushes: ReactNode;
};

/// 映像に重ねる層。読み上げない(同じ内容と操作には、スレッドの列から届く)。
function OverlayLayers({ posts, counts, floats, settings, mediaObjectUrls, mediaForPost, imagesEnabled, pushes }: OverlayLayersProps) {
  const { t } = useTranslation('live');
  const top = <T extends { count: number }>(items: T[], limit: number) =>
    [...items].sort((left, right) => right.count - left.count).slice(0, limit);
  return (
    <div
      className='live-stage-layers'
      data-position={settings.position}
      data-text={settings.text}
      data-reaction-size={settings.reactionSize}
      aria-hidden='true'
    >
      {settings.posts ? (
        <ol className='live-stage-posts'>
          {posts.map(({ post, liveReply, fresh }) => (
            <li
              key={post.object_id}
              className={fresh ? 'live-stage-post live-stage-post-fresh' : 'live-stage-post'}
              data-post-object-id={post.object_id}
            >
              <div className='live-stage-post-copy'>
                <span className='live-stage-author'>
                  {authorDisplayLabel(post.author_pubkey, post.author_display_name, post.author_name)}
                </span>
                {liveReply ? <span className='live-stage-badge'>{t('stage.liveReply')}</span> : null}
                <span>
                  {post.content.trim()
                    ? post.content.replace(MENTION_PATTERN, '@$1')
                    : t('stage.attachments', { count: post.attachments.length })}
                </span>
                {post.reaction_summary?.length ? (
                  <span className='live-stage-chips'>
                    {top(post.reaction_summary, POST_CHIP_LIMIT).map((reaction) => (
                      <span key={reaction.normalized_reaction_key} className='live-stage-chip'>
                        <Glyph reaction={reaction} mediaObjectUrls={mediaObjectUrls} />
                        {reaction.count}
                      </span>
                    ))}
                  </span>
                ) : null}
              </div>
              {imagesEnabled ? <LiveOverlayImage media={mediaForPost(post)} /> : null}
            </li>
          ))}
        </ol>
      ) : null}
      {(settings.posts && imagesEnabled) || settings.reactions ? <div className='live-stage-aside'>
        {settings.posts && imagesEnabled ? pushes : null}
        {settings.reactions ? (
          <div className='live-stage-reactions'>
            {floats.map((float) => (
              <span key={float.id} className='live-stage-float' style={{ left: floatOffset(float.id) }}>
                <Glyph reaction={float.reaction} mediaObjectUrls={mediaObjectUrls} />
              </span>
            ))}
            <div className='live-stage-counts'>
              {top(counts, COUNT_LIMIT).map((reaction) => (
                <span key={reaction.normalized_reaction_key} className='live-stage-count'>
                  <Glyph reaction={reaction} mediaObjectUrls={mediaObjectUrls} />
                  {reaction.count}
                </span>
              ))}
            </div>
          </div>
        ) : null}
      </div> : null}
    </div>
  );
}

function OverlayPostDemand({ post }: { post: PostView }) {
  const observer = useId();
  const setPosts = useDesktopShellFieldSetter('liveStagePosts');
  useEffect(() => {
    setPosts(setRecordEntry(observer, post));
    return () => setPosts(removeRecordEntry(observer));
  }, [observer, post, setPosts]);
  return null;
}

/** 回数だけの更新で、映像・通常投稿・操作部を再計算しない。 */
function LivePushImages({ source, hidden, mediaForPost }: Pick<LiveStageProps, 'source' | 'hidden' | 'mediaForPost'>) {
  const { t } = useTranslation('live');
  const actions = useContext(LiveActionsContext);
  const pushed = useLiveOverlayPushes(actions?.api, source, true, hidden);
  const [host, setHost] = useState<Element | null>(null);
  const attach = useCallback((element: HTMLDivElement | null) => setHost(element?.closest('.live-stage') ?? null), []);
  return <>
    <div className='live-stage-pushes' ref={attach}>
      {pushed.items.filter(({ post }) => !post || !hidden?.(post)).map((slot) => slot.post ? <OverlayPushImage key={slot.key} slot={slot} mediaForPost={mediaForPost} /> : (
        <span key={slot.key} className='live-stage-post-copy'>{t('loading')}</span>
      ))}
    </div>
    {pushed.notice && host ? createPortal(<span className='live-push-notice' role='status'>{t(`push.${pushed.notice}`)}</span>, host) : null}
  </>;
}

function OverlayPushImage({ slot, mediaForPost }: { slot: OverlayPush; mediaForPost: (post: PostView) => PostMediaView }) {
  const post = slot.post!;
  const media = mediaForPost(post);
  const selected = media.imageGalleryItems?.find((image) => image.hash === slot.event.target.attachment_hash);
  return <>
    <OverlayPostDemand post={post} />
    <LiveOverlayImage media={{ ...media, imageGalleryItems: selected ? [selected] : [] }} count={slot.count} />
  </>;
}

function Glyph({ reaction, mediaObjectUrls }: { reaction: ReactionKeyView; mediaObjectUrls: Record<string, string | null> }) {
  return (
    <span className='live-stage-glyph'>
      <ReactionGlyph reaction={reaction} mediaObjectUrls={mediaObjectUrls} />
    </span>
  );
}

/// 浮かべる位置を id から決める(同じものは同じ位置。0〜80%)。
function floatOffset(id: string) {
  let hash = 0;
  for (const char of id) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return `${hash % 81}%`;
}

/// 表示の設定(投稿の位置・文字の大きさ・リアクションの大きさ)。端末に 1 組だけ保存する(#1841 判断 5)。
function LiveOverlaySettingsMenu({
  settings,
  onChange,
}: {
  settings: LiveOverlaySettings;
  onChange: (patch: Partial<LiveOverlaySettings>) => void;
}) {
  const { t } = useTranslation('live');
  const [open, setOpen] = useState(false);
  const group = <K extends 'position' | 'text' | 'reactionSize'>(
    key: K,
    values: readonly LiveOverlaySettings[K][],
    label: (value: LiveOverlaySettings[K]) => string
  ) => (
    <div role='group' aria-label={t(`stage.${key}`)} className='space-y-1'>
      <p className='post-reaction-section-title'>{t(`stage.${key}`)}</p>
      {values.map((value) => (
        <Button
          key={value}
          role='menuitemradio'
          aria-checked={settings[key] === value}
          variant='ghost'
          className='w-full justify-start'
          onClick={() => onChange({ [key]: value })}
        >
          {label(value)}
          {settings[key] === value ? <Check className='ml-auto size-4 shrink-0' aria-hidden='true' /> : null}
        </Button>
      ))}
    </div>
  );
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <IconButton
          variant='secondary'
          className='post-action-button'
          type='button'
          label={t('stage.settings')}
          aria-haspopup='menu'
        >
          <SlidersHorizontal className='size-4' aria-hidden='true' />
        </IconButton>
      </PopoverTrigger>
      <PopoverMenuContent aria-label={t('stage.settings')} align='start' className='space-y-2'>
        {group('position', LIVE_OVERLAY_POSITIONS, (value) => t(`stage.positions.${value}`))}
        {group('text', LIVE_OVERLAY_SIZES, (value) => t(`stage.sizes.${value}`))}
        {group('reactionSize', LIVE_OVERLAY_SIZES, (value) => t(`stage.sizes.${value}`))}
      </PopoverMenuContent>
    </Popover>
  );
}

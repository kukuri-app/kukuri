import { GripVertical, Pin, PinOff, X } from 'lucide-react';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { IconButton } from '@/components/ui/icon-button';
import type { ColumnSpan } from '@/shell/slices/workspace';
import { useColumnRuntime } from '@/shell/ColumnRuntimeContext';
import { ColumnFullscreenContext } from '@/shell/ColumnPresentationContext';
import { ColumnMenu } from './ColumnMenu';

type ColumnSurfaceProps = {
  active: boolean;
  children: ReactNode;
  columnId: string;
  footer?: ReactNode;
  fullscreenable?: boolean;
  headerActions?: ReactNode;
  scopeControl?: ReactNode;
  // scope 行の右側に置く導線(例: Timeline Column のプライベートチャンネル入口、Issue #966)。
  scopeActions?: ReactNode;
  // header の下段に幅いっぱいで置く、本文と一緒に流れない内容(例: 公開スレッドの詳細の列の配信中の Live。#1840 AC-4)。
  headerDetails?: ReactNode;
  onPinnedChange?: (pinned: boolean) => void;
  onClose?: () => void;
  onMoveLeft?: () => void;
  onMoveRight?: () => void;
  onSpanChange?: (span: ColumnSpan) => void;
  pinned: boolean;
  position: number;
  scopeLabel?: string;
  span: ColumnSpan;
  spanOptions?: ColumnSpan[];
  title: string;
  total: number;
  resourceManaged?: boolean;
};

export function ColumnSurface({
  active,
  children,
  columnId,
  footer,
  fullscreenable = false,
  headerActions,
  scopeControl,
  scopeActions,
  headerDetails,
  onPinnedChange,
  onClose,
  onMoveLeft,
  onMoveRight,
  onSpanChange,
  pinned,
  position,
  scopeLabel,
  span,
  spanOptions = [span],
  title,
  total,
  resourceManaged = false,
}: ColumnSurfaceProps) {
  const { t } = useTranslation('shell');
  const runtime = useColumnRuntime();
  const surfaceRef = useRef<HTMLElement | null>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [announcement, setAnnouncement] = useState('');
  const wasFullscreenRef = useRef(false);
  const bodyScrollRef = useRef(0);
  const canvasScrollRef = useRef(0);
  const activityLabel = t(active ? 'columnState.active' : 'columnState.inactive');
  const stateLabel = t(pinned ? 'columnState.pinned' : 'columnState.temporary');
  const accessibleLabel = t('columnState.accessibleLabel', {
    title,
    position,
    total,
    span,
    activity: activityLabel,
    persistence: stateLabel,
  });

  useEffect(() => {
    if (!resourceManaged || (!runtime.suspended && runtime.audioFocused)) return;
    const media = surfaceRef.current?.querySelectorAll<HTMLMediaElement>('audio, video');
    media?.forEach((item) => {
      if (!item.muted && !item.paused && (runtime.suspended || !runtime.audioFocused)) {
        item.pause();
      }
    });
  }, [resourceManaged, runtime.audioFocused, runtime.suspended]);

  useEffect(() => {
    let restoreFrame = 0;
    let releaseAnchor = () => {};
    let cancelResize = () => {};
    const handleFullscreenChange = () => {
      const nextFullscreen = document.fullscreenElement === surfaceRef.current;
      cancelResize();
      cancelAnimationFrame(restoreFrame);
      releaseAnchor();
      if (wasFullscreenRef.current && !nextFullscreen) {
        const owner = surfaceRef.current;
        const body = owner?.querySelector<HTMLElement>('.shell-column-body');
        if (body && owner) {
          const anchor = body.style.overflowAnchor;
          body.style.overflowAnchor = 'none';
          // Native resize and responsive form reflow can finish after the DOM
          // fullscreen event. Keep the restored reading position until input.
          releaseAnchor = (event?: Event) => {
            if (event instanceof KeyboardEvent && event.key === 'Escape') return;
            cancelResize();
            cancelAnimationFrame(restoreFrame);
            body.style.overflowAnchor = anchor;
            document.removeEventListener('pointerdown', releaseAnchor, true);
            document.removeEventListener('wheel', releaseAnchor, true);
            document.removeEventListener('keydown', releaseAnchor, true);
          };
          document.addEventListener('pointerdown', releaseAnchor, { capture: true, once: true });
          document.addEventListener('wheel', releaseAnchor, { capture: true, once: true });
          document.addEventListener('keydown', releaseAnchor, true);
        }
        // Restore after the normal layout has committed; fullscreen temporarily
        // removes this Column's width and browsers clamp the Canvas scrollLeft.
        const restore = () => {
          cancelAnimationFrame(restoreFrame);
          restoreFrame = requestAnimationFrame(() => {
            const surface = surfaceRef.current;
            if (document.fullscreenElement || !surface) { releaseAnchor(); return; }
            surface.focus({ preventScroll: true });
            const body = surface.querySelector('.shell-column-body');
            const canvas = surface.closest('.shell-column-canvas');
            if (body) body.scrollTop = bodyScrollRef.current;
            if (canvas) canvas.scrollLeft = canvasScrollRef.current;
            restoreFrame = requestAnimationFrame(() => {
              if (!document.fullscreenElement && body) body.scrollTop = bodyScrollRef.current;
            });
          });
        };
        // WebView2 emits intermediate native sizes on exit. Keep restoring until
        // the user's next input, rather than treating the first resize as final.
        window.addEventListener('resize', restore);
        cancelResize = () => window.removeEventListener('resize', restore);
        restore();
      }
      wasFullscreenRef.current = nextFullscreen;
      setFullscreen(nextFullscreen);
    };
    document.addEventListener('fullscreenchange', handleFullscreenChange);
    return () => {
      document.removeEventListener('fullscreenchange', handleFullscreenChange);
      cancelAnimationFrame(restoreFrame);
      cancelResize();
      releaseAnchor();
    };
  }, []);

  const toggleFullscreen = async () => {
    const surface = surfaceRef.current;
    try {
      if (!surface) {
        throw new Error('fullscreen surface unavailable');
      }
      if (document.fullscreenElement === surface) {
        if (typeof document.exitFullscreen !== 'function') {
          throw new Error('fullscreen exit unavailable');
        }
        await document.exitFullscreen();
      } else {
        bodyScrollRef.current = surface.querySelector('.shell-column-body')?.scrollTop ?? 0;
        canvasScrollRef.current = surface.closest('.shell-column-canvas')?.scrollLeft ?? 0;
        if (
          document.fullscreenEnabled === false ||
          typeof surface.requestFullscreen !== 'function'
        ) {
          throw new Error('fullscreen request unavailable');
        }
        await surface.requestFullscreen();
      }
    } catch {
      setAnnouncement(t('columnMenu.fullscreenFailed', { title }));
    }
  };

  return (
    <section
      ref={surfaceRef}
      className='shell-column-surface'
      data-active={active || undefined}
      data-column-id={columnId}
      data-pinned={pinned || undefined}
      data-span={span}
      data-transient={!pinned || undefined}
      data-runtime-visible={runtime.visible}
      data-runtime-suspended={runtime.suspended || undefined}
      aria-current={active ? 'true' : undefined}
      aria-label={accessibleLabel}
      aria-roledescription='Column'
      tabIndex={-1}
      onPlayCapture={(event) => {
        if (event.target instanceof HTMLMediaElement && !event.target.muted) {
          runtime.requestAudioFocus();
        }
      }}
      onPauseCapture={(event) => {
        if (event.target instanceof HTMLMediaElement) {
          runtime.releaseAudioFocus();
        }
      }}
    >
      <header className='shell-column-header'>
        <IconButton
          variant='ghost'
          type='button'
          className='shell-column-drag-grip'
          data-column-drag-grip
          label={t('columnMenu.drag', { title })}
        >
          <GripVertical className='size-4' aria-hidden='true' />
        </IconButton>
        <div className='shell-column-heading'>
          <div className='shell-column-title-row'>
            <h2>{title}</h2>
            {active ? <span className='shell-column-state-label'>{activityLabel}</span> : null}
            <span className='shell-column-state-label'>{stateLabel}</span>
          </div>
          {scopeLabel || scopeControl || scopeActions ? (
            <div className='shell-column-scope-row'>
              {scopeControl ? (
                <>
                  <span className='sr-only'>{scopeLabel}</span>
                  {scopeControl}
                </>
              ) : (
                <p>{scopeLabel}</p>
              )}
              {scopeActions}
            </div>
          ) : null}
        </div>
        {headerActions || onPinnedChange || onClose ? (
          <div className='shell-column-header-actions'>
            {headerActions}
            {onPinnedChange ? (
              <IconButton
                variant='ghost'
                type='button'
                className='shell-column-pin-button'
                label={t(pinned ? 'columnMenu.unpin' : 'columnMenu.pin', { title })}
                aria-pressed={pinned}
                onClick={() => onPinnedChange(!pinned)}
              >
                {pinned ? (
                  <PinOff className='size-4' aria-hidden='true' />
                ) : (
                  <Pin className='size-4' aria-hidden='true' />
                )}
              </IconButton>
            ) : null}
            {onClose ? (
              <IconButton
                variant='ghost'
                type='button'
                className='shell-column-close-button'
                label={t('columnMenu.closeColumn', { title })}
                onClick={onClose}
              >
                <X className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
            <ColumnMenu
              title={title}
              pinned={pinned}
              span={span}
              spanOptions={spanOptions}
              onMoveLeft={
                onMoveLeft
                  ? () => {
                      onMoveLeft();
                      setAnnouncement(
                        t('columnMenu.moved', { title, position: position - 1, total })
                      );
                    }
                  : undefined
              }
              onMoveRight={
                onMoveRight
                  ? () => {
                      onMoveRight();
                      setAnnouncement(
                        t('columnMenu.moved', { title, position: position + 1, total })
                      );
                    }
                  : undefined
              }
              onPinnedChange={onPinnedChange}
              onClose={onClose}
              fullscreen={fullscreen}
              onToggleFullscreen={fullscreenable ? () => void toggleFullscreen() : undefined}
              onSpanChange={
                onSpanChange
                  ? (nextSpan) => {
                      onSpanChange(nextSpan);
                      setAnnouncement(t('columnMenu.spanChanged', { title, count: nextSpan }));
                    }
                  : undefined
              }
            />
          </div>
        ) : (
          <div className='shell-column-header-actions'>
            <ColumnMenu
              title={title}
              pinned={pinned}
              span={span}
              spanOptions={spanOptions}
              onMoveLeft={onMoveLeft}
              onMoveRight={onMoveRight}
              fullscreen={fullscreen}
              onToggleFullscreen={fullscreenable ? () => void toggleFullscreen() : undefined}
              onSpanChange={onSpanChange}
            />
          </div>
        )}
        {headerDetails ? <div className='shell-column-header-details'>{headerDetails}</div> : null}
      </header>
      <span className='sr-only' aria-live='polite'>{announcement}</span>
      <div className='shell-column-body'>
        <ColumnFullscreenContext.Provider value={fullscreen}>{children}</ColumnFullscreenContext.Provider>
      </div>
      {footer ? <footer className='shell-column-footer'>{footer}</footer> : null}
    </section>
  );
}

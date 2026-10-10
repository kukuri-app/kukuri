import { useCallback, useEffect, useRef, useState } from 'react';

import type { ChannelAccessTokenPreview, DesktopApi } from '@/lib/api';
import { parseChannelAccessPreviewDeepLink } from '@/lib/internalLinks';
import type { Translate } from '@/shell/actions/shared';
import { messageFromError } from '@/shell/presentation';

type UseSharePreviewArgs = {
  api: DesktopApi;
  importChannelAccessToken: (token: string) => Promise<void>;
  translate: Translate;
};

export function useSharePreview({
  api,
  importChannelAccessToken,
  translate,
}: UseSharePreviewArgs) {
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState<string | null>(null);
  const [data, setData] = useState<ChannelAccessTokenPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [importPending, setImportPending] = useState(false);
  const previewGeneration = useRef(0);

  const handleOpenChange = useCallback((nextOpen: boolean) => {
    setOpen(nextOpen);
    if (!nextOpen) {
      previewGeneration.current++;
      setError(null);
      setData(null);
      setToken(null);
      setLoading(false);
    }
  }, []);

  const openPreview = useCallback(
    async (nextToken: string) => {
      const generation = ++previewGeneration.current;
      setOpen(true);
      setToken(nextToken);
      setData(null);
      setError(null);
      setLoading(true);
      try {
        const preview = await api.previewChannelAccessToken(nextToken);
        if (generation === previewGeneration.current) setData(preview);
      } catch (previewError) {
        if (generation === previewGeneration.current) setError(
          messageFromError(previewError, translate('channels:errors.failedPreviewToken'))
        );
      } finally {
        if (generation === previewGeneration.current) setLoading(false);
      }
    },
    [api, translate]
  );

  const openPreviewFromUrl = useCallback(
    async (url: string) => {
      const reference = parseChannelAccessPreviewDeepLink(url);
      if (reference) {
        await openPreview(reference.token);
      }
    },
    [openPreview]
  );

  useEffect(() => {
    const generation = previewGeneration;
    const initialGeneration = generation.current;
    const handleBrowserEvent = (event: Event) => {
      const url =
        event instanceof CustomEvent && typeof event.detail?.url === 'string'
          ? event.detail.url
          : null;
      if (url) {
        void openPreviewFromUrl(url);
      }
    };
    window.addEventListener('kukuri:open-url', handleBrowserEvent);

    let disposed = false;
    let unlisten: (() => void) | undefined;
    void import('@tauri-apps/plugin-deep-link')
      .then(async ({ getCurrent, onOpenUrl }) => {
        if (disposed) {
          return;
        }
        const dispose = await onOpenUrl((urls) => {
          if (disposed) return;
          for (const url of urls) {
            void openPreviewFromUrl(url);
          }
        });
        if (disposed) { dispose(); return; }
        unlisten = dispose;
        const currentUrls = await getCurrent();
        if (!disposed && generation.current === initialGeneration) {
          for (const url of currentUrls ?? []) {
            if (disposed) break;
            await openPreviewFromUrl(url);
          }
        }
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      generation.current++;
      window.removeEventListener('kukuri:open-url', handleBrowserEvent);
      unlisten?.();
    };
  }, [openPreviewFromUrl]);

  const confirmImport = useCallback(async () => {
    if (!token) {
      return;
    }
    setImportPending(true);
    setError(null);
    try {
      await importChannelAccessToken(token);
      handleOpenChange(false);
    } catch (importError) {
      setError(messageFromError(importError, translate('channels:errors.failedJoinChannel')));
    } finally {
      setImportPending(false);
    }
  }, [handleOpenChange, importChannelAccessToken, token, translate]);

  return {
    data,
    error,
    handleOpenChange,
    importPending,
    loading,
    open,
    openPreview,
    confirmImport,
    token,
  };
}

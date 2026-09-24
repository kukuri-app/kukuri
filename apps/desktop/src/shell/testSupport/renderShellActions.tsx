/**
 * useDesktopShellActions の test 用共通ハーネス。
 *
 * 21 引数を全て vi.fn / mock api / stub で配線し、返り値のハンドラ単位で
 * 「操作 → api 呼び出し引数 + store 状態遷移」を固定する test から共有する。
 * translate は既定で key をそのまま返す stub を注入し、文言 assert を locale
 * リソースから切り離す。補間値まで固定したい test は recordingTranslate を渡す。
 */
import { renderHook } from '@testing-library/react';
import type { ChangeEvent } from 'react';
import { vi } from 'vitest';

import type { DesktopApi } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type { DesktopShellState, DraftMediaItem } from '@/shell/store';
import { useDesktopShellActions } from '@/shell/useDesktopShellActions';
import { createShellHookHarness } from '@/shell/testSupport/renderShellHook';

// key をそのまま返す stub。文言 assert を locale リソースから切り離す。
export const stubTranslate = (key: string) => key;

// 添付選択は ChangeEvent を受けるため files だけを持つ stub を渡す。
export function attachmentChangeEvent(files: File[]) {
  return { target: { files } } as unknown as ChangeEvent<HTMLInputElement>;
}

// 補間値まで固定したい文言は key と options を連結する stub を使う。
export const recordingTranslate = (key: string, options?: Record<string, unknown>) =>
  options ? `${key}:${JSON.stringify(options)}` : key;

export type RenderActionsOptions = {
  /** mock api のうち差し替えたいメソッドだけを vi.fn で上書きする。 */
  api?: Partial<DesktopApi>;
  /** 既定は key をそのまま返す stub。補間値を固定したい test だけ差し替える。 */
  translate?: (key: string, options?: Record<string, unknown>) => string;
  /** render 前の store プリセット(act 不要)。current を受けて patch を返す。 */
  preset?: (current: DesktopShellState) => Partial<DesktopShellState>;
};

// 21 引数を全て vi.fn / mock api / stub で配線する共通ハーネス。
export function renderActionsHook(options: RenderActionsOptions = {}) {
  const harness = createShellHookHarness();
  if (options.preset) {
    harness.store.getState().patchState(options.preset(harness.store.getState()));
  }

  const api: DesktopApi = { ...createDesktopMockApi(), ...options.api };
  const loadTopics = vi.fn(async () => undefined);
  const refreshVisibleTimelineAfterPublish = vi.fn(async () => undefined);
  const syncRoute = vi.fn();
  const openDirectMessagePane = vi.fn(async () => undefined);
  const openAuthorDetail = vi.fn(async () => undefined);
  const openThread = vi.fn(async () => undefined);
  const setLiveCreateDialogOpen = vi.fn();
  const setGameCreateDialogOpen = vi.fn();
  const setProfileAvatarPreviewUrl = vi.fn();
  const setProfileAvatarInputKey = vi.fn();
  const releaseDraftPreview = vi.fn();
  const rememberDraftPreview = vi.fn();
  const releaseDirectMessageDraftPreview = vi.fn();
  const releaseAllDirectMessageDraftPreviews = vi.fn();
  const rememberDirectMessageDraftPreview = vi.fn();
  const buildImageDraftItem = vi.fn(
    async (file: File): Promise<DraftMediaItem> => ({
      id: `image-item-${file.name}`,
      source_name: file.name,
      preview_url: `blob:image-item-${file.name}`,
      attachments: [],
    })
  );
  const buildVideoDraftItem = vi.fn(
    async (file: File): Promise<DraftMediaItem> => ({
      id: `video-item-${file.name}`,
      source_name: file.name,
      preview_url: `blob:video-item-${file.name}`,
      attachments: [],
    })
  );

  const rendered = renderHook(
    () =>
      useDesktopShellActions({
        api,
        translate: options.translate ?? stubTranslate,
        loadTopics,
        refreshProfile: vi.fn(async () => undefined),
        refreshBookmarks: vi.fn(async () => undefined),
        refreshVisibleTimelineAfterPublish,
        syncRoute,
        openDirectMessagePane,
        openAuthorDetail,
        openThread,
        setLiveCreateDialogOpen,
        setGameCreateDialogOpen,
        setProfileAvatarPreviewUrl,
        setProfileAvatarInputKey,
        releaseDraftPreview,
        rememberDraftPreview,
        releaseDirectMessageDraftPreview,
        releaseAllDirectMessageDraftPreviews,
        rememberDirectMessageDraftPreview,
        buildImageDraftItem,
        buildVideoDraftItem,
      }),
    { wrapper: harness.wrapper }
  );

  return {
    ...rendered,
    store: harness.store,
    api,
    mocks: {
      loadTopics,
      refreshVisibleTimelineAfterPublish,
      syncRoute,
      openDirectMessagePane,
      openAuthorDetail,
      openThread,
      setLiveCreateDialogOpen,
      setGameCreateDialogOpen,
      setProfileAvatarPreviewUrl,
      setProfileAvatarInputKey,
      releaseDraftPreview,
      rememberDraftPreview,
      releaseDirectMessageDraftPreview,
      releaseAllDirectMessageDraftPreviews,
      rememberDirectMessageDraftPreview,
      buildImageDraftItem,
      buildVideoDraftItem,
    },
  };
}


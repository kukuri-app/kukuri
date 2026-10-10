import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import { buildChannelAccessPreviewDeepLink } from '@/lib/internalLinks';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { useSharePreview } from '@/shell/page/useSharePreview';

const deepLinkMock = vi.hoisted(() => ({
  currentUrls: [] as string[],
  currentResult: null as Promise<string[]> | null,
  onOpenUrl: vi.fn(),
  unlisten: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-deep-link', () => ({
  getCurrent: vi.fn(async () => deepLinkMock.currentResult ?? deepLinkMock.currentUrls),
  onOpenUrl: deepLinkMock.onOpenUrl,
}));

const preview = {
  channel_id: 'channel-imported',
  topic_id: 'kukuri:topic:private-imported',
  channel_label: 'Imported',
  inviter_pubkey: 'f'.repeat(64),
  owner_pubkey: 'f'.repeat(64),
  epoch_id: 'epoch-imported-1',
  expires_at: null,
  namespace_secret_hex: 'a'.repeat(64),
  kind: 'invite' as const,
};

function setup() {
  const api = createDesktopMockApi({ invitePreview: preview });
  const importChannelAccessToken = vi.fn().mockResolvedValue(undefined);
  const translate = vi.fn((key: string) => key);
  const hook = renderHook(() =>
    useSharePreview({ api, importChannelAccessToken, translate })
  );
  return { api, hook, importChannelAccessToken };
}

afterEach(() => {
  deepLinkMock.currentUrls = [];
  deepLinkMock.currentResult = null;
  deepLinkMock.onOpenUrl.mockReset();
  deepLinkMock.unlisten.mockReset();
});

describe('useSharePreview', () => {
  test('keeps a warm link when an earlier current-URL lookup finishes later', async () => {
    let finishCurrent!: (urls: string[]) => void;
    deepLinkMock.currentResult = new Promise((resolve) => { finishCurrent = resolve; });
    deepLinkMock.onOpenUrl.mockResolvedValue(deepLinkMock.unlisten);
    const { api, hook, importChannelAccessToken } = setup();
    const previewSpy = vi.spyOn(api, 'previewChannelAccessToken');
    await waitFor(() => expect(deepLinkMock.onOpenUrl).toHaveBeenCalled());
    await act(async () => {
      deepLinkMock.onOpenUrl.mock.calls[0][0]([buildChannelAccessPreviewDeepLink('invite:warm')]);
    });
    await act(async () => { finishCurrent([buildChannelAccessPreviewDeepLink('invite:old-current')]); });
    expect(hook.result.current.token).toBe('invite:warm');
    expect(previewSpy).toHaveBeenCalledTimes(1);
    expect(importChannelAccessToken).not.toHaveBeenCalled();
  });

  test('keeps the selected token and its preview together when an older lookup finishes late', async () => {
    deepLinkMock.onOpenUrl.mockResolvedValue(deepLinkMock.unlisten);
    const { api, hook, importChannelAccessToken } = setup();
    let resolveOld!: (value: typeof preview) => void;
    vi.spyOn(api, 'previewChannelAccessToken').mockReturnValueOnce(
      new Promise((resolve) => { resolveOld = resolve; })
    );
    let oldRequest!: Promise<void>;
    act(() => { oldRequest = hook.result.current.openPreview('invite:old'); });
    await act(async () => { await hook.result.current.openPreview('invite:current'); });
    await act(async () => {
      resolveOld({ ...preview, channel_id: 'old-channel' });
      await oldRequest;
    });
    expect(hook.result.current.token).toBe('invite:current');
    expect(hook.result.current.data?.channel_id).toBe(preview.channel_id);
    expect(importChannelAccessToken).not.toHaveBeenCalled();
  });

  test('releases the secret and preview when closed during a lookup', async () => {
    deepLinkMock.onOpenUrl.mockResolvedValue(deepLinkMock.unlisten);
    const { api, hook, importChannelAccessToken } = setup();
    let resolvePreview!: (value: typeof preview) => void;
    vi.spyOn(api, 'previewChannelAccessToken').mockReturnValueOnce(
      new Promise((resolve) => { resolvePreview = resolve; })
    );
    let request!: Promise<void>;
    act(() => { request = hook.result.current.openPreview('invite:cancelled'); });
    act(() => { hook.result.current.handleOpenChange(false); });
    await act(async () => { resolvePreview(preview); await request; });
    expect(hook.result.current.open).toBe(false);
    expect(hook.result.current.token).toBeNull();
    expect(hook.result.current.data).toBeNull();
    expect(hook.result.current.loading).toBe(false);
    expect(importChannelAccessToken).not.toHaveBeenCalled();
  });

  test('disposes a native listener that finishes registering after unmount', async () => {
    let finishRegistration!: (dispose: () => void) => void;
    deepLinkMock.onOpenUrl.mockReturnValueOnce(
      new Promise((resolve) => { finishRegistration = resolve; })
    );
    const { hook } = setup();
    await waitFor(() => expect(deepLinkMock.onOpenUrl).toHaveBeenCalled());
    hook.unmount();
    await act(async () => { finishRegistration(deepLinkMock.unlisten); });
    expect(deepLinkMock.unlisten).toHaveBeenCalledTimes(1);
  });

  test('opens a preview from a browser deep-link event and imports only after confirmation', async () => {
    deepLinkMock.onOpenUrl.mockResolvedValue(deepLinkMock.unlisten);
    const { api, hook, importChannelAccessToken } = setup();
    const previewSpy = vi.spyOn(api, 'previewChannelAccessToken');
    const token = 'invite:encoded-token';

    act(() => {
      window.dispatchEvent(
        new CustomEvent('kukuri:open-url', {
          detail: { url: buildChannelAccessPreviewDeepLink(token) },
        })
      );
    });

    await waitFor(() =>
      expect(hook.result.current.data).toMatchObject({
        channel_id: preview.channel_id,
        kind: preview.kind,
        topic_id: preview.topic_id,
      })
    );
    expect(previewSpy).toHaveBeenCalledWith(token);
    expect(importChannelAccessToken).not.toHaveBeenCalled();

    await act(async () => {
      await hook.result.current.confirmImport();
    });
    expect(importChannelAccessToken).toHaveBeenCalledWith(token);
    expect(hook.result.current.open).toBe(false);
    expect(hook.result.current.token).toBeNull();
  });

  test('consumes current Tauri URLs and releases both listeners on unmount', async () => {
    const token = 'invite:startup-token';
    deepLinkMock.currentUrls = [buildChannelAccessPreviewDeepLink(token)];
    deepLinkMock.onOpenUrl.mockResolvedValue(deepLinkMock.unlisten);
    const { api, hook } = setup();
    const previewSpy = vi.spyOn(api, 'previewChannelAccessToken');

    await waitFor(() => expect(previewSpy).toHaveBeenCalledWith(token));
    hook.unmount();
    expect(deepLinkMock.unlisten).toHaveBeenCalled();

    window.dispatchEvent(
      new CustomEvent('kukuri:open-url', {
        detail: { url: buildChannelAccessPreviewDeepLink('invite:after-unmount') },
      })
    );
    expect(previewSpy).toHaveBeenCalledTimes(1);
  });
});

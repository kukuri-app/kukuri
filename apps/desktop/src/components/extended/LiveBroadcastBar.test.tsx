import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, test, vi } from 'vitest';

import type { LiveBroadcastStatus, LiveVideoSource, SetLiveBroadcastRequest } from '@/lib/api';
import { LiveBroadcastBar } from './LiveBroadcastBar';
import { LiveBroadcastController } from './liveBroadcast';
import { captureVideo, encodeVideo, liveThumbnailDataUrl } from './liveMedia';

vi.mock('./liveMedia', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./liveMedia')>()),
  captureVideo: vi.fn(),
  encodeVideo: vi.fn(),
  liveThumbnailDataUrl: vi.fn(),
}));

const LIMITS = { width: 1280, height: 720, framerate: 30, bitrate: 1_500_000 };
const request = { topic: 'kukuri:topic:test', session_id: 'live-1' };
const target = { topic: request.topic, sessionId: 'live-1', title: 'Launch Party' };

function api() {
  return {
    setLiveBroadcast: vi.fn<(request: SetLiveBroadcastRequest) => Promise<LiveBroadcastStatus>>(async () => ({
      microphone: false,
      limits: LIMITS,
    })),
    pushLiveVideo: vi.fn(async () => undefined),
    stopLiveBroadcast: vi.fn(async () => undefined),
  };
}

/// 配信を始め(card の操作)、shell の上部の bar を描く。
function broadcast(media: ReturnType<typeof api>, source: LiveVideoSource = 'camera') {
  const controller = new LiveBroadcastController(media);
  render(<LiveBroadcastBar controller={controller} />);
  act(() => controller.start(target, source));
  return controller;
}

describe('LiveBroadcastBar', () => {
  test('updates a snapshot every thirty seconds and releases the timer when stopped', async () => {
    vi.useFakeTimers();
    const media = { ...api(), setLiveThumbnail: vi.fn(async () => undefined) };
    vi.mocked(captureVideo).mockResolvedValue({} as MediaStream);
    vi.mocked(encodeVideo).mockReturnValue({ stop: vi.fn() });
    vi.mocked(liveThumbnailDataUrl).mockReturnValue('data:image/jpeg;base64,snapshot');
    const controller = new LiveBroadcastController(media);
    Object.defineProperties(controller.preview, { videoWidth: { value: 640 }, videoHeight: { value: 360 } });
    try {
      controller.start(target, 'camera');
      await vi.advanceTimersByTimeAsync(1);
      expect(media.setLiveThumbnail).toHaveBeenCalledWith({ ...request, custom: false, data_url: 'data:image/jpeg;base64,snapshot' });
      await vi.advanceTimersByTimeAsync(30_000);
      expect(media.setLiveThumbnail).toHaveBeenCalledTimes(2);
      controller.stop();
      await vi.advanceTimersByTimeAsync(60_000);
      expect(media.setLiveThumbnail).toHaveBeenCalledTimes(2);
    } finally { controller.stop(); vi.useRealTimers(); }
  });
  test('shows nothing until a broadcast starts', () => {
    const { container } = render(<LiveBroadcastBar controller={new LiveBroadcastController(api())} />);
    expect(container).toBeEmptyDOMElement();
  });

  test('selects another camera and replaces the capture without closing the microphone', async () => {
    const media = api();
    media.setLiveBroadcast.mockImplementation(async ({ microphone }) => ({ microphone, limits: LIMITS }));
    const stopEncoding = vi.fn();
    const cameras = ['front', 'rear'].map((deviceId) => ({ kind: 'videoinput', deviceId, label: deviceId } as MediaDeviceInfo));
    const devices = { enumerateDevices: vi.fn(async () => cameras) };
    vi.stubGlobal('navigator', { mediaDevices: devices });
    vi.mocked(captureVideo).mockImplementation(async (_source, _limits, cameraId) => ({
      getVideoTracks: () => [{ getSettings: () => ({ deviceId: cameraId || 'front' }) }],
    }) as unknown as MediaStream);
    vi.mocked(encodeVideo).mockReturnValue({ stop: stopEncoding });
    const controller = broadcast(media);
    try {
      const select = await screen.findByRole('combobox', { name: 'Camera to use' });
      expect(select).toHaveValue('front');
      await userEvent.selectOptions(select, 'rear');
      await waitFor(() => expect(controller.getState()?.phase).toBe('live'));
      expect(captureVideo).toHaveBeenLastCalledWith('camera', LIMITS, 'rear');
      expect(stopEncoding).toHaveBeenCalledTimes(1);
      expect(media.setLiveBroadcast).toHaveBeenLastCalledWith({ ...request, source: 'camera', microphone: true });
      expect(select).toHaveValue('rear');
    } finally {
      controller.stop();
      vi.unstubAllGlobals();
    }
  });

  // ADR 0067 の live_media_requires_explicit_start_and_permission(画面側): 取り込みが拒否されたら配信を止め、mic を開かない。
  test('stops the broadcast without opening the microphone when capture is refused', async () => {
    const media = api();
    vi.mocked(captureVideo).mockRejectedValue(Object.assign(new Error('denied'), { name: 'NotAllowedError' }));
    const controller = broadcast(media, 'screen');

    expect(await screen.findByRole('alert')).toHaveTextContent('Camera or screen capture was not allowed.');
    expect(media.setLiveBroadcast).toHaveBeenCalledTimes(1);
    expect(media.setLiveBroadcast).toHaveBeenCalledWith({ ...request, source: 'screen', microphone: false });
    expect(media.stopLiveBroadcast).toHaveBeenCalledWith(request);
    expect(encodeVideo).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(controller.getState()).toBeNull();
  });

  // この端末で作っていない session は、取り込まずに理由を示す(2026-10-13 のユーザー判断)。
  test('explains that a session created elsewhere cannot be broadcast here', async () => {
    const media = api();
    media.setLiveBroadcast.mockRejectedValue(
      Object.assign(new Error('elsewhere'), { name: 'InvokeError', code: 'LIVE_BROADCAST_ELSEWHERE' })
    );
    broadcast(media);
    expect(await screen.findByRole('alert')).toHaveTextContent(
      "This live session can't be broadcast from this device."
    );
    expect(captureVideo).not.toHaveBeenCalled();
  });

  test('opens the microphone after capture starts and can turn it off and stop', async () => {
    const media = api();
    const stopEncoding = vi.fn();
    vi.mocked(captureVideo).mockResolvedValue({} as MediaStream);
    vi.mocked(encodeVideo).mockReturnValue({ stop: stopEncoding });
    media.setLiveBroadcast.mockImplementation(async ({ microphone }) => ({ microphone, limits: LIMITS }));
    const controller = broadcast(media);

    expect(await screen.findByText('Broadcasting video')).toBeInTheDocument();
    expect(media.setLiveBroadcast.mock.calls.map(([call]) => call.microphone)).toEqual([false, true]);
    expect(vi.mocked(encodeVideo).mock.calls[0][0].limits).toEqual(LIMITS);

    // 配信中に画面へ切り替えても mic は閉じない。
    await userEvent.click(screen.getByRole('button', { name: 'Screen' }));
    await waitFor(() => expect(encodeVideo).toHaveBeenCalledTimes(2));
    expect(stopEncoding).toHaveBeenCalledTimes(1);
    expect(media.setLiveBroadcast).toHaveBeenLastCalledWith({ ...request, source: 'screen', microphone: true });

    await userEvent.click(await screen.findByRole('button', { name: 'Disable microphone' }));
    await waitFor(() =>
      expect(media.setLiveBroadcast).toHaveBeenLastCalledWith({ ...request, source: 'screen', microphone: false })
    );
    expect(screen.getByRole('button', { name: 'Enable microphone' })).toHaveAttribute('aria-pressed', 'false');

    // 停止で取り込みと送信を止め、bar を閉じる。
    await userEvent.click(screen.getByRole('button', { name: 'Stop video broadcast' }));
    expect(stopEncoding).toHaveBeenCalledTimes(2);
    expect(media.stopLiveBroadcast).toHaveBeenCalledWith(request);
    expect(controller.getState()).toBeNull();
    expect(screen.queryByRole('region')).toBeNull();
  });

  test('shows a microphone that could not be opened', async () => {
    const media = api();
    vi.mocked(captureVideo).mockResolvedValue({} as MediaStream);
    vi.mocked(encodeVideo).mockReturnValue({ stop: vi.fn() });
    broadcast(media);
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'The microphone is unavailable, so only video is being broadcast.'
    );
    expect(screen.getByRole('button', { name: 'Enable microphone' })).toHaveAttribute('aria-pressed', 'false');
  });

  // 取り込みの許可を待つ間に配信を止めたら(停止・Live の終了)、後から得た取り込みを止める(T4: 残留 stream の回収)。
  test('stops a capture that arrives after the broadcast is stopped', async () => {
    const media = api();
    let resolveCapture: (stream: MediaStream) => void = () => undefined;
    vi.mocked(captureVideo).mockReturnValue(new Promise((resolve) => (resolveCapture = resolve)));
    const controller = broadcast(media);
    await waitFor(() => expect(captureVideo).toHaveBeenCalled());
    act(() => controller.stopIfSession('live-1'));
    const track = { stop: vi.fn() };
    resolveCapture({ getTracks: () => [track] } as unknown as MediaStream);
    await waitFor(() => expect(track.stop).toHaveBeenCalled());
    expect(encodeVideo).not.toHaveBeenCalled();
    expect(media.stopLiveBroadcast).toHaveBeenCalledWith(request);
  });

  // 同時に配信するのは 1 つの Live だけ。
  test('does not start a second live session while one is broadcasting', () => {
    const media = api();
    vi.mocked(captureVideo).mockReturnValue(new Promise(() => undefined));
    const controller = broadcast(media);
    act(() => controller.start({ ...target, sessionId: 'live-2' }, 'camera'));
    expect(controller.getState()?.sessionId).toBe('live-1');
  });
});

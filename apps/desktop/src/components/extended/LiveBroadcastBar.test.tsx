import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, test, vi } from 'vitest';

import type { LiveBroadcastStatus, SetLiveBroadcastRequest } from '@/lib/api';
import { LiveBroadcastBar } from './LiveBroadcastBar';
import { captureVideo, encodeVideo } from './liveMedia';

vi.mock('./liveMedia', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./liveMedia')>()),
  captureVideo: vi.fn(),
  encodeVideo: vi.fn(),
}));

const LIMITS = { width: 1280, height: 720, framerate: 30, bitrate: 1_500_000 };

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

const request = { topic: 'kukuri:topic:test', session_id: 'live-1' };
const onClose = vi.fn();

describe('LiveBroadcastBar', () => {
  // ADR 0067 の live_media_requires_explicit_start_and_permission(画面側): 取り込みが拒否されたら配信を止め、mic を開かない。
  test('stops the broadcast without opening the microphone when capture is refused', async () => {
    const media = api();
    vi.mocked(captureVideo).mockRejectedValue(Object.assign(new Error('denied'), { name: 'NotAllowedError' }));
    render(<LiveBroadcastBar api={media} topic={request.topic} sessionId='live-1' title='Launch Party' onClose={onClose} />);
    expect(media.setLiveBroadcast).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('button', { name: 'Screen' }));
    await userEvent.click(screen.getByRole('button', { name: 'Start video broadcast' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Camera or screen capture was not allowed.');
    expect(media.setLiveBroadcast).toHaveBeenCalledTimes(1);
    expect(media.setLiveBroadcast).toHaveBeenCalledWith({ ...request, source: 'screen', microphone: false });
    expect(media.stopLiveBroadcast).toHaveBeenCalledWith(request);
    expect(encodeVideo).not.toHaveBeenCalled();
  });

  // この端末で作っていない session は、取り込まずに理由を示す(2026-10-13 のユーザー判断)。
  test('explains that a session created elsewhere cannot be broadcast here', async () => {
    const media = api();
    media.setLiveBroadcast.mockRejectedValue(
      Object.assign(new Error('elsewhere'), { name: 'InvokeError', code: 'LIVE_BROADCAST_ELSEWHERE' })
    );
    render(<LiveBroadcastBar api={media} topic={request.topic} sessionId='live-1' title='Launch Party' onClose={onClose} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start video broadcast' }));
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
    render(<LiveBroadcastBar api={media} topic={request.topic} sessionId='live-1' title='Launch Party' onClose={onClose} />);

    await userEvent.click(screen.getByRole('button', { name: 'Start video broadcast' }));
    expect(await screen.findByText('Broadcasting video')).toBeInTheDocument();
    expect(media.setLiveBroadcast.mock.calls.map(([call]) => call.microphone)).toEqual([false, true]);
    expect(vi.mocked(encodeVideo).mock.calls[0][0].limits).toEqual(LIMITS);

    // 配信中に画面へ切り替えても mic は閉じない。
    await userEvent.click(screen.getByRole('button', { name: 'Screen' }));
    await waitFor(() => expect(encodeVideo).toHaveBeenCalledTimes(2));
    expect(stopEncoding).toHaveBeenCalledTimes(1);
    expect(media.setLiveBroadcast).toHaveBeenLastCalledWith({ ...request, source: 'screen', microphone: true });

    await userEvent.click(screen.getByRole('button', { name: 'Disable microphone' }));
    await waitFor(() =>
      expect(media.setLiveBroadcast).toHaveBeenLastCalledWith({ ...request, source: 'screen', microphone: false })
    );
    expect(screen.getByRole('button', { name: 'Enable microphone' })).toHaveAttribute('aria-pressed', 'false');

    await userEvent.click(screen.getByRole('button', { name: 'Stop video broadcast' }));
    expect(stopEncoding).toHaveBeenCalled();
    expect(media.stopLiveBroadcast).toHaveBeenCalledWith(request);
  });

  test('shows a microphone that could not be opened', async () => {
    const media = api();
    vi.mocked(captureVideo).mockResolvedValue({} as MediaStream);
    vi.mocked(encodeVideo).mockReturnValue({ stop: vi.fn() });
    render(<LiveBroadcastBar api={media} topic={request.topic} sessionId='live-1' title='Launch Party' onClose={onClose} />);

    await userEvent.click(screen.getByRole('button', { name: 'Start video broadcast' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'The microphone is unavailable, so only video is being broadcast.'
    );
    expect(screen.getByRole('button', { name: 'Enable microphone' })).toHaveAttribute('aria-pressed', 'false');
  });

  // 取り込みの許可を待つ間に bar を閉じたら、後から得た取り込みを止め、配信も止める(T4: 残留 stream の回収)。
  test('stops a capture that arrives after the bar is closed', async () => {
    const media = api();
    let resolveCapture: (stream: MediaStream) => void = () => undefined;
    vi.mocked(captureVideo).mockReturnValue(new Promise((resolve) => (resolveCapture = resolve)));
    const { unmount } = render(<LiveBroadcastBar api={media} topic={request.topic} sessionId='live-1' title='Launch Party' onClose={onClose} />);
    await userEvent.click(screen.getByRole('button', { name: 'Start video broadcast' }));
    await waitFor(() => expect(captureVideo).toHaveBeenCalled());
    unmount();
    const track = { stop: vi.fn() };
    resolveCapture({ getTracks: () => [track] } as unknown as MediaStream);
    await waitFor(() => expect(track.stop).toHaveBeenCalled());
    expect(encodeVideo).not.toHaveBeenCalled();
    expect(media.stopLiveBroadcast).toHaveBeenCalledTimes(2);
  });

  // #1839 AC-3: 閉じる操作で bar を閉じる(閉じると取り込みと配信を止める。上の試験)。
  test('closes from its own button', async () => {
    render(<LiveBroadcastBar api={api()} topic={request.topic} sessionId='live-1' title='Launch Party' onClose={onClose} />);
    expect(screen.getByRole('region', { name: 'Broadcast controls for Launch Party' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalled();
  });
});

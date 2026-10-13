import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, test, vi } from 'vitest';

import type { LiveBroadcastStatus, LiveSessionView, SetLiveBroadcastRequest } from '@/lib/api';
import { LiveSessionMedia } from './LiveSessionMedia';
import { captureVideo, createLivePlayer, encodeVideo } from './liveMedia';

vi.mock('./liveMedia', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./liveMedia')>()),
  captureVideo: vi.fn(),
  encodeVideo: vi.fn(),
  createLivePlayer: vi.fn(),
}));

const LIMITS = { width: 1280, height: 720, framerate: 30, bitrate: 1_500_000 };

function api(nextState = 2) {
  return {
    setLiveBroadcast: vi.fn<(request: SetLiveBroadcastRequest) => Promise<LiveBroadcastStatus>>(async () => ({
      microphone: false,
      limits: LIMITS,
    })),
    pushLiveVideo: vi.fn(async () => undefined),
    stopLiveBroadcast: vi.fn(async () => undefined),
    watchLiveMedia: vi.fn(async () => undefined),
    nextLiveVideo: vi.fn(async () => {
      const batch = new ArrayBuffer(11);
      new DataView(batch).setUint8(0, nextState);
      return batch;
    }),
    stopWatchingLiveMedia: vi.fn(async () => undefined),
  };
}

function session(overrides: Partial<LiveSessionView> = {}): LiveSessionView {
  return {
    session_id: 'live-1',
    host_pubkey: 'host',
    title: 'Launch Party',
    description: '',
    status: 'Live',
    started_at: 0,
    ended_at: null,
    viewer_count: 0,
    joined_by_me: false,
    channel_id: null,
    audience_label: 'Public',
    ...overrides,
  } as LiveSessionView;
}

const request = { topic: 'kukuri:topic:test', session_id: 'live-1' };

describe('LiveSessionMedia', () => {
  beforeEach(() => {
    vi.mocked(createLivePlayer).mockReturnValue({ push: vi.fn(), reset: vi.fn(), close: vi.fn() });
  });

  // ADR 0067 の live_media_requires_explicit_start_and_permission(画面側): 取り込みが拒否されたら配信を止め、mic を開かない。
  test('stops the broadcast without opening the microphone when capture is refused', async () => {
    const media = api();
    vi.mocked(captureVideo).mockRejectedValue(Object.assign(new Error('denied'), { name: 'NotAllowedError' }));
    render(<LiveSessionMedia api={media} topic={request.topic} session={session()} isOwner />);
    expect(media.setLiveBroadcast).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('button', { name: 'Screen' }));
    await userEvent.click(screen.getByRole('button', { name: 'Start video broadcast' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Camera or screen capture was not allowed.');
    expect(media.setLiveBroadcast).toHaveBeenCalledTimes(1);
    expect(media.setLiveBroadcast).toHaveBeenCalledWith({ ...request, source: 'screen', microphone: false });
    expect(media.stopLiveBroadcast).toHaveBeenCalledWith(request);
    expect(encodeVideo).not.toHaveBeenCalled();
  });

  test('opens the microphone after capture starts and can turn it off and stop', async () => {
    const media = api();
    const stopEncoding = vi.fn();
    vi.mocked(captureVideo).mockResolvedValue({} as MediaStream);
    vi.mocked(encodeVideo).mockReturnValue({ stop: stopEncoding });
    media.setLiveBroadcast.mockImplementation(async ({ microphone }) => ({ microphone, limits: LIMITS }));
    render(<LiveSessionMedia api={media} topic={request.topic} session={session()} isOwner />);

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
    render(<LiveSessionMedia api={media} topic={request.topic} session={session()} isOwner />);

    await userEvent.click(screen.getByRole('button', { name: 'Start video broadcast' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'The microphone is unavailable, so only video is being broadcast.'
    );
    expect(screen.getByRole('button', { name: 'Enable microphone' })).toHaveAttribute('aria-pressed', 'false');
  });

  // 5 人目以降は満員と示す(#1034 AC-2a)。退出・画面を離れたら購読を止める。
  test('shows a full broadcast to a joined viewer and stops watching when it unmounts', async () => {
    const media = api(1);
    const { unmount } = render(
      <LiveSessionMedia api={media} topic={request.topic} session={session({ joined_by_me: true })} isOwner={false} />
    );
    expect(await screen.findByText(/This broadcast is full/)).toBeInTheDocument();
    expect(media.watchLiveMedia).toHaveBeenCalledWith(request);
    unmount();
    expect(media.stopWatchingLiveMedia).toHaveBeenCalledWith(request);
  });

  test('shows nothing to viewers who have not joined or after the session ends', () => {
    const media = api();
    const { container, rerender } = render(
      <LiveSessionMedia api={media} topic={request.topic} session={session()} isOwner={false} />
    );
    expect(container).toBeEmptyDOMElement();
    rerender(
      <LiveSessionMedia
        api={media}
        topic={request.topic}
        session={session({ status: 'Ended', joined_by_me: true })}
        isOwner
      />
    );
    expect(container).toBeEmptyDOMElement();
    expect(media.watchLiveMedia).not.toHaveBeenCalled();
  });
});

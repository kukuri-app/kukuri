import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';

import type { LiveSessionView } from '@/lib/api';
import { LiveSessionMedia } from './LiveSessionMedia';
import { createLivePlayer } from './liveMedia';
import { LIVE_VOLUME_LIMIT, setLiveWatchVolume } from './liveWatch';

vi.mock('./liveMedia', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./liveMedia')>()),
  createLivePlayer: vi.fn(),
}));

function api(nextState = 2) {
  return {
    watchLiveMedia: vi.fn(async () => undefined),
    nextLiveVideo: vi.fn(async () => {
      const batch = new ArrayBuffer(11);
      new DataView(batch).setUint8(0, nextState);
      return batch;
    }),
    stopWatchingLiveMedia: vi.fn(async () => undefined),
    setLiveVolume: vi.fn(async () => undefined),
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

  test('stops watching only after an in-flight start finishes', async () => {
    const media = api();
    let finishWatch: () => void = () => undefined;
    media.watchLiveMedia.mockReturnValue(new Promise<undefined>((resolve) => (finishWatch = () => resolve(undefined))));
    const { unmount } = render(
      <LiveSessionMedia api={media} topic={request.topic} session={session({ joined_by_me: true })} isOwner={false} />
    );
    await waitFor(() => expect(media.watchLiveMedia).toHaveBeenCalled());
    unmount();
    await Promise.resolve();
    expect(media.stopWatchingLiveMedia).not.toHaveBeenCalled();
    finishWatch();
    await waitFor(() => expect(media.stopWatchingLiveMedia).toHaveBeenCalledWith(request));
  });

  // 配信者と転送する視聴者のどこにも空きが無ければ、参加できないと示す(#1034 AC-2b)。退出・画面を離れたら購読を止める。
  test('shows a full broadcast to a joined viewer and stops watching when it unmounts', async () => {
    const media = api(1);
    const { unmount } = render(
      <LiveSessionMedia api={media} topic={request.topic} session={session({ joined_by_me: true })} isOwner={false} />
    );
    expect(await screen.findByText(/No free spot to watch was found/)).toBeInTheDocument();
    expect(media.watchLiveMedia).toHaveBeenCalledWith(request);
    unmount();
    await waitFor(() => expect(media.stopWatchingLiveMedia).toHaveBeenCalledWith(request));
  });

  // #1839 AC-3: 配信の操作は shell の上部の bar にあり、主催者の card には映像の欄を出さない。
  test('shows nothing to viewers who have not joined, to the host, or after the session ends', () => {
    const media = api();
    const { container, rerender } = render(
      <LiveSessionMedia api={media} topic={request.topic} session={session()} isOwner={false} />
    );
    expect(container).toBeEmptyDOMElement();
    rerender(
      <LiveSessionMedia api={media} topic={request.topic} session={session({ joined_by_me: true })} isOwner />
    );
    expect(container).toBeEmptyDOMElement();
    rerender(
      <LiveSessionMedia
        api={media}
        topic={request.topic}
        session={session({ status: 'Ended', joined_by_me: true })}
        isOwner={false}
      />
    );
    expect(container).toBeEmptyDOMElement();
    expect(media.watchLiveMedia).not.toHaveBeenCalled();
  });

  // #1839 AC-4: 同じ Live の 2 つの表示は 1 つの視聴(購読・音声)を共有し、映像は両方の canvas へ届く。片方を閉じても
  // 続き、最後を閉じると止める。
  test('two views of the same live session share one watch until the last one closes', async () => {
    const media = api(0);
    // 視聴中の(空の)映像を少しずつ返す。
    media.nextLiveVideo.mockImplementation(async () => {
      await new Promise((resolve) => setTimeout(resolve, 10));
      return new ArrayBuffer(11);
    });
    const players = [0, 1].map(() => ({ push: vi.fn(), reset: vi.fn(), close: vi.fn() }));
    vi.mocked(createLivePlayer).mockReturnValueOnce(players[0]).mockReturnValueOnce(players[1]);
    const joined = session({ joined_by_me: true });
    const first = render(<LiveSessionMedia api={media} topic={request.topic} session={joined} isOwner={false} />);
    const second = render(<LiveSessionMedia api={media} topic={request.topic} session={joined} isOwner={false} />);
    await waitFor(() => expect(players[0].push).toHaveBeenCalled());
    await waitFor(() => expect(players[1].push).toHaveBeenCalled());
    expect(media.watchLiveMedia).toHaveBeenCalledTimes(1);
    first.unmount();
    // 残った表示には映像が届き続け、視聴は止まらない。
    const pushed = players[1].push.mock.calls.length;
    await waitFor(() => expect(players[1].push.mock.calls.length).toBeGreaterThan(pushed));
    expect(media.stopWatchingLiveMedia).not.toHaveBeenCalled();
    second.unmount();
    await waitFor(() => expect(media.stopWatchingLiveMedia).toHaveBeenCalledTimes(1));
    expect(media.stopWatchingLiveMedia).toHaveBeenCalledWith(request);
  });

  // 音量は Live ごと。同じ Live の表示は同じ音量を示し、別の Live の音量は変わらない。0 で無音にできる。
  test('the volume is set per live session and shared by its views', async () => {
    const media = api();
    const other = { ...request, session_id: 'live-2' };
    const joined = session({ joined_by_me: true });
    render(
      <>
        <LiveSessionMedia api={media} topic={request.topic} session={joined} isOwner={false} />
        <LiveSessionMedia api={media} topic={request.topic} session={joined} isOwner={false} />
        <LiveSessionMedia api={media} topic={request.topic} session={session({ session_id: 'live-2', joined_by_me: true })} isOwner={false} />
      </>
    );
    await waitFor(() => expect(media.watchLiveMedia).toHaveBeenCalledTimes(2));
    const sliders = screen.getAllByRole('slider', { name: 'Volume' });
    fireEvent.change(sliders[0], { target: { value: '0' } });
    expect(media.setLiveVolume).toHaveBeenLastCalledWith({ session_id: 'live-1', volume: 0 });
    expect(sliders.map((slider) => (slider as HTMLInputElement).value)).toEqual(['0', '0', '100']);
    setLiveWatchVolume(media, other, 0.4);
    expect(media.setLiveVolume).toHaveBeenLastCalledWith({ session_id: 'live-2', volume: 0.4 });
    expect((sliders[0] as HTMLInputElement).value).toBe('0');
  });

  // 端末に残す音量は最近変えた Live の上限件数だけ(古いものから消す)。
  test('keeps only the most recently changed volumes', () => {
    const media = api();
    for (let index = 0; index < LIVE_VOLUME_LIMIT + 5; index += 1) {
      setLiveWatchVolume(media, { ...request, session_id: `live-${index}` }, 0.5);
    }
    const saved = JSON.parse(localStorage.getItem('kukuri.desktop.live-volume') ?? '{}') as Record<string, number>;
    expect(Object.keys(saved)).toHaveLength(LIVE_VOLUME_LIMIT);
    expect(saved['live-0']).toBeUndefined();
    expect(saved[`live-${LIVE_VOLUME_LIMIT + 4}`]).toBe(0.5);
  });
});

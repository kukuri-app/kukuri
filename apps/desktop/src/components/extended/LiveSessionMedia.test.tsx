import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, test, vi } from 'vitest';

import type { LiveSessionView } from '@/lib/api';
import { LiveSessionMedia } from './LiveSessionMedia';
import { createLivePlayer } from './liveMedia';

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
});

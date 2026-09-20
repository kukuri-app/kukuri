import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import {
  createDeferred,
  getDetailPane,
  getActiveColumn,
  buildImagePost,
  buildVideoPost,
  installObjectUrlMocks,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';
import type { BlobViewStatus } from '@/lib/api';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';
import {
  MEDIA_FETCH_MAX_AUTO_ATTEMPTS,
  MEDIA_FETCH_RETRY_DELAYS_MS,
  mediaFetchRetryPolicy,
} from '@/shell/data/mediaFetchLedger';

// #1207: 上限到達までに取得の往復を複数回はさむ。全体実行の負荷でも待てる長さにする。
const FAILURE_WAIT = { timeout: 10_000 };

beforeEach(() => {
  // #1207: 自動再試行の待ち時間を 0 にし、実時間を待たずに上限到達まで進める。
  mediaFetchRetryPolicy.retryDelaysMs = [0, 0];
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  mediaFetchRetryPolicy.retryDelaysMs = MEDIA_FETCH_RETRY_DELAYS_MS;
  vi.useRealTimers();
});

test('timeline image stops loading and shows the fetch failure after null responses in normal mode', async () => {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
  const payload = createDeferred<null>();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildImagePost({ content: 'available caption', content_status: 'Available' }),
      ],
    },
  });
  api.getBlobMediaPayload = async () => payload.promise;

  render(<App api={api} />);

  expect(await within(getActiveColumn('Timeline')).findByTestId('media-skeleton-image-post')).toBeInTheDocument();
  act(() => payload.resolve(null));

  await waitFor(() => {
    expect(screen.queryByTestId('media-skeleton-image-post')).not.toBeInTheDocument();
  });
  expect(screen.queryByText('image/png')).not.toBeInTheDocument();
  const timeline = getActiveColumn('Timeline');
  expect(await within(timeline).findByText('Failed to load.', {}, FAILURE_WAIT)).toBeInTheDocument();
  expect(within(timeline).getByRole('button', { name: 'Retry loading' })).toBeInTheDocument();
});

test('timeline image stops loading and shows the fetch failure after rejected responses in normal mode', async () => {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
  const payload = createDeferred<null>();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildImagePost({ content: 'available caption', content_status: 'Available' }),
      ],
    },
  });
  api.getBlobMediaPayload = async () => payload.promise;

  render(<App api={api} />);

  expect(await within(getActiveColumn('Timeline')).findByTestId('media-skeleton-image-post')).toBeInTheDocument();
  act(() => payload.reject(new Error('blob unavailable')));

  await waitFor(() => {
    expect(screen.queryByTestId('media-skeleton-image-post')).not.toBeInTheDocument();
  });
  expect(screen.queryByText('image/png')).not.toBeInTheDocument();
  expect(screen.queryByText('blob unavailable')).not.toBeInTheDocument();
  expect(
    await within(getActiveColumn('Timeline')).findByText('Failed to load.', {}, FAILURE_WAIT)
  ).toBeInTheDocument();
});

test('missing text body does not occupy normal UI', async () => {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [buildImagePost({ attachments: [] })],
        },
      })}
    />
  );

  await waitFor(() => {
    expect(document.querySelector('[data-post-object-id="image-post"]')).toBeInTheDocument();
  });
  expect(screen.queryByText('envelope-image-post')).not.toBeInTheDocument();
  expect(screen.queryByTestId('text-skeleton-image-post')).not.toBeInTheDocument();
  expect(screen.queryByText('[blob pending]')).not.toBeInTheDocument();
  expect(screen.queryByText('Content unavailable.')).not.toBeInTheDocument();
});

test('developer mode shows concise diagnostics after body and media become unavailable', async () => {
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [buildImagePost()],
    },
  });
  api.getBlobMediaPayload = async () => null;

  render(<App api={api} />);

  expect(await within(getActiveColumn('Timeline')).findByText('Content unavailable.')).toBeInTheDocument();
  expect(await within(getActiveColumn('Timeline')).findByText('Failed to load.', {}, FAILURE_WAIT)).toBeInTheDocument();
  expect(screen.queryByTestId('text-skeleton-image-post')).not.toBeInTheDocument();
  expect(screen.queryByTestId('media-skeleton-image-post')).not.toBeInTheDocument();
  expect(screen.queryByText('[blob pending]')).not.toBeInTheDocument();
});

test('developer mode replaces a missing image skeleton with a diagnostic', async () => {
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [buildImagePost()],
    },
  });
  api.getBlobMediaPayload = async () => null;

  render(<App api={api} />);

  expect(await within(getActiveColumn('Timeline')).findByText('Failed to load.', {}, FAILURE_WAIT)).toBeInTheDocument();
  expect(screen.queryByTestId('media-skeleton-image-post')).not.toBeInTheDocument();
  expect(screen.queryByText('image/png')).not.toBeInTheDocument();
});

test('timeline image post switches to ready state when attachment becomes available', async () => {
  const missingPost = buildImagePost();
  const { rerender } = render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [missingPost],
        },
      })}
    />
  );

  await waitFor(() => {
    expect(within(getActiveColumn('Timeline')).getByTestId('media-skeleton-image-post')).toBeInTheDocument();
  });

  rerender(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [
            buildImagePost({
              content: 'caption ready',
              content_status: 'Available' satisfies BlobViewStatus,
              attachments: [
                {
                  ...missingPost.attachments[0],
                  status: 'Available',
                },
              ],
            }),
          ],
        },
      })}
    />
  );

  await waitFor(() => {
    expect(screen.getByText('caption ready')).toBeInTheDocument();
  });
  expect(screen.queryByTestId('media-skeleton-image-post')).not.toBeInTheDocument();
});

test('timeline image recovers when an existing refresh retries a previously unavailable hash', async () => {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'false');
  installObjectUrlMocks();
  const missingPost = buildImagePost({ content: 'caption', content_status: 'Available' });
  const unavailableApi = createDesktopMockApi({
    seedPosts: { 'kukuri:topic:general': [missingPost] },
  });
  unavailableApi.getBlobMediaPayload = async () => null;
  const recoveredApi = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildImagePost({
          content: 'caption',
          content_status: 'Available',
          attachments: [{ ...missingPost.attachments[0], status: 'Available' }],
        }),
      ],
    },
  });
  recoveredApi.getBlobMediaPayload = async (_hash, mime) => ({
    bytes_base64: 'ZmFrZS1pbWFnZQ==',
    mime,
  });

  const { rerender } = render(<App api={unavailableApi} />);
  await waitFor(() => {
    expect(screen.queryByTestId('media-skeleton-image-post')).not.toBeInTheDocument();
  });

  rerender(<App api={recoveredApi} />);

  expect(await within(getActiveColumn('Timeline')).findByTestId('media-preview-image-post')).toHaveAttribute(
    'src',
    expect.stringContaining('blob:mock-')
  );
});

test('timeline image post renders actual preview when object-url payload is available', async () => {
  const { revokeObjectUrl } = installObjectUrlMocks();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildImagePost({
          content: 'caption ready',
          content_status: 'Available',
          attachments: [
            {
              hash: 'b'.repeat(64),
              mime: 'image/png',
              bytes: 4096,
              role: 'image_original',
              status: 'Available',
            },
          ],
        }),
      ],
    },
  });
  api.getBlobMediaPayload = async () => ({
    bytes_base64: 'ZmFrZS1pbWFnZQ==',
    mime: 'image/png',
  });

  const { unmount } = render(<App api={api} />);

  const preview = await within(getActiveColumn('Timeline')).findByTestId('media-preview-image-post');
  expect(preview).toBeInTheDocument();
  const previewUrl = preview.getAttribute('src');
  expect(previewUrl).toMatch(/^blob:mock-\d+$/);
  expect(revokeObjectUrl).not.toHaveBeenCalledWith(previewUrl);

  unmount();
  expect(revokeObjectUrl).toHaveBeenCalledWith(previewUrl);
});

test('thread pane reuses the same unavailable media renderer', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildImagePost(),
        {
          ...buildImagePost({
            object_id: 'reply-post',
            envelope_id: 'envelope-reply-post',
            object_kind: 'comment',
            content: 'reply body',
            content_status: 'Available',
            attachments: [],
            reply_to: 'image-post',
            root_id: 'image-post',
          }),
        },
      ],
    },
  });
  api.getBlobMediaPayload = async () => null;

  render(
    <App api={api} />
  );

  let imagePost: HTMLElement | null = null;
  await waitFor(() => {
    imagePost = document.querySelector(
      '[data-post-object-id="image-post"] [data-testid="post-identifier-target"]'
    );
    expect(imagePost).toBeInTheDocument();
  });
  expect(screen.queryByText('envelope-image-post')).not.toBeInTheDocument();
  await user.click(imagePost!);
  await waitFor(() => expect(getDetailPane('Thread')).toBeInTheDocument());
  const threadPanel = getDetailPane('Thread');

  expect(await within(threadPanel).findByText('Failed to load.', {}, FAILURE_WAIT)).toBeInTheDocument();
  expect(within(threadPanel).queryByTestId('media-skeleton-image-post')).not.toBeInTheDocument();
});

test('developer mode reports an unavailable text body without rendering its placeholder', async () => {
  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [buildImagePost({ attachments: [] })],
        },
      })}
    />
  );

  expect(await within(getActiveColumn('Timeline')).findByText('Content unavailable.')).toBeInTheDocument();
  expect(screen.queryByTestId('text-skeleton-image-post')).not.toBeInTheDocument();
  expect(screen.queryByText('[blob pending]')).not.toBeInTheDocument();
});

test('developer mode replaces an unavailable video skeleton with a diagnostic', async () => {
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [buildVideoPost()],
    },
  });
  api.getBlobMediaPayload = async () => null;

  render(<App api={api} />);

  expect(await within(getActiveColumn('Timeline')).findByText('Failed to load.', {}, FAILURE_WAIT)).toBeInTheDocument();
  expect(screen.queryByTestId('media-skeleton-video-post')).not.toBeInTheDocument();
  expect(screen.queryByText('video/mp4')).not.toBeInTheDocument();
});

test('poster-only video card renders poster preview without video element', async () => {
  installObjectUrlMocks();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildVideoPost({
          attachments: [
            {
              hash: 'v'.repeat(64),
              mime: 'video/mp4',
              bytes: 8192,
              role: 'video_manifest',
              status: 'Missing',
            },
            {
              hash: 'p'.repeat(64),
              mime: 'image/jpeg',
              bytes: 1024,
              role: 'video_poster',
              status: 'Available',
            },
          ],
        }),
      ],
    },
  });
  api.getBlobMediaPayload = async (hash, mime) =>
    hash === 'p'.repeat(64)
      ? {
          bytes_base64: 'ZmFrZS1wb3N0ZXI=',
          mime,
        }
      : null;

  render(<App api={api} />);

  const posterPreview = await screen.findByTestId('media-preview-video-post');
  expect(posterPreview).toBeInTheDocument();
  expect(screen.queryByTestId('media-video-video-post')).not.toBeInTheDocument();
  expect(posterPreview.getAttribute('src')).toContain('blob:mock-');
});

test('video card fetches manifest payload even when attachment status is missing', async () => {
  installObjectUrlMocks();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildVideoPost({
          attachments: [
            {
              hash: 'late-manifest'.repeat(4),
              mime: 'video/mp4',
              bytes: 9999,
              role: 'video_manifest',
              status: 'Missing',
            },
            {
              hash: 'late-poster'.repeat(4),
              mime: 'image/jpeg',
              bytes: 1024,
              role: 'video_poster',
              status: 'Available',
            },
          ],
        }),
      ],
    },
  });
  api.getBlobMediaPayload = async (hash, mime) => {
    if (hash === 'late-manifest'.repeat(4)) {
      return {
        bytes_base64: 'ZmFrZS12aWRlbw==',
        mime,
      };
    }
    if (hash === 'late-poster'.repeat(4)) {
      return {
        bytes_base64: 'ZmFrZS1wb3N0ZXI=',
        mime,
      };
    }
    return null;
  };

  render(<App api={api} />);

  const video = await screen.findByTestId('media-video-video-post');
  expect(video).toBeInTheDocument();
  expect(video).toHaveAttribute('src', expect.stringContaining('blob:mock-'));
});

test('video card retries after stalled manifest fetch after rerender', async () => {
  installObjectUrlMocks();
  const manifestHash = 'retry-manifest'.repeat(4);
  const posterHash = 'retry-poster'.repeat(4);
  const seedPosts = {
    'kukuri:topic:general': [
      buildVideoPost({
        attachments: [
          {
            hash: manifestHash,
            mime: 'video/mp4',
            bytes: 9999,
            role: 'video_manifest',
            status: 'Missing',
          },
          {
            hash: posterHash,
            mime: 'image/jpeg',
            bytes: 1024,
            role: 'video_poster',
            status: 'Missing',
          },
        ],
      }),
    ],
  };
  const stalledApi = createDesktopMockApi({
    seedPosts,
  });
  stalledApi.getBlobMediaPayload = async (hash, mime) => {
    if (hash === manifestHash) {
      return new Promise<null>(() => {});
    }
    if (hash === posterHash) {
      return {
        bytes_base64: 'ZmFrZS1wb3N0ZXI=',
        mime,
      };
    }
    return null;
  };
  const recoveredApi = createDesktopMockApi({
    seedPosts: {
      ...seedPosts,
    },
  });
  recoveredApi.getBlobMediaPayload = async (hash, mime) => {
    if (hash === manifestHash) {
      return {
        bytes_base64: 'ZmFrZS12aWRlbw==',
        mime,
      };
    }
    if (hash === posterHash) {
      return {
        bytes_base64: 'ZmFrZS1wb3N0ZXI=',
        mime,
      };
    }
    return null;
  };

  const { rerender } = render(<App api={stalledApi} />);

  await waitFor(() => {
    expect(screen.getByTestId('media-preview-video-post')).toBeInTheDocument();
  });

  rerender(<App api={recoveredApi} />);

  const video = await screen.findByTestId('media-video-video-post');
  expect(video).toBeInTheDocument();
  expect(video).toHaveAttribute('src', expect.stringContaining('blob:mock-'));
});

test('video card renders object-url playback source when manifest payload is available', async () => {
  installObjectUrlMocks();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildVideoPost({
          attachments: [
            {
              hash: 'manifest'.repeat(8),
              mime: 'video/mp4',
              bytes: 9999,
              role: 'video_manifest',
              status: 'Available',
            },
            {
              hash: 'poster'.repeat(8),
              mime: 'image/jpeg',
              bytes: 1024,
              role: 'video_poster',
              status: 'Available',
            },
          ],
        }),
      ],
    },
  });
  api.getBlobMediaPayload = async (hash, mime) => {
    if (hash === 'manifest'.repeat(8)) {
      return {
        bytes_base64: 'ZmFrZS12aWRlbw==',
        mime,
      };
    }
    if (hash === 'poster'.repeat(8)) {
      return {
        bytes_base64: 'ZmFrZS1wb3N0ZXI=',
        mime,
      };
    }
    return null;
  };

  render(<App api={api} />);

  const video = await screen.findByTestId('media-video-video-post');
  expect(video).toBeInTheDocument();
  expect(video.getAttribute('src')).toContain('blob:mock-');
});

test('video card falls back to poster preview when playback is unsupported on this client', async () => {
  installObjectUrlMocks();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildVideoPost({
          attachments: [
            {
              hash: 'manifest'.repeat(8),
              mime: 'video/mp4',
              bytes: 9999,
              role: 'video_manifest',
              status: 'Available',
            },
            {
              hash: 'poster'.repeat(8),
              mime: 'image/jpeg',
              bytes: 1024,
              role: 'video_poster',
              status: 'Available',
            },
          ],
        }),
      ],
    },
  });
  api.getBlobMediaPayload = async (hash, mime) => {
    if (hash === 'manifest'.repeat(8)) {
      return {
        bytes_base64: 'ZmFrZS12aWRlbw==',
        mime,
      };
    }
    if (hash === 'poster'.repeat(8)) {
      return {
        bytes_base64: 'ZmFrZS1wb3N0ZXI=',
        mime,
      };
    }
    return null;
  };

  render(<App api={api} />);

  const video = await screen.findByTestId('media-video-video-post');
  Object.defineProperty(video, 'error', {
    configurable: true,
    get: () => ({ code: 4 }),
  });
  fireEvent.error(video);

  await waitFor(() => {
    expect(screen.queryByTestId('media-video-video-post')).not.toBeInTheDocument();
  });
  expect(screen.getByTestId('media-preview-video-post')).toBeInTheDocument();
  expect(screen.getByAltText('video preview')).toBeInTheDocument();
});


// #1207 TR-1 / TR-9: 自動取得は上限回数で止まり、その後の定期 refresh では取り直さない。
test('automatic media fetch stops at the attempt limit and later refreshes do not retry', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const post = buildImagePost({ content: 'caption', content_status: 'Available' });
  const imageHash = post.attachments[0].hash;
  const api = createDesktopMockApi({ seedPosts: { 'kukuri:topic:general': [post] } });
  const requested: string[] = [];
  api.getBlobMediaPayload = async (hash) => {
    requested.push(hash);
    return null;
  };

  render(<App api={api} />);

  const timeline = getActiveColumn('Timeline');
  expect(await within(timeline).findByText('Failed to load.', {}, FAILURE_WAIT)).toBeInTheDocument();
  const attempts = () => requested.filter((hash) => hash === imageHash).length;
  expect(attempts()).toBe(MEDIA_FETCH_MAX_AUTO_ATTEMPTS);

  // 3 秒 refresh を複数回またぐ。timeline の参照は毎回変わるが、失敗した hash は取り直さない。
  await act(async () => {
    await vi.advanceTimersByTimeAsync(15_000);
  });
  expect(attempts()).toBe(MEDIA_FETCH_MAX_AUTO_ATTEMPTS);
  expect(within(timeline).getByText('Failed to load.')).toBeInTheDocument();
});

// #1207 TR-2: 明示再試行は対象 hash だけを 1 回取り直し、再試行中は操作を重複させない。
test('manual retry refetches only the failed hash and recovers when the blob arrives', async () => {
  installObjectUrlMocks();
  const user = userEvent.setup();
  const post = buildImagePost({ content: 'caption', content_status: 'Available' });
  const imageHash = post.attachments[0].hash;
  const api = createDesktopMockApi({ seedPosts: { 'kukuri:topic:general': [post] } });
  const requested: string[] = [];
  let retryPayload = createDeferred<{ bytes_base64: string; mime: string } | null>();
  let failing = true;
  api.getBlobMediaPayload = async (hash, mime) => {
    requested.push(hash);
    if (failing) {
      return null;
    }
    void mime;
    return retryPayload.promise;
  };

  render(<App api={api} />);

  const timeline = getActiveColumn('Timeline');
  const retryButton = await within(timeline).findByRole('button', { name: 'Retry loading' }, FAILURE_WAIT);
  const before = requested.length;
  failing = false;

  await user.click(retryButton);
  await waitFor(() => expect(retryButton).toHaveAttribute('aria-disabled', 'true'));
  expect(within(timeline).getByText('Retrying…')).toBeInTheDocument();
  // 再試行中の追加 click は新しい取得を起こさない。
  await user.click(retryButton);
  expect(requested.slice(before)).toEqual([imageHash]);

  act(() => retryPayload.resolve({ bytes_base64: 'ZmFrZS1pbWFnZQ==', mime: 'image/png' }));
  expect(await within(timeline).findByTestId('media-preview-image-post')).toHaveAttribute(
    'src',
    expect.stringContaining('blob:mock-')
  );
  expect(within(timeline).queryByText('Failed to load.')).not.toBeInTheDocument();

  retryPayload = createDeferred();
});

// #1207 TR-2: 明示再試行が失敗したら失敗表示へ戻り、自動では取り直さない。
test('manual retry that fails returns to the failure display without looping', async () => {
  const user = userEvent.setup();
  const post = buildImagePost({ content: 'caption', content_status: 'Available' });
  const api = createDesktopMockApi({ seedPosts: { 'kukuri:topic:general': [post] } });
  let calls = 0;
  api.getBlobMediaPayload = async () => {
    calls += 1;
    return null;
  };

  render(<App api={api} />);

  const timeline = getActiveColumn('Timeline');
  const retryButton = await within(timeline).findByRole('button', { name: 'Retry loading' }, FAILURE_WAIT);
  const before = calls;
  await user.click(retryButton);

  await waitFor(() => expect(calls).toBe(before + 1));
  await waitFor(() =>
    expect(within(timeline).getByRole('button', { name: 'Retry loading' })).toHaveAttribute(
      'aria-disabled',
      'false'
    )
  );
  expect(within(timeline).getByText('Failed to load.')).toBeInTheDocument();
  expect(calls).toBe(before + 1);
});

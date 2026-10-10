import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, vi } from 'vitest';

import { App } from '@/App';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type { AttachmentView, NotificationView, PostView, TimelineCursor, TimelineView } from '@/lib/api';
import { topicDisplayName } from '@/lib/topicId';

export function setViewportWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    writable: true,
    value: width,
  });
  window.dispatchEvent(new Event('resize'));
}

export function createDeferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return {
    promise,
    reject,
    resolve,
  };
}

export function buildImagePost(overrides?: Partial<PostView>): PostView {
  const attachment: AttachmentView = {
    hash: 'a'.repeat(64),
    mime: 'image/png',
    bytes: 2048,
    role: 'image_original',
    status: 'Missing',
  };

  return {
    object_id: 'image-post',
    envelope_id: 'envelope-image-post',
    author_pubkey: 'f'.repeat(64),
    author_name: null,
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: '[blob pending]',
    content_status: 'Missing',
    attachments: [attachment],
    created_at: 1,
    reply_to: null,
    root_id: 'image-post',
    channel_id: null,
    audience_label: 'Public',
    ...overrides,
  };
}

export function buildVideoPost(overrides?: Partial<PostView>): PostView {
  return {
    object_id: 'video-post',
    envelope_id: 'envelope-video-post',
    author_pubkey: 'e'.repeat(64),
    author_name: null,
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: 'video caption',
    content_status: 'Available',
    attachments: [
      {
        hash: 'v'.repeat(64),
        mime: 'video/mp4',
        bytes: 8192,
        role: 'video_manifest',
        status: 'Available',
      },
      {
        hash: 'p'.repeat(64),
        mime: 'image/jpeg',
        bytes: 1024,
        role: 'video_poster',
        status: 'Missing',
      },
    ],
    created_at: 2,
    reply_to: null,
    root_id: 'video-post',
    channel_id: null,
    audience_label: 'Public',
    ...overrides,
  };
}

export function buildNotification(overrides?: Partial<NotificationView>): NotificationView {
  return {
    notification_id: 'notification-1',
    kind: 'reply',
    actor_pubkey: 'c'.repeat(64),
    actor_name: 'carol',
    actor_display_name: null,
    actor_picture_asset: null,
    source_envelope_id: 'notification-envelope-1',
    source_replica_id: 'replica:notification',
    topic_id: 'kukuri:topic:general',
    channel_id: null,
    object_id: 'reply-1',
    thread_root_object_id: 'post-thread-open',
    dm_id: null,
    message_id: null,
    preview_text: 'notification preview',
    content_labels: [],
    created_at: 1,
    received_at: 1,
    read_at: null,
    ...overrides,
  };
}

export function buildPaginatedPost(index: number, overrides?: Partial<PostView>): PostView {
  return {
    object_id: `paginated-post-${index}`,
    envelope_id: `paginated-envelope-${index}`,
    author_pubkey: 'a'.repeat(64),
    author_name: 'alice',
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: index === 1 ? 'post' : 'comment',
    is_threadable: true,
    content: `paginated post ${index}`,
    content_status: 'Available',
    attachments: [],
    created_at: index,
    reply_to: index === 1 ? null : 'paginated-post-1',
    root_id: 'paginated-post-1',
    channel_id: null,
    audience_label: 'Public',
    ...overrides,
  };
}

export function paginatePosts(posts: PostView[], cursor: TimelineCursor | null, limit: number): TimelineView {
  const startIndex = cursor
    ? posts.findIndex(
        (post) =>
          post.object_id === cursor.object_id && post.created_at === cursor.created_at
      ) + 1
    : 0;
  const normalizedStartIndex = startIndex > 0 ? startIndex : 0;
  const items = posts.slice(normalizedStartIndex, normalizedStartIndex + limit);
  return {
    items,
    next_cursor:
      normalizedStartIndex + limit < posts.length
        ? {
            created_at: items[items.length - 1]!.created_at,
            object_id: items[items.length - 1]!.object_id,
          }
        : null,
  };
}

export function installObjectUrlMocks() {
  let sequence = 0;
  const createObjectUrl = vi
    .spyOn(URL, 'createObjectURL')
    .mockImplementation(() => `blob:mock-${++sequence}`);
  const revokeObjectUrl = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});

  return { createObjectUrl, revokeObjectUrl };
}

export function installSuccessfulPosterGenerationMocks() {
  Object.defineProperty(HTMLVideoElement.prototype, 'videoWidth', {
    configurable: true,
    get: () => 640,
  });
  Object.defineProperty(HTMLVideoElement.prototype, 'videoHeight', {
    configurable: true,
    get: () => 360,
  });
  Object.defineProperty(HTMLMediaElement.prototype, 'readyState', {
    configurable: true,
    get: () => 2,
  });
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(function load(
    this: HTMLMediaElement
  ) {
    queueMicrotask(() => {
      this.dispatchEvent(new Event('loadeddata'));
    });
  });
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => {});
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
    drawImage: vi.fn(),
  } as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, 'toDataURL').mockReturnValue('data:image/jpeg;base64,CQgHBg==');
}

export function installMetadataSeekPosterGenerationMocks() {
  Object.defineProperty(HTMLVideoElement.prototype, 'videoWidth', {
    configurable: true,
    get: () => 640,
  });
  Object.defineProperty(HTMLVideoElement.prototype, 'videoHeight', {
    configurable: true,
    get: () => 360,
  });
  Object.defineProperty(HTMLMediaElement.prototype, 'duration', {
    configurable: true,
    get: () => 12,
  });
  Object.defineProperty(HTMLMediaElement.prototype, 'readyState', {
    configurable: true,
    get: () => 2,
  });
  let currentTime = 0;
  Object.defineProperty(HTMLMediaElement.prototype, 'currentTime', {
    configurable: true,
    get: () => currentTime,
    set(this: HTMLMediaElement, value: number) {
      currentTime = value;
      queueMicrotask(() => {
        this.dispatchEvent(new Event('seeked'));
      });
    },
  });
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(function load(
    this: HTMLMediaElement
  ) {
    queueMicrotask(() => {
      this.dispatchEvent(new Event('loadedmetadata'));
    });
  });
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockImplementation(async () => undefined);
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => {});
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
    drawImage: vi.fn(),
  } as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, 'toDataURL').mockReturnValue('data:image/jpeg;base64,CQgHBg==');
}

export function installFailedPosterGenerationMocks() {
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(function load(
    this: HTMLMediaElement
  ) {
    queueMicrotask(() => {
      this.dispatchEvent(new Event('error'));
    });
  });
}

export async function openSettingsDrawer(user: ReturnType<typeof userEvent.setup>) {
  await waitFor(() => {
    expect(window.location.hash).toMatch(/^#\/(?:timeline|channels|live|game|profile)/);
  });
  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Settings' }));
  return await screen.findByRole('dialog', { name: 'Settings' });
}

export async function openControlCenter(user: ReturnType<typeof userEvent.setup>) {
  const existing = screen.queryByRole('complementary', { name: 'Control Center' });
  if (existing) return existing;
  await user.click(await screen.findByTestId('control-center-trigger'));
  return screen.getByRole('complementary', { name: 'Control Center' });
}

export async function openSettingsSection(
  user: ReturnType<typeof userEvent.setup>,
  section:
    | 'appearance'
    | 'keyboard'
    | 'safety'
    | 'backup'
    | 'account'
    | 'connectivity'
    | 'discovery'
    | 'community-node'
    | 'reactions'
    | 'developer'
) {
  const drawer = await openSettingsDrawer(user);
  await user.click(within(drawer).getByTestId(`settings-section-${section}`));
  return drawer;
}

export function closestSection(element: HTMLElement) {
  const section = element.closest('section');
  if (!(section instanceof HTMLElement)) {
    throw new Error('expected enclosing section');
  }
  return section;
}

export function renderAtHash(hash: string, api = createDesktopMockApi()) {
  window.history.replaceState(null, '', `/${hash}`);
  return render(<App api={api} />);
}

export function expectActiveTopic(topic: string) {
  expect(window.location.hash).toContain(`topic=${encodeURIComponent(topic)}`);
  expect(getActiveColumn('Timeline')).toHaveTextContent(topicDisplayName(topic));
}

export function getWorkspaceTabs() {
  return screen.getByRole('tablist', { name: 'Workspaces' });
}

export function getTimelineViewTabs() {
  return screen.getByRole('tablist', { name: 'Timeline views' });
}

export function getSocialConnectionsTabs() {
  return screen.getByRole('tablist', { name: 'Social connections' });
}

export async function selectWorkspace(
  user: ReturnType<typeof userEvent.setup>,
  label: 'Timeline' | 'Live' | 'Metaverse' | 'Messages' | 'Profile'
) {
  const titleByLabel = {
    Timeline: 'Timeline',
    Live: 'Live',
    Metaverse: 'Metaverse',
    Messages: 'Messages',
    Profile: 'Profile',
  } as const;
  const controlCenter = await openControlCenter(user);
  await user.click(
    within(controlCenter).getByRole('button', {
      name: `Add ${titleByLabel[label]} Column`,
    })
  );
  await waitFor(() => {
    expect(screen.getByRole('region', { name: new RegExp(`${titleByLabel[label]} Column.*Active`) }))
      .toHaveAttribute('aria-current', 'true');
  });
}

export async function selectTimelineView(
  user: ReturnType<typeof userEvent.setup>,
  label: 'Feed' | 'Bookmarks'
) {
  await user.click(within(getTimelineViewTabs()).getByRole('tab', { name: label }));
  await waitFor(() => {
    expect(within(getTimelineViewTabs()).getByRole('tab', { name: label })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
}

export function getPrimaryNavigation() {
  return screen.getByLabelText('Primary navigation');
}

export async function openPublishDialog(user: ReturnType<typeof userEvent.setup>) {
  await user.click(within(getActiveColumn('Timeline')).getByRole('button', { name: /^Post to / }));
  return getActiveColumn('Timeline');
}

export async function publishPost(
  user: ReturnType<typeof userEvent.setup>,
  content: string,
  // 'paste' は本文を 1 回の入力で渡す。打鍵ごとに App 全体が再描画されるため、
  // 入力そのものを検証しない長い scenario で負荷時の所要時間を抑える(#1165)。
  { input = 'type' }: { input?: 'type' | 'paste' } = {}
) {
  const dialog = await openPublishDialog(user);
  const composer = within(dialog).getByPlaceholderText('Write a post');
  if (input === 'paste') {
    await user.click(composer);
    await user.paste(content);
  } else {
    await user.type(composer, content);
  }
  await user.click(within(dialog).getByRole('button', { name: 'Post' }));
  await waitFor(() => {
    expect(within(dialog).queryByPlaceholderText('Write a post')).not.toBeInTheDocument();
  });
}

export async function openChannelManager(user: ReturnType<typeof userEvent.setup>) {
  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Create or join a private channel' }));
  return await screen.findByRole('dialog', { name: 'Create / Join Private Channel' });
}

export function getChannelShareButton(
  dialog: HTMLElement,
  channelLabel?: string,
  audienceLabel?: string
) {
  void channelLabel;
  void audienceLabel;
  return within(dialog).getByRole('button', {
    name: 'Create share link',
  });
}

export async function openChannelSettings(user: ReturnType<typeof userEvent.setup>, channelLabel: string) {
  const controlCenter = await openControlCenter(user);
  await user.click(
    within(controlCenter).getByRole('button', { name: `Open ${channelLabel} channel settings` })
  );
  return await screen.findByRole('dialog', { name: 'Channel Settings' });
}

export async function openLiveCreateDialog(user: ReturnType<typeof userEvent.setup>) {
  await selectWorkspace(user, 'Live');
  await user.click(within(getActiveColumn('Live')).getByRole('button', { name: 'Start Live' }));
  return await screen.findByRole('dialog', { name: 'Start Live' });
}

export async function openNotificationsInbox(user: ReturnType<typeof userEvent.setup>) {
  const controlCenter = await openControlCenter(user);
  await user.click(
    within(controlCenter).getByRole('button', { name: /^(?:Notifications|通知) (?:\d+|99\+)$/ })
  );
  return (await screen.findAllByRole('heading', { name: 'Notifications' }))[0];
}

export function getDetailPane(name: 'Thread' | 'Author') {
  const columnTitle = name === 'Author' ? 'Profile' : 'Thread';
  const columns = screen.queryAllByRole('region', {
    name: new RegExp(`^${columnTitle} Column,`),
  });
  const activeColumn = columns.find(
    (column) => column.getAttribute('aria-current') === 'true',
  );

  return activeColumn ?? columns.at(-1) ?? screen.getByRole('complementary', { name });
}

export function getActiveColumn(title: string) {
  const columns = screen.getAllByRole('region', {
    name: new RegExp(`^${title} Column,`),
  });
  const active = columns.find((column) => column.getAttribute('aria-current') === 'true');
  if (!active) throw new Error(`expected active ${title} Column`);
  return active;
}

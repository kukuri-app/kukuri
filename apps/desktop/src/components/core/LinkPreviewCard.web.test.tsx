import { render, screen, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { afterEach, expect, test, vi } from 'vitest';

import { LinkPreviewCard } from './LinkPreviewCard';
import { PostCard } from './PostCard';
import { createView } from './PostCard.testHelpers';

// AC-2d: Web は外部 URL を取得せず、投稿者が書いた record を読む。native は従来の取得のまま、投稿の ID を渡す。
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const url = 'https://example.test/web-record?q=1';

afterEach(() => {
  invokeMock.mockReset();
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

const record = {
  url,
  source_label: 'Example',
  title: 'Author record',
  description: 'Written by the author',
  image_data_url: 'data:image/png;base64,iVBORw0KGgo=',
};

test('Web renders the card from the author record of the post', async () => {
  invokeMock.mockResolvedValue(record);
  render(<LinkPreviewCard content={`見て ${url}`} objectId='web-card' enabled />);

  const card = await screen.findByRole('link', { name: 'Author record — Example' });
  expect(card).toHaveAttribute('href', url);
  expect(card.querySelector('img')).toHaveAttribute('src', record.image_data_url);
  expect(invokeMock).toHaveBeenCalledExactlyOnceWith('read_link_preview_record', {
    objectId: 'web-card',
    url,
  });
});

test('Web reuses the read record while it is fresh', async () => {
  invokeMock.mockResolvedValue(record);
  const first = render(<LinkPreviewCard content={url} objectId='web-cache' enabled />);
  await screen.findByRole('link', { name: 'Author record — Example' });
  first.unmount();
  render(<LinkPreviewCard content={url} objectId='web-cache' enabled />);
  await screen.findByRole('link', { name: 'Author record — Example' });
  expect(invokeMock).toHaveBeenCalledOnce();
});

test.each([
  ['missing', async () => null],
  ['rejected', async () => Promise.reject(new Error('invalid record'))],
])('Web shows only the URL when the record is %s', async (label, result) => {
  invokeMock.mockImplementation(result);
  const base = createView();
  const { container } = render(
    <PostCard
      enableLinkPreview
      view={createView({
        post: { ...base.post, object_id: `web-${label}`, content: `見て ${url}` },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  await waitFor(() =>
    expect(container.querySelector('.link-preview-slot')).toHaveAttribute(
      'data-state',
      'unavailable'
    )
  );
  expect(container.querySelector('.link-preview-card')).toBeNull();
  expect(screen.getByRole('link', { name: url })).toHaveAttribute('href', url);
});

test('native keeps fetching the URL and passes the post for the author record', async () => {
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  invokeMock.mockResolvedValue({ status: 'available', preview: record });
  render(<LinkPreviewCard content={url} objectId='native-card' enabled />);

  await screen.findByRole('link', { name: 'Author record — Example' });
  expect(invokeMock).toHaveBeenCalledExactlyOnceWith('fetch_link_preview', {
    url,
    objectId: 'native-card',
  });
});

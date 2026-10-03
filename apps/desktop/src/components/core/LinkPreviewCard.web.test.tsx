import { render, screen, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { afterEach, expect, test, vi } from 'vitest';

import { readLinkPreviewRecord } from '@/lib/api';

import { LinkPreviewCard } from './LinkPreviewCard';
import { PostCard } from './PostCard';
import { createView } from './PostCard.testHelpers';

// AC-2d: Web は外部 URL を取得せず、投稿者が書いた record を読む。native は従来の取得のまま、投稿の ID を渡す。
// AC-2f: native も表示したときに record を読み(中継のための保持)、読取りの結果は native の取得と同じ上限で持つ。
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

test('native keeps showing its own fetch and also reads the author record to relay it', async () => {
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  invokeMock.mockImplementation(async (command) =>
    command === 'fetch_link_preview' ? { status: 'available', preview: record } : null
  );
  render(<LinkPreviewCard content={url} objectId='native-card' enabled />);

  await screen.findByRole('link', { name: 'Author record — Example' });
  expect(invokeMock).toHaveBeenCalledTimes(2);
  expect(invokeMock).toHaveBeenCalledWith('fetch_link_preview', { url, objectId: 'native-card' });
  expect(invokeMock).toHaveBeenCalledWith('read_link_preview_record', {
    objectId: 'native-card',
    url,
  });
});

test('a refused or failed read is not kept and is read again on the next display', async () => {
  invokeMock.mockRejectedValueOnce(new Error('link preview reads are busy'));
  invokeMock.mockResolvedValueOnce(record);

  expect((await readLinkPreviewRecord(url, 'web-retry')).status).toBe('unavailable');
  expect((await readLinkPreviewRecord(url, 'web-retry')).status).toBe('available');
  expect(invokeMock).toHaveBeenCalledTimes(2);
});

test('the kept results are limited to 16 MiB and the oldest is dropped first', async () => {
  invokeMock.mockImplementation(async () => ({
    ...record,
    image_data_url: `data:image/png;base64,${'A'.repeat(6 * 1024 * 1024)}`,
  }));
  for (const objectId of ['big-1', 'big-2', 'big-3']) {
    await readLinkPreviewRecord(url, objectId);
  }
  invokeMock.mockClear();

  await readLinkPreviewRecord(url, 'big-3');
  expect(invokeMock).not.toHaveBeenCalled();
  await readLinkPreviewRecord(url, 'big-1');
  expect(invokeMock).toHaveBeenCalledOnce();
});

test('the kept results are limited to 128 entries and the oldest is dropped first', async () => {
  invokeMock.mockResolvedValue(null);
  for (let index = 0; index <= 128; index += 1) {
    await readLinkPreviewRecord(url, `count-${index}`);
  }
  invokeMock.mockClear();

  await readLinkPreviewRecord(url, 'count-128');
  expect(invokeMock).not.toHaveBeenCalled();
  await readLinkPreviewRecord(url, 'count-0');
  expect(invokeMock).toHaveBeenCalledOnce();
});

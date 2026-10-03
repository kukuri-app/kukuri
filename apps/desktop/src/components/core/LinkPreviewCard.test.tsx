import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import type { LinkPreviewOutcome } from '@/lib/api';

import { LinkPreviewCard } from './LinkPreviewCard';

afterEach(() => {
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

const available = async (url: string): Promise<LinkPreviewOutcome> => ({
  status: 'available',
  preview: {
    url,
    source_label: 'Example Site',
    title: 'A bounded link preview',
    description: 'Metadata returned by the guarded desktop command.',
    image_data_url: 'data:image/png;base64,iVBORw0KGgo=',
  },
});

test('renders a successful preview as a link to the original URL', async () => {
  const fetcher = vi.fn(available);
  const url = 'https://example.test/article?q=preview#section';
  render(<LinkPreviewCard objectId='post-1' content={`Read ${url}`} enabled fetcher={fetcher} />);

  const card = await screen.findByRole('link', {
    name: 'A bounded link preview — Example Site',
  });
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher).toHaveBeenCalledWith(url, 'post-1');
  expect(card).toHaveAttribute('href', url);
  expect(card.querySelector('img')).toHaveAttribute(
    'src',
    'data:image/png;base64,iVBORw0KGgo='
  );
  expect(card.querySelector('img')).toHaveAttribute('alt', '');
});

test('keeps the slot empty when metadata is unavailable', async () => {
  const fetcher = vi.fn(async (): Promise<LinkPreviewOutcome> => ({
    status: 'unavailable',
    reason: 'timeout',
  }));
  const { container } = render(
    <LinkPreviewCard objectId='post-1' content='https://example.test/unavailable' enabled fetcher={fetcher} />
  );

  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  await waitFor(() =>
    expect(container.querySelector('.link-preview-slot')).toHaveAttribute(
      'data-state',
      'unavailable'
    )
  );
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
});

test('does not render an unapproved data image MIME returned by the bridge', async () => {
  const fetcher = vi.fn(async (url: string): Promise<LinkPreviewOutcome> => ({
    status: 'available',
    preview: {
      url,
      source_label: 'example.test',
      title: 'Text preview remains available',
      description: null,
      image_data_url: 'data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=',
    },
  }));
  render(
    <LinkPreviewCard objectId='post-1' content='https://example.test/svg' enabled fetcher={fetcher} />
  );

  const card = await screen.findByRole('link', {
    name: 'Text preview remains available — example.test',
  });
  expect(card.querySelector('img')).toBeNull();
});

test('does not request an offscreen preview until it intersects', async () => {
  let notify: IntersectionObserverCallback = () => undefined;
  let observed: Element | null = null;
  let observerOptions: IntersectionObserverInit | undefined;
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) {
        notify = callback;
        observerOptions = options;
      }
      observe(target: Element) { observed = target; }
      disconnect() {}
      unobserve() {}
      takeRecords() { return []; }
      readonly root = null;
      readonly rootMargin = '0px';
      readonly thresholds = [0];
    }
  );
  const fetcher = vi.fn(available);
  const { container } = render(
    <article>
      <LinkPreviewCard objectId='post-1' content='https://example.test/visible' enabled fetcher={fetcher} />
    </article>
  );

  expect(fetcher).not.toHaveBeenCalled();
  const target = container.querySelector('article')!;
  expect(observed).toBe(target);
  expect(observerOptions?.rootMargin).toBe('0px');
  notify(
    [{ target, isIntersecting: true } as unknown as IntersectionObserverEntry],
    {} as IntersectionObserver
  );
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
});

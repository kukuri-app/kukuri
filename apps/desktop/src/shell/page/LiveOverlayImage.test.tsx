import { render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { MediaDemandContext } from '@/components/core/mediaRetryContext';
import { buildPostMediaView } from '@/shell/viewModels/postMediaView';
import { LiveOverlayImage } from './LiveOverlayImage';

afterEach(() => vi.unstubAllGlobals());

test('shares a hash demand and releases it when gated, pending, or removed', () => {
  vi.stubGlobal('IntersectionObserver', undefined);
  const demand = vi.fn();
  const post = { object_id: 'post', attachments: [{ hash: 'gif', mime: 'image/gif', bytes: 100, role: 'image_original', status: 'Available' as const }] };
  const media = (gated = false, pending = false) => buildPostMediaView(post, {
    mediaObjectUrls: { gif: 'blob:shared-gif' }, adultContentGated: gated, advisoryPending: pending, unsupportedVideoManifests: {},
  });
  const view = (gated = false, pending = false) => <MediaDemandContext.Provider value={demand}><LiveOverlayImage media={media(gated, pending)} /></MediaDemandContext.Provider>;
  const rendered = render(view());
  expect(rendered.container.querySelector('img')).toHaveAttribute('src', 'blob:shared-gif');
  expect(demand).toHaveBeenLastCalledWith('gif', true);
  rendered.rerender(view(true));
  expect(rendered.container).toBeEmptyDOMElement();
  expect(demand).toHaveBeenLastCalledWith('gif', false);
  rendered.rerender(view(false, true));
  expect(rendered.container).toBeEmptyDOMElement();
  rendered.rerender(view());
  expect(demand).toHaveBeenLastCalledWith('gif', true);
  rendered.unmount();
  expect(demand).toHaveBeenLastCalledWith('gif', false);
});

test('missing image data leaves a local status without blocking the overlay', () => {
  const media = buildPostMediaView({ object_id: 'post', attachments: [{ hash: 'missing', mime: 'image/png', bytes: 1, role: 'image_original', status: 'Missing' }] }, {
    mediaObjectUrls: { missing: null }, adultContentGated: false, unsupportedVideoManifests: {},
  });
  const { container } = render(<LiveOverlayImage media={media} />);
  expect(screen.getByText('Failed to load.')).toBeInTheDocument();
  expect(container.querySelector('img')).toBeNull();
});

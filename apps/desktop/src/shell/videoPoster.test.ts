import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { blobToBase64 } from '@/lib/attachments';
import { generateVideoPoster } from './media';

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:poster-source');
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => {});
  vi.spyOn(HTMLVideoElement.prototype, 'videoWidth', 'get').mockReturnValue(1920);
  vi.spyOn(HTMLVideoElement.prototype, 'videoHeight', 'get').mockReturnValue(1080);
  vi.spyOn(HTMLMediaElement.prototype, 'readyState', 'get').mockReturnValue(2);
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
    drawImage: vi.fn(),
  } as unknown as CanvasRenderingContext2D);
  // Android WebView can postpone this callback beyond the poster deadline.
  vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation(() => {});
  vi.spyOn(HTMLCanvasElement.prototype, 'toDataURL').mockReturnValue(
    'data:image/jpeg;base64,CQgHBg=='
  );
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.querySelectorAll('video').forEach((video) => video.remove());
});

test('1080p poster completes with intact JPEG bytes when idle encoding is delayed', async () => {
  const outcome = generateVideoPoster(new File(['video'], 'clip.mp4', { type: 'video/mp4' }))
    .then((file) => ({ file }), (error: unknown) => ({ error }));
  await vi.runAllTimersAsync();
  const result = await outcome;
  expect(result).toHaveProperty('file');
  if (!('file' in result)) throw result.error;
  expect(result.file).toMatchObject({ name: 'clip.poster.jpg', type: 'image/jpeg', size: 4 });
  vi.useRealTimers();
  expect(await blobToBase64(result.file)).toBe('CQgHBg==');
  expect(document.querySelector('video')).toBeNull();
  expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:poster-source');
});

test.each(['decode timeout', 'JPEG failure'])('%s rejects and releases the decoder and URL', async (failure) => {
  if (failure === 'decode timeout') {
    vi.spyOn(HTMLMediaElement.prototype, 'readyState', 'get').mockReturnValue(0);
  } else {
    vi.spyOn(HTMLCanvasElement.prototype, 'toDataURL').mockImplementation(() => {
      throw new Error('JPEG failure');
    });
    vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation(() => {
      throw new Error('JPEG failure');
    });
  }
  const outcome = generateVideoPoster(new File(['video'], 'clip.mp4', { type: 'video/mp4' }))
    .catch((error: unknown) => error);
  await vi.runAllTimersAsync();
  expect(await outcome).toEqual(new Error('failed to generate video poster'));
  expect(document.querySelector('video')).toBeNull();
  expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:poster-source');
});

import { expect, test, vi } from 'vitest';

const prepare = vi.hoisted(() => vi.fn());
vi.mock('./webRuntime', () => ({ IS_WEB_RUNTIME: true, prepareWebAttachment: prepare }));
import { fileToCreateAttachment } from './attachments';

test('Web keeps the selected original across a draft snapshot without a whole base64 value', async () => {
  const file = new File(['original'], 'image.png', { type: 'image/png' });
  prepare.mockResolvedValueOnce({ owner: 'account-a', hash: 'original-hash' });
  const input = await fileToCreateAttachment(file, 'image_original');
  expect(input.data_base64).toBe('');
  expect({ ...input }).toMatchObject({
    file_name: 'image.png', mime: 'image/png', byte_size: 8, role: 'image_original',
    web_source: { blob: file, owner: 'account-a', hash: 'original-hash' },
  });
  expect(prepare).toHaveBeenCalledWith(file);
});

test('Web read failure returns to the existing caller without accepting an attachment', async () => {
  prepare.mockRejectedValueOnce(new DOMException('file is unreadable', 'NotReadableError'));
  await expect(fileToCreateAttachment(new File(['x'], 'image.png'), 'image_original')).rejects.toMatchObject({ name: 'NotReadableError' });
});

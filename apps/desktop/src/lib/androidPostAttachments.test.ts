import { afterEach, describe, expect, it, vi } from 'vitest';
vi.mock('./platform', () => ({ IS_ANDROID: true }));
import { fileToPostAttachment, materializeMessageAttachments, postAttachmentDocuments } from './androidPostAttachments';

afterEach(() => { delete window.__KUKURI_SELECTED_FILES__; });

describe('Android selected post files', () => {
  it('keeps the selected file through draft cloning without reading or serializing its body', async () => {
    const file = new File(['video'], 'selected.mp4', { type: 'video/mp4' });
    const read = vi.spyOn(FileReader.prototype, 'readAsDataURL');
    window.__KUKURI_SELECTED_FILES__ = [{ uri: 'content://picker/selected', name: file.name, size: file.size }];
    const attachment = { ...await fileToPostAttachment(file, 'video_manifest') };
    expect(read).not.toHaveBeenCalled();
    expect(postAttachmentDocuments([attachment])).toEqual([{ index: 0, uri: 'content://picker/selected' }]);
    expect(JSON.parse(JSON.stringify(attachment))).toEqual({ file_name: file.name, mime: 'video/mp4', byte_size: 5, data_base64: '', role: 'video_manifest' });
    read.mockRestore();
    const [messageAttachment] = await materializeMessageAttachments([attachment]);
    expect(messageAttachment.data_base64).toBe(btoa('video'));
    expect(postAttachmentDocuments([messageAttachment])).toEqual([]);
  });

  it('keeps pasted images on the existing byte path and cancel adds no document', async () => {
    window.__KUKURI_SELECTED_FILES__ = [];
    const attachment = await fileToPostAttachment(new File(['png'], 'pasted.png', { type: 'image/png' }), 'image_original');
    expect(attachment.data_base64).toBe(btoa('png'));
    expect(postAttachmentDocuments([attachment])).toEqual([]);
  });
});

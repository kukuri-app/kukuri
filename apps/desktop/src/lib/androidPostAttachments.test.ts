import { afterEach, describe, expect, it, vi } from 'vitest';
vi.mock('./platform', () => ({ IS_ANDROID: true }));
import { fileToPostAttachment, materializeMessageAttachments, postAttachmentDocuments } from './androidPostAttachments';

afterEach(() => { delete window.__KUKURI_SELECTED_FILES__; });

function returnPickedFiles(files: File[], uris: string[]) {
  window.__KUKURI_SELECTED_FILES__ = files.map((file, index) => ({ uri: uris[index], name: file.name, size: file.size }));
  const input = document.createElement('input');
  input.type = 'file';
  Object.defineProperty(input, 'files', { value: files });
  document.body.append(input);
  input.dispatchEvent(new Event('change', { bubbles: true }));
  input.remove();
}

describe('Android selected post files', () => {
  it('keeps the selected file through draft cloning without reading or serializing its body', async () => {
    const file = new File(['video'], 'selected.mp4', { type: 'video/mp4' });
    const read = vi.spyOn(FileReader.prototype, 'readAsDataURL');
    returnPickedFiles([file], ['content://picker/selected']);
    const attachment = { ...await fileToPostAttachment(file, 'video_manifest') };
    expect(read).not.toHaveBeenCalled();
    expect(postAttachmentDocuments([attachment])).toEqual([{ index: 0, uri: 'content://picker/selected' }]);
    expect(JSON.parse(JSON.stringify(attachment))).toEqual({ file_name: file.name, mime: 'video/mp4', byte_size: 5, data_base64: '', role: 'video_manifest' });
    read.mockRestore();
    const [messageAttachment] = await materializeMessageAttachments([attachment]);
    expect(messageAttachment.data_base64).toBe(btoa('video'));
    expect(postAttachmentDocuments([messageAttachment])).toEqual([]);
  });

  it('associates same-named selected files by File identity and leaves a pasted file on bytes', async () => {
    const first = new File(['one'], 'image.png', { type: 'image/png' });
    const second = new File(['two'], 'image.png', { type: 'image/png' });
    returnPickedFiles([first, second], ['content://picker/one', 'content://picker/two']);
    const attachments = await Promise.all([first, second].map(file => fileToPostAttachment(file, 'image_original')));
    expect(postAttachmentDocuments(attachments)).toEqual([{ index: 0, uri: 'content://picker/one' }, { index: 1, uri: 'content://picker/two' }]);
    expect(window.__KUKURI_SELECTED_FILES__).toEqual([]);
    const pasted = await fileToPostAttachment(new File(['new'], first.name, { type: first.type }), 'image_original');
    expect(postAttachmentDocuments([pasted])).toEqual([]);
    expect(pasted.data_base64).toBe(btoa('new'));
  });

  it('keeps pasted images on the existing byte path and cancel adds no document', async () => {
    window.__KUKURI_SELECTED_FILES__ = [];
    const attachment = await fileToPostAttachment(new File(['png'], 'pasted.png', { type: 'image/png' }), 'image_original');
    expect(attachment.data_base64).toBe(btoa('png'));
    expect(postAttachmentDocuments([attachment])).toEqual([]);
  });
});

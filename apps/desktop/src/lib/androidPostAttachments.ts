import type { CreateAttachmentInput } from './api/types';
import { fileToCreateAttachment } from './attachments';
import { IS_ANDROID } from './platform';

const selectedFile = Symbol('selected Android post file');
type SelectedFile = { uri: string; name: string; size: number | null };
type NativeAttachment = CreateAttachmentInput & {
  [selectedFile]?: { file: File; uri: string };
};

declare global {
  interface Window { __KUKURI_SELECTED_FILES__?: SelectedFile[] }
}

// File と URI はローカル下書きの寿命だけ保持する。Symbol は JSON/wire に含まれず、
// 既存の下書きの clone（object spread）は保持する。
export async function fileToPostAttachment(
  file: File,
  role: CreateAttachmentInput['role']
): Promise<CreateAttachmentInput> {
  if (!IS_ANDROID) return fileToCreateAttachment(file, role);
  const selected = window.__KUKURI_SELECTED_FILES__?.find(
    item => item.name === file.name && (item.size === null || item.size === file.size)
  );
  // 貼り付けた画像など OS picker 由来でない File は既存の bytes 入力を維持する。
  if (!selected) return fileToCreateAttachment(file, role);
  return {
    file_name: file.name, mime: file.type || 'application/octet-stream',
    byte_size: file.size, role, data_base64: '',
    [selectedFile]: { file, uri: selected.uri },
  } as NativeAttachment;
}

export function postAttachmentDocuments(attachments: CreateAttachmentInput[]) {
  return attachments.flatMap((attachment, index) => {
    const selected = (attachment as NativeAttachment)[selectedFile];
    return selected ? [{ index, uri: selected.uri }] : [];
  });
}

// DM は既存の暗号化 bytes 入力を維持する。投稿用 URI をその DTO へ流さない。
export async function materializeMessageAttachments(attachments: CreateAttachmentInput[]) {
  return Promise.all(attachments.map(attachment => {
    const selected = (attachment as NativeAttachment)[selectedFile];
    return selected ? fileToCreateAttachment(selected.file, attachment.role) : attachment;
  }));
}

import {
  type AttachmentView,
  type BlobMediaPayload,
  type ContentAdvisory,
} from '@/lib/api';
import {
  blobToCreateAttachment,
  fileToCreateAttachment,
} from '@/lib/attachments';

import {
  type DraftMediaItem,
  MEDIA_DEBUG_STORAGE_KEY,
  VIDEO_POSTER_TIMEOUT_MS,
} from './store';

export type MediaDebugValue = boolean | number | string | null | undefined;
export type MediaDebugFields = Record<string, MediaDebugValue>;

// #858: 投稿者自己申告の成人向けラベル(ADR 0046)。Rust 側 ADULT_CONTENT_LABEL と対。
export const ADULT_CONTENT_LABEL = 'adult';

export function hasAdultContentLabel(labels: string[] | null | undefined): boolean {
  return (labels ?? []).includes(ADULT_CONTENT_LABEL);
}

export function isAdultLabeledPost(post: {
  content_labels?: string[] | null;
  repost_of?: { content_labels?: string[] | null } | null;
  reply_preview?: { content_labels?: string[] | null } | null;
}): boolean {
  return (
    hasAdultContentLabel(post.content_labels) ||
    hasAdultContentLabel(post.repost_of?.content_labels) ||
    hasAdultContentLabel(post.reply_preview?.content_labels)
  );
}

// #1055: Community Node が発行した content advisory(ADR 0028 §8.6)のうち、client が成人向け
// ゲートの対象として扱う表示ラベル。未知ラベルは無視する(前方互換。node が語彙を増やしても
// client が勝手にゲートしない)。Rust 側 `GATING_ADVISORY_LABELS` と対。
export const GATING_ADVISORY_LABELS = ['adult', 'sensitive'] as const;

export function isGatingContentAdvisory(advisory: ContentAdvisory): boolean {
  return (GATING_ADVISORY_LABELS as readonly string[]).includes(advisory.label.trim());
}

export function hasGatingContentAdvisory(
  advisories: ContentAdvisory[] | null | undefined
): boolean {
  return (advisories ?? []).some(isGatingContentAdvisory);
}

/// 代替表示の説明に出す advisory を 1 件選ぶ。添付そのものへの判定(`blob_cid`)を優先し、
/// 無ければ投稿本文への判定(`post_id`)を使う。
export function primaryContentAdvisory(
  advisories: ContentAdvisory[] | null | undefined
): ContentAdvisory | null {
  const gating = (advisories ?? []).filter(isGatingContentAdvisory);
  return (
    gating.find((advisory) => advisory.subject_kind === 'blob_cid') ?? gating[0] ?? null
  );
}

/// #1690: 投稿カードに並べる画像の数。5 枚目以降は拡大表示で示す。
export const POST_CARD_IMAGE_LIMIT = 4;

/// 投稿の画像(動画の poster を除く)。カードは先頭 `POST_CARD_IMAGE_LIMIT` 枚を並べ、拡大表示はすべてを順に示す。
export function selectPostImages(post: { attachments: AttachmentView[] }): AttachmentView[] {
  return post.attachments.filter(
    (attachment) => attachment.mime.startsWith('image/') && attachment.role !== 'video_poster'
  );
}

export function selectVideoPoster(post: { attachments: AttachmentView[] }): AttachmentView | null {
  return selectVideoPosterAttachment(post.attachments);
}

export function selectVideoManifest(post: { attachments: AttachmentView[] }): AttachmentView | null {
  return selectVideoManifestAttachment(post.attachments);
}

export function selectPrimaryImageAttachment(
  attachments: AttachmentView[]
): AttachmentView | null {
  return attachments.find((attachment) => attachment.role === 'image_original') ?? null;
}

export function selectVideoPosterAttachment(
  attachments: AttachmentView[]
): AttachmentView | null {
  return attachments.find((attachment) => attachment.role === 'video_poster') ?? null;
}

export function selectVideoManifestAttachment(
  attachments: AttachmentView[]
): AttachmentView | null {
  return (
    attachments.find(
      (attachment) =>
        attachment.role === 'video_manifest' || attachment.mime.startsWith('video/')
    ) ?? null
  );
}

export function base64ToBytes(base64: string): Uint8Array<ArrayBuffer> {
  const binary = window.atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

export function createObjectUrlFromPayload(payload: BlobMediaPayload): string {
  const bytes = base64ToBytes(payload.bytes_base64);
  const normalizedBytes = new Uint8Array(bytes.length);
  normalizedBytes.set(bytes);
  return URL.createObjectURL(new Blob([normalizedBytes], { type: payload.mime }));
}

function isMediaDebugEnabled(): boolean {
  if (import.meta.env.MODE === 'test') {
    return false;
  }

  if (import.meta.env.DEV) {
    return true;
  }

  try {
    return window.localStorage.getItem(MEDIA_DEBUG_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

export function logMediaDebug(
  level: 'info' | 'warn',
  event: string,
  fields: MediaDebugFields
): void {
  if (!isMediaDebugEnabled()) {
    return;
  }

  const logger = level === 'warn' ? console.warn : console.info;
  logger(`[kukuri.media] ${event}`, fields);
}

export function mediaElementDebugFields(media: HTMLMediaElement): MediaDebugFields {
  return {
    current_src: media.currentSrc || media.getAttribute('src') || null,
    current_time: Number.isFinite(media.currentTime) ? media.currentTime : null,
    duration: Number.isFinite(media.duration) ? media.duration : null,
    ended: media.ended,
    error_code: media.error?.code ?? null,
    network_state: media.networkState,
    paused: media.paused,
    ready_state: media.readyState,
  };
}

function attachVideoDebugListeners(
  video: HTMLVideoElement,
  phase: string,
  fields: MediaDebugFields
): () => void {
  const eventNames = [
    'loadstart',
    'loadedmetadata',
    'loadeddata',
    'canplay',
    'durationchange',
    'seeked',
    'playing',
    'error',
  ] as const;
  const removeListeners = eventNames.map((eventName) => {
    const handler = () => {
      logMediaDebug(eventName === 'error' ? 'warn' : 'info', `${phase} ${eventName}`, {
        ...fields,
        ...mediaElementDebugFields(video),
        video_height: video.videoHeight || null,
        video_width: video.videoWidth || null,
      });
    };
    video.addEventListener(eventName, handler);
    return () => {
      video.removeEventListener(eventName, handler);
    };
  });

  return () => {
    for (const removeListener of removeListeners) {
      removeListener();
    }
  };
}

function posterFileName(fileName: string): string {
  const extensionIndex = fileName.lastIndexOf('.');
  const baseName = extensionIndex >= 0 ? fileName.slice(0, extensionIndex) : fileName;
  return `${baseName}.poster.jpg`;
}

function attachHiddenVideo(video: HTMLVideoElement) {
  video.setAttribute('aria-hidden', 'true');
  video.style.position = 'fixed';
  video.style.left = '-9999px';
  video.style.top = '0';
  video.style.width = '1px';
  video.style.height = '1px';
  video.style.opacity = '0';
  video.style.pointerEvents = 'none';
  document.body.appendChild(video);
}

async function waitForPosterFrame(video: HTMLVideoElement): Promise<void> {
  return await new Promise<void>((resolve, reject) => {
    let settled = false;

    const cleanup = () => {
      video.removeEventListener('loadeddata', resolveIfReady);
      video.removeEventListener('canplay', resolveIfReady);
      video.removeEventListener('seeked', resolveIfReady);
      video.removeEventListener('timeupdate', resolveIfReady);
      video.removeEventListener('loadedmetadata', handleMetadata);
      video.removeEventListener('error', fail);
    };

    const finish = () => {
      if (settled) {
        return;
      }
      settled = true;
      cleanup();
      resolve();
    };

    const fail = () => {
      if (settled) {
        return;
      }
      settled = true;
      cleanup();
      reject(new Error('failed to generate video poster'));
    };

    const resolveIfReady = () => {
      if (
        video.videoWidth > 0 &&
        video.videoHeight > 0 &&
        video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA
      ) {
        finish();
      }
    };

    const handleMetadata = () => {
      resolveIfReady();
      if (settled) {
        return;
      }

      const duration = Number.isFinite(video.duration) ? video.duration : 0;
      const seekTarget = duration > 0 ? Math.min(duration / 2, 0.1) : 0.1;
      if (seekTarget > 0) {
        try {
          video.currentTime = seekTarget;
        } catch {
          // Some platforms reject seek before decode warms up.
        }
      }

      try {
        const playAttempt = video.play();
        if (playAttempt && typeof playAttempt.then === 'function') {
          void playAttempt.then(() => {
            video.pause();
            resolveIfReady();
          });
        }
      } catch {
        // ignore
      }
    };

    video.addEventListener('loadeddata', resolveIfReady);
    video.addEventListener('canplay', resolveIfReady);
    video.addEventListener('seeked', resolveIfReady);
    video.addEventListener('timeupdate', resolveIfReady);
    video.addEventListener('loadedmetadata', handleMetadata);
    video.addEventListener('error', fail, { once: true });
    resolveIfReady();
  });
}

export async function generateVideoPoster(file: File): Promise<File> {
  const videoObjectUrl = URL.createObjectURL(file);
  logMediaDebug('info', 'poster generation start', {
    file_name: file.name,
    mime: file.type || null,
    size: file.size,
    video_object_url: videoObjectUrl,
  });

  try {
    return await new Promise<File>((resolve, reject) => {
      const video = document.createElement('video');
      const canvas = document.createElement('canvas');
      let finished = false;
      const removeDebugListeners = attachVideoDebugListeners(video, 'poster', {
        file_name: file.name,
        mime: file.type || null,
        size: file.size,
      });

      const fail = () => {
        if (finished) {
          return;
        }
        finished = true;
        logMediaDebug('warn', 'poster generation failed', {
          file_name: file.name,
          mime: file.type || null,
          size: file.size,
          ...mediaElementDebugFields(video),
          video_height: video.videoHeight || null,
          video_width: video.videoWidth || null,
        });
        cleanup();
        reject(new Error('failed to generate video poster'));
      };

      const timeoutId = window.setTimeout(fail, VIDEO_POSTER_TIMEOUT_MS);

      const cleanup = () => {
        window.clearTimeout(timeoutId);
        removeDebugListeners();
        try {
          video.pause();
        } catch {
          // ignore
        }
        video.removeAttribute('src');
        try {
          video.load();
        } catch {
          // ignore
        }
        video.remove();
      };

      video.preload = 'auto';
      video.muted = true;
      video.playsInline = true;
      attachHiddenVideo(video);

      video.src = videoObjectUrl;
      video.load();

      void waitForPosterFrame(video)
        .then(() => {
          if (finished) {
            return;
          }

          const width = video.videoWidth;
          const height = video.videoHeight;
          if (!width || !height) {
            fail();
            return;
          }

          logMediaDebug('info', 'poster frame ready', {
            file_name: file.name,
            height,
            mime: file.type || null,
            size: file.size,
            width,
            ...mediaElementDebugFields(video),
          });

          canvas.width = width;
          canvas.height = height;
          const context = canvas.getContext('2d');
          if (!context) {
            fail();
            return;
          }

          context.drawImage(video, 0, 0, width, height);
          // WebView の idle JPEG callback を待たず、読み込めた frame を同じ品質で保存する。
          const dataUrl = canvas.toDataURL('image/jpeg', 0.85);
          const poster = new File([base64ToBytes(dataUrl.split(',')[1])], posterFileName(file.name), {
            type: 'image/jpeg',
          });
          finished = true;
          cleanup();
          logMediaDebug('info', 'poster generation complete', {
            blob_size: poster.size,
            file_name: file.name,
            mime: file.type || null,
            poster_file_name: poster.name,
            size: file.size,
          });
          resolve(poster);
        })
        .catch((error: unknown) => {
          logMediaDebug('warn', 'poster generation exception', {
            error: error instanceof Error ? error.message : 'unknown error',
            file_name: file.name,
            mime: file.type || null,
            size: file.size,
          });
          fail();
        });
    });
  } finally {
    URL.revokeObjectURL(videoObjectUrl);
  }
}

export async function buildImageDraftItem(
  file: File,
  nextDraftId: () => string
): Promise<DraftMediaItem> {
  const attachment = await fileToCreateAttachment(file, 'image_original');
  return {
    id: nextDraftId(),
    source_name: file.name,
    preview_url: URL.createObjectURL(file),
    attachments: [attachment],
  };
}

export async function buildVideoDraftItem(
  file: File,
  nextDraftId: () => string
): Promise<DraftMediaItem> {
  const posterFile = await generateVideoPoster(file);
  return {
    id: nextDraftId(),
    source_name: file.name,
    preview_url: URL.createObjectURL(posterFile),
    attachments: [
      await fileToCreateAttachment(file, 'video_manifest'),
      await blobToCreateAttachment(posterFile, posterFile.name, 'video_poster'),
    ],
  };
}

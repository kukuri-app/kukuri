import { createContext } from 'react';
import type { DesktopApi } from '@/lib/api';

export const MediaBookmarksContext = createContext<{ api: DesktopApi; gatedMediaHashes?: readonly string[]; rememberDraftPreview?: (item: import('@/shell/store').DraftMediaItem) => void;
  // 投稿を取り下げる(メディアの列の「投稿」。#1949)。取り下げたら true。
  withdrawPost?: (post: import('@/lib/api').PostView) => Promise<boolean> } | null>(null);

export const MEDIA_BOOKMARK_CHANGED = 'kukuri:media-bookmark-changed';

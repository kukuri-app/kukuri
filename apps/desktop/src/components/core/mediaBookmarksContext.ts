import { createContext } from 'react';
import type { DesktopApi } from '@/lib/api';

export const MediaBookmarksContext = createContext<{ api: DesktopApi; rememberDraftPreview?: (item: import('@/shell/store').DraftMediaItem) => void } | null>(null);

export const MEDIA_BOOKMARK_CHANGED = 'kukuri:media-bookmark-changed';

import { createContext } from 'react';

/// #1207: 取得に失敗したメディアの明示再試行。shell が提供し、未提供(Storybook や読み取り専用の
/// 表示)では失敗表示だけを出す。
export type MediaRetryHandler = (hashes: readonly string[]) => void;

export const MediaRetryContext = createContext<MediaRetryHandler | null>(null);

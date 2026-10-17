import { createContext } from 'react';

import type { SupportedLocale } from '@/i18n';
import type { DesktopApi, PostLiveView } from '@/lib/api';
import type { InternalSmartReference } from '@/lib/internalLinks';

/** Live の一覧と投稿の中の Live の表示が使う操作(#1839)。shell が 1 か所で渡す。 */
export type LiveActions = {
  api: DesktopApi;
  locale: SupportedLocale;
  localPubkey: string;
  pendingBySessionId: Record<string, true>;
  join: (topic: string, sessionId: string) => Promise<void>;
  leave: (topic: string, sessionId: string) => Promise<void>;
  end: (topic: string, sessionId: string) => Promise<void>;
  introduce: (live: PostLiveView, title: string) => void;
  openPost: (topic: string, postId: string) => void;
  copyLink: (link: string) => void;
  activateReference: (reference: InternalSmartReference) => void;
};

export const LiveActionsContext = createContext<LiveActions | null>(null);

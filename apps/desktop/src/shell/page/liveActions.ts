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
  // #1839 AC-3: shell の上部の配信の操作。この端末から同時に配信するのは 1 つだけ。
  broadcast: LiveBroadcastTarget | null;
  openBroadcast: (target: LiveBroadcastTarget) => void;
  closeBroadcast: () => void;
  openPost: (topic: string, postId: string) => void;
  copyLink: (link: string) => void;
  activateReference: (reference: InternalSmartReference) => void;
};

export type LiveBroadcastTarget = { topic: string; sessionId: string; title: string };

export const LiveActionsContext = createContext<LiveActions | null>(null);

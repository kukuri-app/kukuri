import { createContext } from 'react';

import type { SupportedLocale } from '@/i18n';
import type { LiveBroadcastTarget } from '@/components/extended/liveBroadcast';
import type { DesktopApi, LiveVideoSource, PostLiveView } from '@/lib/api';
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
  // #1839 AC-3: この端末の配信(shell の上部の bar)。配信中の Live の id と、配信の開始。同時に配信するのは 1 つだけ。
  broadcastingSessionId: string | null;
  startBroadcast: (target: LiveBroadcastTarget, source: LiveVideoSource) => void;
  openPost: (topic: string, postId: string) => void;
  copyLink: (link: string) => void;
  activateReference: (reference: InternalSmartReference) => void;
};

export const LiveActionsContext = createContext<LiveActions | null>(null);

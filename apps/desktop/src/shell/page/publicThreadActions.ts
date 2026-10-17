import { createContext } from 'react';

import type { DesktopApi, PublicChannelView } from '@/lib/api';

/** 公開スレッド(#1840)の一覧・詳細の列と本文のリンクの表示が使う操作。shell が 1 か所で渡す。 */
export type PublicThreadActions = {
  api: DesktopApi;
  /** `channelId` が null なら topic の一覧の列、あれば詳細の列を開く(詳細は一覧の列を親にする)。 */
  open: (topic: string, channelId: string | null) => void;
  /** 一覧の列の「スレッドを作る」の Dialog を開く。 */
  create: (topic: string) => void;
  copyLink: (topic: string, channelId: string) => void;
  /** 読んだスレッドを覚える(列の見出し・投稿の公開範囲・本文のリンクの表示に使う)。 */
  remember: (threads: PublicChannelView[]) => void;
  /** ID から 1 件読んで覚える。同じスレッドは並行して読まず、読めたものは使い回す(読めなければ次にまた読む)。 */
  load: (topic: string, channelId: string) => Promise<PublicChannelView | null>;
  /** この端末で最後に作ったスレッド。その topic の一覧の列は先頭から読み直す。 */
  lastCreated: PublicChannelView | null;
};

export const PublicThreadActionsContext = createContext<PublicThreadActions | null>(null);

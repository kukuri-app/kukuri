import { createContext } from 'react';

import type { DesktopApi, PublicChannelView } from '@/lib/api';
import { useDesktopShellStore } from '@/shell/store';

/** 公開スレッド(#1840)の一覧・詳細の列と本文のリンクの表示が使う操作。shell が 1 か所で渡す。 */
export type PublicThreadActions = {
  api: DesktopApi;
  /** `channelId` が null なら topic の一覧の列、あれば詳細の列を開く(詳細は一覧の列を親にする)。 */
  open: (topic: string, channelId: string | null) => void;
  /** 一覧の列の「スレッドを作る」の Dialog を開く。 */
  create: (topic: string) => void;
  copyLink: (topic: string, channelId: string) => void;
  /** 詳細の列を親にして、個別ライブを大きく表示する。 */
  openLive: (topic: string, channelId: string, sessionId: string) => void;
  /** 読んだスレッドを覚える(列の見出し・投稿の公開範囲・本文のリンクの表示に使う)。 */
  remember: (threads: PublicChannelView[]) => void;
  /** ID から 1 件読んで覚える。同じスレッドは並行して読まず、読めたものは使い回す(読めなければ次にまた読む)。 */
  load: (topic: string, channelId: string) => Promise<PublicChannelView | null>;
  /** 使い回さずに ID から読み直して覚える(表示中の詳細が、変更と削除を取り込む。#1888)。 */
  refresh: (topic: string, channelId: string) => Promise<PublicChannelView | null>;
  /** 作成者のタイトルの変更の Dialog を開く(#1888)。 */
  rename: (thread: PublicChannelView) => void;
  /** 作成者がスレッドを削除する。確かめてから削除し、取り消せない(#1888)。 */
  remove: (thread: PublicChannelView) => void;
  /** この端末で最後に作ったスレッド。その topic の一覧の列は先頭から読み直す。 */
  lastCreated: PublicChannelView | null;
};

export const PublicThreadActionsContext = createContext<PublicThreadActions | null>(null);

/// Live の画面を出すか(Live・Stream の画面は開発者モードでだけ出す。README の Experimental。Web も同じ、#1879)。
export function useLiveSurfacesEnabled(): boolean {
  return useDesktopShellStore((state) => state.developerModeEnabled);
}

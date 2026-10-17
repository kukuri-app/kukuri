import type { TFunction } from 'i18next';
import { useShallow } from 'zustand/react/shallow';

import { isPublicChannelId } from '@/lib/publicChannels';
import { knownPublicThreadTitle } from '@/shell/slices/publicThreads';
import type { ColumnKind, ColumnState } from '@/shell/slices/workspace';
import { useDesktopShellStore } from '@/shell/store';

/// 列の種類ごとの見出し(Column の header と Control Center)。公開スレッドの詳細の列の見出しも持つ。
export type ColumnTitles = Record<ColumnKind, string> & { publicThread: string };

export function columnTitlesFor(t: TFunction): ColumnTitles {
  return {
    timeline: t('shell:primarySections.timeline'),
    notifications: t('shell:primarySections.notifications'),
    thread: t('shell:context.thread'),
    profile: t('shell:primarySections.profile'),
    explore: t('shell:primarySections.explore'),
    messages: t('shell:primarySections.messages'),
    conversation: 'Conversation',
    stream: t('shell:primarySections.live'),
    game: t('shell:primarySections.game'),
    metaverse: 'Metaverse',
    publicThreads: t('shell:publicThreads.listTitle'),
    publicThread: t('shell:publicThreads.title'),
  };
}

/// 列の見出し。公開スレッドの詳細(scope の channel が公開スレッドのタイムラインの列)は「公開スレッド」(#1840 判断 1)。
export function columnTitle(titles: ColumnTitles, column: Pick<ColumnState, 'kind' | 'scope'>): string {
  return column.kind === 'timeline' && isPublicChannelId(column.scope?.channelId)
    ? titles.publicThread
    : titles[column.kind];
}

/// 列の scope の公開スレッドのタイトル(作成者を非表示にしたものは除く)。タイトルは変わらないので、知ったときだけ描き直す。
export function usePublicThreadColumnTitles(): Record<string, string> {
  return useDesktopShellStore(
    useShallow((state) => {
      const titles: Record<string, string> = {};
      for (const column of state.workspaceState.columns) {
        const channelId = column.scope?.channelId;
        if (!isPublicChannelId(channelId)) continue;
        const title = knownPublicThreadTitle(state.publicThreadsById, state.knownAuthorsByPubkey, channelId);
        if (title) titles[channelId] = title;
      }
      return titles;
    })
  );
}

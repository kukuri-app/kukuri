import type { TFunction } from 'i18next';

import type { ColumnKind } from '@/shell/slices/workspace';

/// 列の種類ごとの見出し（Column の header と Control Center の列の追加で使う）。
export function columnTitlesFor(t: TFunction): Record<ColumnKind, string> {
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
  };
}

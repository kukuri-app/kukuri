import { createContext, type ReactNode, useContext, useMemo } from 'react';

import { firstPublicThreadLink } from '@/lib/internalLinks';

/** #1840: 本文の中の公開スレッドのリンクの表示。スレッドの取得は shell が持つので、本文の部品はここから受け取る。 */
export type PublicThreadSlotValue = {
  /** 知っているスレッドのタイトル(知らなければ `undefined`)を受け取って描く(本文の chip と公開範囲の icon)。 */
  withTitle: (channelId: string, render: (title: string | undefined) => ReactNode) => ReactNode;
  /** 本文の chip の文言。知らなければ `fallback` を出し、画面内にある間に ID から読む（判断 10: どのリンクもタイトル）。 */
  renderChipTitle: (topic: string, channelId: string, fallback: string) => ReactNode;
  /** 本文の下に出す、タイトルと最初の投稿の card。 */
  renderCard: (topic: string, channelId: string) => ReactNode;
};

// eslint-disable-next-line react-refresh/only-export-components
export const PublicThreadSlot = createContext<PublicThreadSlotValue | null>(null);

/** 本文の中の最初の公開スレッドのリンクの card（判断 10: 外部のリンクの preview と同じく 1 件だけ）。 */
export function PublicThreadLinkOutlet({ content }: { content: string }) {
  const slot = useContext(PublicThreadSlot);
  const link = useMemo(() => (slot ? firstPublicThreadLink(content) : null), [content, slot]);
  return link && slot ? slot.renderCard(link.topic, link.channelId) : null;
}

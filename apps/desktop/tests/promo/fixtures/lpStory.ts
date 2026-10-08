import { readFileSync } from 'node:fs';
import path from 'node:path';

import type { BlobMediaPayload, ProfileAssetView } from '../../../src/lib/api';
import type { DesktopMockApiOptions } from '../../../src/mocks/desktopMockModel';
import { FUTABA, MINATO, post, replyFields, type Person, type PromoLocale } from './demoStory';

/**
 * LP の画面 (#1668) の合成のデモ物語。話題「ベランダ菜園」で、ふたば（操作者）が質問し、
 * みなとが答え、みなとの招待でプライベートチャンネル「苗の交換会」に入って話を続ける。
 *
 * 台本の正本は docs/progress/2026-09-15-promo-lp-brief.md の LP の shot list (L0〜L3)。
 * 登場人物と公開鍵は Product Hunt・note・X の物語 (demoStory.ts) と同じデモ identity を使う。
 */

/** 話題の名前。言語ごとに、その言語の読み手に自然な名前にする。 */
export const GARDEN_TOPIC_NAME: Record<PromoLocale, string> = {
  ja: 'ベランダ菜園',
  en: 'gardening',
};

export function gardenTopic(locale: PromoLocale) {
  return `kukuri:topic:${GARDEN_TOPIC_NAME[locale]}`;
}

/** 1789466400 = 2026-09-15T10:00:00Z。demoStory.ts と同じく、撮り直しても画面が変わらないよう固定する。 */
const BASE_TIME = 1789466400;
const CHANNEL_ID = 'promo-garden-swap';

export const GARDEN_COPY = {
  ja: {
    question: 'ミニトマトの脇芽、どこまで摘んでいますか？今年は葉ばかり茂ってしまいました。',
    answer: '主枝を1本にして、脇芽は小さいうちに摘むと実がよく付きました。',
    thanks: 'なるほど。今週末、さっそく摘んでみます。',
    firstRed: 'ベランダのミニトマト、今朝ひとつ目が赤くなりました。',
    channelLabel: '苗の交換会（デモ）',
    channel: [
      '来週末、余った苗を持ち寄りませんか？',
      'いいですね。シソとバジルの苗を持っていきます。',
      'では土曜の10時に、いつもの公園で。',
    ],
    searchWord: 'ミニトマト',
  },
  en: {
    question: 'How far do you pinch the side shoots on cherry tomatoes? Mine grew mostly leaves this year.',
    answer: 'Keep one main stem and pinch side shoots while they are small. I got much more fruit that way.',
    thanks: "Good to know. I'll start pinching this weekend.",
    firstRed: 'The first cherry tomato on my balcony turned red this morning.',
    channelLabel: 'Seedling swap (demo)',
    channel: [
      'Want to bring your extra seedlings next weekend?',
      "Sounds good. I'll bring shiso and basil seedlings.",
      'Saturday at 10, at the usual park then.',
    ],
    searchWord: 'tomato',
  },
} as const;

export const GARDEN_QUESTION_ID = 'promo-garden-question';

/**
 * デモ参加者のプロフィール画像。撮影用に作った絵（ふたば: 双葉の芽、みなと: ミニトマト）で、
 * 実在の人物や第三者の画像ではない。
 */
function avatar(file: string): { asset: ProfileAssetView; payload: BlobMediaPayload } {
  const bytes = readFileSync(path.join(import.meta.dirname, 'avatars', file));
  return {
    asset: { hash: `promo-avatar-${file.replace(/\.png$/, '')}`, mime: 'image/png', bytes: bytes.length, role: 'profile_avatar' },
    payload: { bytes_base64: bytes.toString('base64'), mime: 'image/png' },
  };
}

/**
 * 撮影に使う mock の seed。開発者モードの面は seed にも画面にも現れない。
 *
 * - 公開の話題: ふたばの質問、みなとの答え（返信）、みなとの前日の投稿
 * - プライベートチャンネル: みなとが作った招待限定のチャンネル。招待を受け取ると参加でき、
 *   参加した時点で 3 件の会話が見える
 *
 * 検索の語（searchWord）は、質問と前日の投稿にだけ含め、答えとチャンネルの投稿には含めない。
 * mock の検索は seed の全投稿の本文を照合するので、含めるとチャンネルの投稿まで検索に出る。
 */
export function createGardenSeed(locale: PromoLocale): DesktopMockApiOptions {
  const copy = GARDEN_COPY[locale];
  const topic = gardenTopic(locale);
  const futaba = avatar('futaba.png');
  const minato = avatar('minato.png');
  const pictureOf = (person: Person) => ({
    author_picture_asset: person === FUTABA ? futaba.asset : minato.asset,
  });
  const question = post({
    id: GARDEN_QUESTION_ID,
    person: FUTABA,
    locale,
    content: copy.question,
    createdAt: BASE_TIME,
    reactions: [{ emoji: '👍', count: 2 }],
    extra: pictureOf(FUTABA),
  });
  const answer = post({
    id: 'promo-garden-answer',
    person: MINATO,
    locale,
    content: copy.answer,
    createdAt: BASE_TIME + 600,
    extra: { ...replyFields(question, topic), ...pictureOf(MINATO) },
  });
  const firstRed = post({
    id: 'promo-garden-first-red',
    person: MINATO,
    locale,
    content: copy.firstRed,
    createdAt: BASE_TIME - 86400,
    reactions: [{ emoji: '🍅', count: 4 }],
    extra: pictureOf(MINATO),
  });
  // チャンネルの投稿は、タイムラインと同じく新しい順に並べる。
  const channel = [...copy.channel]
    .map((content, index) => {
      const person = index === 1 ? FUTABA : MINATO;
      return post({
        id: `promo-garden-channel-${index}`,
        person,
        locale,
        content,
        createdAt: BASE_TIME + 3600 + index * 300,
        extra: { channel_id: CHANNEL_ID, audience_label: 'Private channel', ...pictureOf(person) },
      });
    })
    .reverse();

  return {
    myProfile: {
      pubkey: FUTABA.pubkey,
      name: FUTABA.name[locale],
      display_name: FUTABA.name[locale],
      about: locale === 'ja' ? 'kukuri を試しているデモ用アカウントです。' : 'A demo account trying kukuri.',
      picture_asset: futaba.asset,
    },
    authorSocialViews: {
      [MINATO.pubkey]: {
        author_pubkey: MINATO.pubkey,
        name: MINATO.name[locale],
        display_name: MINATO.name[locale],
        following: true,
        followed_by: true,
        mutual: true,
        picture_asset: minato.asset,
      },
    },
    seedBlobPayloads: {
      [futaba.asset.hash]: futaba.payload,
      [minato.asset.hash]: minato.payload,
    },
    seedPosts: { [topic]: [...channel, answer, question, firstRed] },
    // 招待を受け取ったときに参加するチャンネル。作ったのはみなと。
    invitePreview: {
      channel_id: CHANNEL_ID,
      topic_id: topic,
      channel_label: copy.channelLabel,
      inviter_pubkey: MINATO.pubkey,
      owner_pubkey: MINATO.pubkey,
      epoch_id: 'promo-garden-epoch-1',
      expires_at: null,
      namespace_secret_hex: 'a'.repeat(64),
    },
    notifications: [],
    // 撮影中に書く返信 (L2) を、答えの少し後の時刻に並べる。
    clockBase: BASE_TIME + 1200,
  };
}

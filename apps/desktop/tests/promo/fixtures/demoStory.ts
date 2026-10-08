import type { Page } from '@playwright/test';

import type { DesktopMockApiOptions, SeedPostInput } from '../../../src/mocks/desktopMockModel';
import { DEVELOPER_MODE_STORAGE_KEY } from '../../../src/lib/developerMode';
import { DESKTOP_THEME_STORAGE_KEY } from '../../../src/lib/theme';

/**
 * 告知素材の 3 場面で共通して使う、合成のデモ物語 (#1039)。
 *
 * 台本の正本は docs/progress/2026-09-15-promo-lp-brief.md。
 * 実在する利用者の投稿・プロフィール・鍵・token は一切入力にしない。
 * 登場人物は表示名の時点でデモと分かるようにする。
 */

export type PromoLocale = 'ja' | 'en';

/** 舞台にする話題。既定で存在する初期トピックを使い、架空のトピックを新規作成しない。 */
export const DEMO_TOPIC = 'kukuri:topic:dev';

/**
 * 合成した公開鍵。実在の鍵ではなく、撮影用に固定した 64 桁の hex。
 *
 * 操作者 (ふたば) は browser mock が「自分」として扱う公開鍵に揃える
 * (mocks/mockRuntime.ts の local_author_pubkey)。揃えないと撮影中に書いた投稿が
 * 自分の投稿として解決されず、投稿者が「不明なユーザー」と表示される。
 */
const MINATO_PUBKEY = '5'.repeat(64);
const FUTABA_PUBKEY = 'f'.repeat(64);

/**
 * 投稿時刻は固定する。相対時刻や撮影日時が混ざると、再撮影のたびに画面が変わる。
 * 1789466400 = 2026-09-15T10:00:00Z。
 */
const BASE_TIME = 1789466400;

export type Person = { pubkey: string; name: Record<PromoLocale, string> };

export const MINATO: Person = {
  pubkey: MINATO_PUBKEY,
  name: { ja: 'みなと（デモ）', en: 'Minato (demo)' },
};

export const FUTABA: Person = {
  pubkey: FUTABA_PUBKEY,
  name: { ja: 'ふたば（デモ）', en: 'Futaba (demo)' },
};

/** 私的チャンネルの名前。S3 の撮影で入力する。 */
export const DEMO_CHANNEL_LABEL: Record<PromoLocale, string> = {
  ja: 'カラム配置の相談（デモ）',
  en: 'Column layout chat (demo)',
};

const CONVERSATION: Record<PromoLocale, { root: string; reply1: string; reply2: string }> = {
  ja: {
    root: 'タイムラインとスレッドを左右に並べて、見つけるを右端に置いています。話題を追いながら返信できるのが気に入っています。',
    reply1:
      '同じ並びにしました。通知のカラムを細くして端に寄せると、会話のほうに集中できますね。',
    reply2: 'カラムを固定しておくと、次に起動したときも並びがそのままなので助かります。',
  },
  en: {
    root: 'I keep Timeline and Thread side by side, with Explore at the right edge. Replying while following the topic works well.',
    reply1:
      'Same arrangement here. Making the Notifications column narrow and moving it to the edge helps me focus on the conversation.',
    reply2: 'Pinning the columns means the arrangement is still there the next time I launch it.',
  },
};

const OTHER_TOPIC_POSTS: Record<PromoLocale, { general: string; test: string }> = {
  ja: {
    general: '今日から使いはじめました。話題ごとにカラムを開けるのが分かりやすいです。',
    test: '画像の添付と同期を試しています。別の端末からも同じ投稿が見えました。',
  },
  en: {
    general: 'Started using it today. Opening a column per topic makes it easy to follow.',
    test: 'Trying image attachments and syncing. The same post showed up on my other device.',
  },
};

export function post(input: {
  id: string;
  person: Person;
  locale: PromoLocale;
  content: string;
  createdAt: number;
  replyTo?: string;
  rootId?: string;
  reactions?: { emoji: string; count: number }[];
  /** 返信先・私的チャンネルなど、上の引数で表せない項目を上書きする。 */
  extra?: Partial<SeedPostInput>;
}): SeedPostInput {
  const { id, person, locale, content, createdAt, replyTo, rootId, reactions, extra } = input;
  return {
    object_id: id,
    envelope_id: `${id}-envelope`,
    author_pubkey: person.pubkey,
    author_name: person.name[locale],
    author_display_name: person.name[locale],
    following: true,
    followed_by: true,
    mutual: true,
    friend_of_friend: false,
    object_kind: replyTo ? 'comment' : 'post',
    content,
    content_status: 'Available',
    attachments: [],
    created_at: createdAt,
    reply_to: replyTo ?? null,
    root_id: rootId ?? id,
    audience_label: 'Public',
    reaction_summary: (reactions ?? []).map((reaction) => ({
      reaction_key_kind: 'emoji',
      // 正規化キーは `emoji:<絵文字>`。形式が違うと、撮影中に押したリアクションが
      // 既存の件数へ加算されず、別のリアクションとして並ぶ。
      normalized_reaction_key: `emoji:${reaction.emoji}`,
      emoji: reaction.emoji,
      custom_asset: null,
      count: reaction.count,
    })),
    my_reactions: [],
    ...extra,
  };
}

/**
 * 返信の項目。実アプリの runtime と同じく、返信先の投稿者と本文を reply_preview に載せる。
 * 載せないと、タイムラインで返信が返信先の無い投稿のように見える。
 */
export function replyFields(parent: SeedPostInput, topic: string): Partial<SeedPostInput> {
  return {
    object_kind: 'comment',
    reply_to: parent.object_id,
    root_id: parent.root_id ?? parent.object_id,
    reply_preview: {
      object_id: parent.object_id,
      topic,
      author: {
        pubkey: parent.author_pubkey,
        name: parent.author_name ?? null,
        display_name: parent.author_display_name ?? null,
        picture_asset: parent.author_picture_asset ?? null,
      },
      content: parent.content,
      content_status: parent.content_status,
      attachments: [],
      root_id: parent.root_id ?? null,
      reply_to: parent.reply_to ?? null,
    },
  };
}

export const DEMO_ROOT_POST_ID = 'promo-demo-root';

/**
 * 撮影に使う mock の seed を作る。
 *
 * 同じ話題と同じ 2 人を 3 場面すべてに通す。開発者モードは有効にしないので、
 * 実験機能 (Dome / Live / Game / Stream) の面は seed にも画面にも現れない。
 */
export function createDemoSeed(locale: PromoLocale): DesktopMockApiOptions {
  const talk = CONVERSATION[locale];
  const others = OTHER_TOPIC_POSTS[locale];

  return {
    myProfile: {
      pubkey: FUTABA_PUBKEY,
      name: FUTABA.name[locale],
      display_name: FUTABA.name[locale],
      about: locale === 'ja' ? 'kukuri を試しているデモ用アカウントです。' : 'A demo account trying kukuri.',
    },
    authorSocialViews: {
      [MINATO_PUBKEY]: {
        author_pubkey: MINATO_PUBKEY,
        name: MINATO.name[locale],
        display_name: MINATO.name[locale],
        following: true,
        followed_by: true,
        mutual: true,
      },
    },
    seedPosts: {
      [DEMO_TOPIC]: [
        post({
          id: DEMO_ROOT_POST_ID,
          person: MINATO,
          locale,
          content: talk.root,
          createdAt: BASE_TIME,
          reactions: [{ emoji: '👍', count: 2 }],
        }),
        post({
          id: 'promo-demo-reply-1',
          person: FUTABA,
          locale,
          content: talk.reply1,
          createdAt: BASE_TIME + 420,
          replyTo: DEMO_ROOT_POST_ID,
          rootId: DEMO_ROOT_POST_ID,
        }),
        post({
          id: 'promo-demo-reply-2',
          person: MINATO,
          locale,
          content: talk.reply2,
          createdAt: BASE_TIME + 900,
          replyTo: DEMO_ROOT_POST_ID,
          rootId: DEMO_ROOT_POST_ID,
        }),
      ],
      'kukuri:topic:general': [
        post({
          id: 'promo-demo-general',
          person: FUTABA,
          locale,
          content: others.general,
          createdAt: BASE_TIME - 3600,
        }),
      ],
      'kukuri:topic:test': [
        post({
          id: 'promo-demo-test',
          person: MINATO,
          locale,
          content: others.test,
          createdAt: BASE_TIME - 7200,
        }),
      ],
    },
    notifications: [],
    // 撮影中に作る投稿 (S3 のチャンネルでの一言) を、seed の会話の少し後の時刻に並べる。
    clockBase: BASE_TIME + 1800,
  };
}

export type SeedOptions = {
  locale: PromoLocale;
  theme: 'dark' | 'light';
};

/**
 * ページを開く前に mock の seed と表示設定を入れる。
 *
 * 開発者モードは必ず `false` を書き込む。既定値に任せず明示することで、
 * 実験機能が写り込んだ素材を作らない (INVAR-3)。
 */
export async function seedDemoStory(
  page: Page,
  { locale, theme }: SeedOptions,
  seed: DesktopMockApiOptions = createDemoSeed(locale)
) {
  await page.addInitScript(
    ({ seed, locale, theme, themeKey, developerKey }) => {
      window.localStorage.setItem('kukuri.desktop.locale', locale);
      window.localStorage.setItem(themeKey, theme);
      window.localStorage.setItem(developerKey, 'false');
      (window as { __KUKURI_PROMO_MOCK_SEED__?: unknown }).__KUKURI_PROMO_MOCK_SEED__ = seed;
    },
    {
      seed: seed as unknown as Record<string, unknown>,
      locale,
      theme,
      themeKey: DESKTOP_THEME_STORAGE_KEY,
      developerKey: DEVELOPER_MODE_STORAGE_KEY,
    }
  );
}

import { expect, test, type Locator, type Page } from '@playwright/test';

import { captureSceneStep } from './fixtures/captureScene';
import { seedDemoStory, type PromoLocale } from './fixtures/demoStory';
import { createGardenSeed, GARDEN_COPY, GARDEN_TOPIC_NAME } from './fixtures/lpStory';
import type { CaptureTarget } from './promoArtifacts';

/**
 * LP の画面 (#1668) を撮る。台本は docs/progress/2026-09-15-promo-lp-brief.md の L0〜L3。
 *
 * L0 Hero（話題のタイムライン）/ L1 話題を見つける（検索）/ L2 会話に加わる（スレッドで返信）/
 * L3 小さな輪で続ける（招待で入ったチャンネル）。
 *
 * LP の狭い幅でも投稿の本文が読めるよう、スマートフォンの幅（1 列の表示）で、
 * 2 倍の解像度で撮る。開発者モードは無効のまま撮る。
 */

const VIEWPORT = { width: 430, height: 760 };
const THEME = 'dark' as const;

const COPY = {
  ja: {
    openControlCenter: /コントロールセンターを開く/,
    topicName: 'トピック名',
    addTopic: 'トピックを追加',
    columnPage: (position: number) => new RegExp(`${position}番目のカラムへ移動`),
    searchPlaceholder: 'インデックスを検索',
    searchSubmit: '結果を表示',
    reply: '返信',
    writeReply: '返信を書く',
    channelEntry: 'チャンネル作成・参加',
    channelDialog: 'プライベートチャンネル作成 / 参加',
    inviteToken: 'プライベートチャンネルの招待・参加権限・共有トークンを貼り付け',
    join: '参加',
    closeDialog: 'ダイアログを閉じる',
  },
  en: {
    openControlCenter: /Open Control Center/,
    topicName: 'Topic name',
    addTopic: 'Add Topic',
    columnPage: (position: number) => new RegExp(`Go to Column ${position} of`),
    searchPlaceholder: 'Search the index',
    searchSubmit: 'Show results',
    reply: 'Reply',
    writeReply: 'Write a reply',
    channelEntry: 'Create or join a private channel',
    channelDialog: 'Create / Join Private Channel',
    inviteToken: 'Paste a private channel invite, mutual grant, or mutuals+ share',
    join: 'Join',
    closeDialog: 'Close dialog',
  },
} as const;

function target(sceneId: string, locale: PromoLocale): CaptureTarget {
  return { sceneId, cutId: 'c1', locale, theme: THEME, developerMode: false, viewport: VIEWPORT };
}

/**
 * 最後に開いた列。話題の追加・スレッド・チャンネルは、どれも列の並びの末尾に開く。
 * 自分の投稿はプロフィールの列にも出るので、投稿は列で絞ってから探す。
 */
function lastColumn(page: Page) {
  return page.locator('[data-column-id]').last();
}

function card(scope: Locator, text: string) {
  return scope.locator('article', { hasText: text });
}

/** 利用者と同じく、コントロールセンターの「場所」から話題を追加して開く。 */
async function addGardenTopic(page: Page, locale: PromoLocale) {
  const copy = COPY[locale];
  await page.getByRole('button', { name: copy.openControlCenter }).first().click();
  await page.getByLabel(copy.topicName, { exact: true }).fill(GARDEN_TOPIC_NAME[locale]);
  await page.getByRole('button', { name: copy.addTopic, exact: true }).click();
  await page.keyboard.press('Escape');
  // 閉じた後に戻る focus の輪を、話題の画面に残さない。
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await expect(card(lastColumn(page), GARDEN_COPY[locale].question)).toBeVisible();
}

/** 撮影の前提。実験機能の面が写り込む状態で撮らない (INVAR-3)。 */
async function assertExperimentalSurfacesHidden(page: Page) {
  await expect(page.locator('[data-column-id][aria-label^="Metaverse"]')).toHaveCount(0);
  await expect(page.locator('[data-column-id][aria-label^="Stream"]')).toHaveCount(0);
}

// スマートフォンの幅では、新しく開いた列へ smooth scroll で送られ、途中の位置で撮れてしまう。
// 視差効果を減らす設定の利用者と同じく、列の移動を即時にする。
test.use({ viewport: VIEWPORT, deviceScaleFactor: 2, reducedMotion: 'reduce' });

for (const locale of ['ja', 'en'] as const) {
  const copy = COPY[locale];
  const story = GARDEN_COPY[locale];

  test.describe(`promo lp (${locale})`, () => {
    test.beforeEach(async ({ page }) => {
      await seedDemoStory(page, { locale, theme: THEME }, createGardenSeed(locale));
    });

    test('L0: 話題のタイムライン', async ({ page, browser }) => {
      await captureSceneStep(page, browser, {
        target: target('l0-hero', locale),
        url: '/',
        anchor: (p) => p.getByRole('button', { name: copy.openControlCenter }).first(),
        prepare: async (p) => {
          await assertExperimentalSurfacesHidden(p);
          await addGardenTopic(p, locale);
          await expect(card(lastColumn(p), story.answer)).toBeVisible();
        },
        caption: null,
      });
    });

    test('L1: 検索で話題と投稿を見つける', async ({ page, browser }) => {
      await captureSceneStep(page, browser, {
        target: target('l1-find', locale),
        url: '/',
        anchor: (p) => p.getByRole('button', { name: copy.columnPage(3) }),
        prepare: async (p) => {
          await assertExperimentalSurfacesHidden(p);
          await p.getByRole('button', { name: copy.columnPage(3) }).click();
          const input = p.getByPlaceholder(copy.searchPlaceholder);
          const explore = p.locator('[data-column-id]', { has: input });
          await expect(input).toBeVisible();
          await input.fill(story.searchWord);
          await explore.getByRole('button', { name: copy.searchSubmit }).click();
          await expect(card(explore, story.question)).toBeVisible();
          await expect(card(explore, story.firstRed)).toBeVisible();
          await input.blur();
        },
        caption: null,
      });
    });

    test('L2: スレッドで返信する', async ({ page, browser }) => {
      await captureSceneStep(page, browser, {
        target: target('l2-talk', locale),
        url: '/',
        anchor: (p) => p.getByRole('button', { name: copy.openControlCenter }).first(),
        prepare: async (p) => {
          await assertExperimentalSurfacesHidden(p);
          await addGardenTopic(p, locale);
          // 質問を開いてスレッドにし、みなとの答えに返信する。
          await card(lastColumn(p), story.question).getByTestId('post-identifier-target').first().click();
          const thread = lastColumn(p);
          await expect(card(thread, story.answer)).toBeVisible();
          await card(thread, story.answer).getByRole('button', { name: copy.reply, exact: true }).click();
          const composer = p.getByPlaceholder(copy.writeReply);
          await expect(composer).toBeVisible();
          await composer.fill(story.thanks);
          await p.keyboard.press('Control+Enter');
          await expect(card(thread, story.thanks)).toBeVisible();
        },
        caption: null,
      });
    });

    test('L3: 招待で入ったチャンネルで話す', async ({ page, browser }) => {
      await captureSceneStep(page, browser, {
        target: target('l3-circle', locale),
        url: '/',
        anchor: (p) => p.getByRole('button', { name: copy.openControlCenter }).first(),
        prepare: async (p) => {
          await assertExperimentalSurfacesHidden(p);
          await addGardenTopic(p, locale);
          await lastColumn(p).getByRole('button', { name: copy.channelEntry }).click();
          const dialog = p.getByRole('dialog', { name: copy.channelDialog });
          await dialog.getByPlaceholder(copy.inviteToken).fill('promo-garden-invite');
          await dialog.getByRole('button', { name: copy.join, exact: true }).click();
          await dialog.getByRole('button', { name: copy.closeDialog }).click();
          await expect(dialog).toBeHidden();
          for (const message of story.channel) {
            await expect(card(lastColumn(p), message)).toBeVisible();
          }
        },
        caption: null,
      });
    });
  });
}

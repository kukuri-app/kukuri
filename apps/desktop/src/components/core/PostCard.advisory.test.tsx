import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';

import i18n from '@/i18n';

import { PostCard } from './PostCard';
import { createView } from './PostCard.testHelpers';
import type { ContentAdvisoryView, PostCardView, PostMediaView } from './types';

// #1108: advisory 付き投稿(成人向け表示 OFF)の代替表示。一覧には枠と短いラベルだけを出し、
// 説明・推定の詳細・異議申し立ては詳細 dialog に置く。

const OBJECT_ID = 'post-1';

const advisory: ContentAdvisoryView = {
  issuerNodeId: 'd'.repeat(64),
  nodeBaseUrl: 'https://index-a.example',
  nodeName: 'index-a.example',
  category: 'nsfw',
  label: 'adult',
  confidence: 84,
  basis: 'classifier_score',
  signalId: 'signal-1',
  subjectKind: 'blob_cid',
  subjectId: 'b'.repeat(64),
};

function gatedMedia(kind: 'image' | 'video'): PostMediaView {
  return {
    objectId: OBJECT_ID,
    kind,
    extraAttachmentCount: 0,
    state: 'gated',
    gatedBy: 'advisory',
    metaMime: kind === 'image' ? 'image/png' : 'video/mp4',
    metaBytesLabel: '2.0 KB',
    imageGalleryItems: [],
    videoPosterPreviewSrc: null,
    videoPlaybackSrc: null,
    videoUnsupportedOnClient: false,
  };
}

function advisoryView(overrides?: Partial<PostCardView>): PostCardView {
  return createView({
    adultContentGated: true,
    gatedBy: 'advisory',
    contentAdvisory: advisory,
    media: gatedMedia('image'),
    ...overrides,
  });
}

function renderCard(view: PostCardView, onSubmitReport = vi.fn()) {
  return render(
    <PostCard
      view={view}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={onSubmitReport}
    />
  );
}

afterEach(async () => {
  await i18n.changeLanguage('en');
});

// AC-1: 一覧上は枠と短いラベルだけ。説明文・推定の詳細・申し立ては出さない。
test.each([
  ['en', 'Adult image: click for details', 'Adult video: click for details'],
  ['ja', '成人向け画像: 詳細はクリック', '成人向け動画: 詳細はクリック'],
  ['zh-CN', '成人图片：点击查看详情', '成人视频：点击查看详情'],
])('the list shows only the frame and a short label (%s)', async (locale, imageLabel, videoLabel) => {
  await i18n.changeLanguage(locale);
  const { rerender } = renderCard(advisoryView());

  const frame = screen.getByTestId(`media-adult-gated-${OBJECT_ID}`);
  expect(frame).toHaveAccessibleName(imageLabel);
  expect(frame).toHaveAttribute('aria-haspopup', 'dialog');
  expect(screen.queryByTestId(`post-advisory-gated-${OBJECT_ID}`)).not.toBeInTheDocument();
  expect(screen.queryByTestId(`post-advisory-appeal-${OBJECT_ID}`)).not.toBeInTheDocument();
  expect(screen.queryByTestId(`post-adult-gated-${OBJECT_ID}`)).not.toBeInTheDocument();
  expect(screen.queryByTestId(`post-advisory-details-trigger-${OBJECT_ID}`)).not.toBeInTheDocument();
  expect(screen.queryByText('hello')).not.toBeInTheDocument();
  // 投稿カードの公開範囲 icon(#1345)以外に、gated media の画像を描画しない。
  expect(
    screen.queryAllByRole('img').filter((element) => !element.classList.contains('post-meta-icon'))
  ).toHaveLength(0);

  rerender(
    <PostCard
      view={advisoryView({ media: gatedMedia('video') })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={vi.fn()}
    />
  );
  expect(screen.getByTestId(`media-adult-gated-${OBJECT_ID}`)).toHaveAccessibleName(videoLabel);
});

// AC-2 / AC-3 / INVAR-1: クリックで詳細 dialog を開く。推定の出所の説明を欠かさず、メディアは描画しない。
// 閉じると枠へ focus が戻る。
test('clicking the frame opens the details dialog and closing returns focus', async () => {
  const user = userEvent.setup();
  renderCard(advisoryView());

  const frame = screen.getByTestId(`media-adult-gated-${OBJECT_ID}`);
  await user.click(frame);

  const dialog = await screen.findByRole('dialog', { name: 'Community Node estimate' });
  expect(dialog).toHaveAccessibleDescription(
    'A Community Node you configured estimates this post may contain adult material. Enable adult material display in Settings to show it.'
  );
  const notice = within(dialog).getByTestId(`post-advisory-gated-${OBJECT_ID}`);
  expect(notice).toHaveTextContent(
    'This is an estimate by index-a.example. It is neither a label from the person who posted it nor a decision by the kukuri network as a whole.'
  );
  expect(within(dialog).getByTestId(`post-advisory-issuer-${OBJECT_ID}`)).toHaveTextContent(
    'index-a.exampledddddddd…dddddddd'
  );
  expect(notice).toHaveTextContent('Possible sexual content');
  expect(notice).toHaveTextContent('84 out of 100');
  expect(notice).toHaveTextContent('Automated classifier score');
  expect(within(dialog).getByRole('button', { name: 'Dispute this estimate' })).toBeInTheDocument();
  expect(within(dialog).queryByRole('img')).not.toBeInTheDocument();
  expect(document.querySelector('img, video')).toBeNull();

  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(frame).toHaveFocus();
});

// AC-2: keyboard(Enter / Space)でも開ける。
test.each([['{Enter}'], [' ']])('the frame opens the dialog from the keyboard (%s)', async (key) => {
  const user = userEvent.setup();
  renderCard(advisoryView());

  const frame = screen.getByTestId(`media-adult-gated-${OBJECT_ID}`);
  frame.focus();
  await user.keyboard(key);

  const dialog = await screen.findByRole('dialog', { name: 'Community Node estimate' });
  // 開いた直後の Enter で申し立てへ進まないよう、focus は見出しに置く。
  await waitFor(() =>
    expect(within(dialog).getByRole('heading', { name: 'Community Node estimate' })).toHaveFocus()
  );
  await user.keyboard('{Enter}');
  expect(screen.queryByRole('dialog', { name: 'Appeal a risk assessment' })).not.toBeInTheDocument();
  await user.click(within(dialog).getByRole('button', { name: 'Close dialog' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(frame).toHaveFocus();
});

// AC-2: 異議申し立てへ移ると詳細 dialog を閉じて通報 dialog を開き、閉じた後は枠へ focus が戻る。
test('the appeal replaces the details dialog and returns focus to the frame afterwards', async () => {
  const user = userEvent.setup();
  renderCard(advisoryView());

  const frame = screen.getByTestId(`media-adult-gated-${OBJECT_ID}`);
  await user.click(frame);
  await user.click(await screen.findByTestId(`post-advisory-appeal-${OBJECT_ID}`));

  const appealDialog = await screen.findByRole('dialog', { name: 'Appeal a risk assessment' });
  await waitFor(() =>
    expect(screen.queryByTestId(`post-advisory-dialog-${OBJECT_ID}`)).not.toBeInTheDocument()
  );
  expect(appealDialog).toBeInTheDocument();

  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(frame).toHaveFocus();
});

// 通報操作が出せない文脈では、詳細 dialog に申し立ての導線を出さない。
test('the dialog omits the appeal when reporting is unavailable', async () => {
  const user = userEvent.setup();
  render(
    <PostCard
      view={advisoryView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
    />
  );

  await user.click(screen.getByTestId(`media-adult-gated-${OBJECT_ID}`));
  const dialog = await screen.findByRole('dialog', { name: 'Community Node estimate' });
  expect(within(dialog).getByTestId(`post-advisory-gated-${OBJECT_ID}`)).toBeInTheDocument();
  expect(within(dialog).queryByTestId(`post-advisory-appeal-${OBJECT_ID}`)).not.toBeInTheDocument();
});

// AC-4: メディア枠が無い advisory 投稿は、本文欄に詳細を開く操作だけを出す。
test('a text-only advisory post shows a compact trigger instead of the explanation', async () => {
  const user = userEvent.setup();
  await i18n.changeLanguage('ja');
  renderCard(
    advisoryView({
      media: { ...gatedMedia('image'), kind: null, state: 'ready', gatedBy: undefined },
      contentAdvisory: { ...advisory, subjectKind: 'post_id', subjectId: OBJECT_ID },
    })
  );

  expect(screen.queryByTestId(`media-adult-gated-${OBJECT_ID}`)).not.toBeInTheDocument();
  expect(screen.queryByTestId(`post-advisory-gated-${OBJECT_ID}`)).not.toBeInTheDocument();
  const trigger = screen.getByTestId(`post-advisory-details-trigger-${OBJECT_ID}`);
  expect(trigger).toHaveAccessibleName('成人向けの投稿: 詳細はクリック');

  await user.click(trigger);
  const dialog = await screen.findByRole('dialog', { name: 'コミュニティノードによる推定' });
  expect(within(dialog).getByTestId(`post-advisory-gated-${OBJECT_ID}`)).toHaveTextContent(
    '投稿した人自身の申告でも'
  );
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(trigger).toHaveFocus();
});

// AC-4: canonical 解決前(本文が待機文言)の結果は、待機文言を保ったまま詳細を開く操作を出す。
test('an unresolved advisory result keeps its waiting text next to the trigger', () => {
  renderCard(
    advisoryView({
      media: { ...gatedMedia('image'), kind: null, state: 'ready', gatedBy: undefined },
      gatedBodyText: 'Resolving the post…',
    })
  );

  expect(screen.getByTestId(`post-adult-gated-${OBJECT_ID}`)).toHaveTextContent('Resolving the post…');
  expect(screen.getByTestId(`post-advisory-details-trigger-${OBJECT_ID}`)).toBeInTheDocument();
});

// 投稿者の自己申告と node の推定が両方ある場合も、詳細 dialog の説明は自己申告の文言にする。
test('a self-labeled post with an advisory explains the self label in the dialog', async () => {
  const user = userEvent.setup();
  renderCard(advisoryView({ gatedBy: 'self_label', media: { ...gatedMedia('image'), gatedBy: 'self_label' } }));

  await user.click(screen.getByTestId(`media-adult-gated-${OBJECT_ID}`));
  expect(await screen.findByRole('dialog', { name: 'Community Node estimate' })).toHaveAccessibleDescription(
    'This post is self-labeled as adult material. Enable adult material display in Settings to show it.'
  );
});

// 自己申告だけの投稿(推定なし)は従来の代替表示のまま。詳細を開く操作は出さない。
test('self-labeled posts without an advisory keep the existing placeholder', () => {
  renderCard(
    createView({
      adultContentGated: true,
      gatedBy: 'self_label',
      media: { ...gatedMedia('image'), gatedBy: 'self_label' },
    })
  );

  const frame = screen.getByTestId(`media-adult-gated-${OBJECT_ID}`);
  expect(frame.tagName).toBe('DIV');
  expect(frame).toHaveTextContent('Adult material (self-labeled).');
  expect(screen.getByTestId(`post-adult-gated-${OBJECT_ID}`)).toHaveTextContent(
    'This post is self-labeled as adult material.'
  );
  expect(screen.queryByTestId(`post-advisory-details-trigger-${OBJECT_ID}`)).not.toBeInTheDocument();
});

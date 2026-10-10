import { useState, type FormEventHandler } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import { ComposerPanel } from './ComposerPanel';
import { type MentionCandidate } from './types';

const ALICE = 'a'.repeat(64);
const BOB = 'b'.repeat(64);
const MENTION_CANDIDATES: MentionCandidate[] = [
  { pubkey: ALICE, label: 'Alice', displayName: 'Alice', name: 'alice', about: 'Bio', picture: null },
  { pubkey: BOB, label: 'Bob', displayName: 'Bob', name: 'bob', about: null, picture: null },
];

function MentionHarness({
  candidates = MENTION_CANDIDATES,
  onSubmit = (event) => event.preventDefault(),
  submitDisabled = false,
}: {
  candidates?: MentionCandidate[];
  onSubmit?: FormEventHandler<HTMLFormElement>;
  submitDisabled?: boolean;
}) {
  const [value, setValue] = useState('');
  return (
    <ComposerPanel
      value={value}
      onChange={(event) => setValue(event.target.value)}
      onValueChange={setValue}
      mentionCandidates={candidates}
      onSubmit={onSubmit}
      submitDisabled={submitDisabled}
      attachmentInputKey={0}
      onAttachmentSelection={() => undefined}
      draftMediaItems={[]}
      onRemoveDraftAttachment={() => undefined}
      audienceLabel='Public'
      onClearReply={() => undefined}
    />
  );
}

test('Ctrl+Enter submits the composer form exactly once', async () => {
  const user = userEvent.setup();
  const onSubmit = vi.fn<FormEventHandler<HTMLFormElement>>((event) => event.preventDefault());
  render(<MentionHarness onSubmit={onSubmit} />);

  const textarea = screen.getByRole('textbox');
  await user.click(textarea);
  await user.type(textarea, 'keyboard post');
  await user.keyboard('{Control>}{Enter}{/Control}');

  expect(onSubmit).toHaveBeenCalledTimes(1);
});

test('Enter and Shift+Enter keep inserting line breaks while Alt+Enter keeps its default behavior', async () => {
  const user = userEvent.setup();
  const onSubmit = vi.fn<FormEventHandler<HTMLFormElement>>((event) => event.preventDefault());
  render(<MentionHarness onSubmit={onSubmit} />);

  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  await user.click(textarea);
  await user.type(textarea, 'first');
  await user.keyboard('{Enter}');
  await user.type(textarea, 'second');
  await user.keyboard('{Shift>}{Enter}{/Shift}');
  await user.type(textarea, 'third');

  expect(textarea.value).toBe('first\nsecond\nthird');
  expect(fireEvent.keyDown(textarea, { key: 'Enter', altKey: true })).toBe(true);
  expect(onSubmit).not.toHaveBeenCalled();
});

// #964: 日本語入力の変換中に押した Ctrl+Enter は IME の確定であり、投稿を送信しない。
test('Ctrl+Enter during IME composition does not submit', async () => {
  const user = userEvent.setup();
  const onSubmit = vi.fn<FormEventHandler<HTMLFormElement>>((event) => event.preventDefault());
  render(<MentionHarness onSubmit={onSubmit} />);

  const textarea = screen.getByRole('textbox');
  await user.click(textarea);
  await user.type(textarea, 'へんかんちゅう');
  fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true, isComposing: true });

  expect(onSubmit).not.toHaveBeenCalled();
  await user.keyboard('{Control>}{Enter}{/Control}');
  expect(onSubmit).toHaveBeenCalledTimes(1);
});

test('disabled submit blocks both Ctrl+Enter and the submit button', async () => {
  const user = userEvent.setup();
  const onSubmit = vi.fn<FormEventHandler<HTMLFormElement>>((event) => event.preventDefault());
  render(<MentionHarness onSubmit={onSubmit} submitDisabled />);

  const textarea = screen.getByRole('textbox');
  await user.click(textarea);
  await user.type(textarea, 'pending post');
  await user.keyboard('{Control>}{Enter}{/Control}');

  expect(screen.getByRole('button', { name: 'Post' })).toBeDisabled();
  expect(onSubmit).not.toHaveBeenCalled();
});

test('typing @ with a query shows matching mention candidates', async () => {
  const user = userEvent.setup();
  render(<MentionHarness />);

  await user.click(screen.getByRole('textbox'));
  await user.keyboard('@al');

  expect(screen.getByRole('listbox', { name: 'Mention suggestions' })).toBeInTheDocument();
  expect(screen.getByRole('option', { name: /Alice/ })).toBeInTheDocument();
  expect(screen.queryByRole('option', { name: /Bob/ })).not.toBeInTheDocument();
  expect(screen.queryByText(ALICE.slice(0, 12))).not.toBeInTheDocument();
});

test('right-clicking a mention candidate copies its author ID without selecting it', async () => {
  const user = userEvent.setup();
  const clipboardWriteText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
  render(<MentionHarness />);

  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  await user.click(textarea);
  await user.keyboard('@al');
  const candidate = screen.getByRole('option', { name: /Alice/ });

  fireEvent.mouseDown(candidate, { button: 2 });
  fireEvent.contextMenu(candidate, { clientX: 20, clientY: 24 });
  await user.click(screen.getByRole('menuitem', { name: 'Copy user ID' }));

  expect(clipboardWriteText).toHaveBeenCalledWith(ALICE);
  expect(textarea).toHaveValue('@al');
  expect(screen.getByRole('listbox', { name: 'Mention suggestions' })).toBeInTheDocument();

  candidate.focus();
  fireEvent.keyDown(candidate, { key: 'F10', shiftKey: true });
  await user.click(screen.getByRole('menuitem', { name: 'Copy user ID' }));

  expect(clipboardWriteText).toHaveBeenNthCalledWith(2, ALICE);
  expect(textarea).toHaveValue('@al');
});

test('selecting a candidate with the keyboard inserts the mention token', async () => {
  const user = userEvent.setup();
  render(<MentionHarness />);

  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  await user.click(textarea);
  await user.keyboard('hi @al');
  await user.keyboard('{Enter}');

  expect(textarea.value).toBe(`hi @[Alice](${ALICE}) `);
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
});

test('mention selection keeps priority over Ctrl+Enter form submission', async () => {
  const user = userEvent.setup();
  const onSubmit = vi.fn<FormEventHandler<HTMLFormElement>>((event) => event.preventDefault());
  render(<MentionHarness onSubmit={onSubmit} />);

  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  await user.click(textarea);
  await user.keyboard('hi @al');
  await user.keyboard('{Control>}{Enter}{/Control}');

  expect(textarea.value).toBe(`hi @[Alice](${ALICE}) `);
  expect(onSubmit).not.toHaveBeenCalled();
});

test('Tab keeps selecting a mention candidate without submitting', async () => {
  const user = userEvent.setup();
  const onSubmit = vi.fn<FormEventHandler<HTMLFormElement>>((event) => event.preventDefault());
  render(<MentionHarness onSubmit={onSubmit} />);

  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
  await user.click(textarea);
  await user.keyboard('hi @al');
  await user.keyboard('{Tab}');

  expect(textarea.value).toBe(`hi @[Alice](${ALICE}) `);
  expect(onSubmit).not.toHaveBeenCalled();
});

test('Escape closes the suggestion list', async () => {
  const user = userEvent.setup();
  render(<MentionHarness />);

  await user.click(screen.getByRole('textbox'));
  await user.keyboard('@al');
  expect(screen.getByRole('listbox')).toBeInTheDocument();

  await user.keyboard('{Escape}');
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
});

test('a bare @ lists all candidates', async () => {
  const user = userEvent.setup();
  render(<MentionHarness />);

  await user.click(screen.getByRole('textbox'));
  await user.keyboard('@');

  expect(screen.getByRole('option', { name: /Alice/ })).toBeInTheDocument();
  expect(screen.getByRole('option', { name: /Bob/ })).toBeInTheDocument();
});

test('mention autocomplete stays inert without onValueChange', async () => {
  const user = userEvent.setup();
  render(
    <ComposerPanel
      value='@al'
      onChange={() => undefined}
      mentionCandidates={MENTION_CANDIDATES}
      onSubmit={(event) => event.preventDefault()}
      attachmentInputKey={0}
      onAttachmentSelection={() => undefined}
      draftMediaItems={[]}
      onRemoveDraftAttachment={() => undefined}
      audienceLabel='Public'
      onClearReply={() => undefined}
    />
  );

  await user.click(screen.getByRole('textbox'));
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
});

test('reply banner keeps the target summary and a compact clear icon action', () => {
  render(
    <ComposerPanel
      value=''
      onChange={() => undefined}
      onSubmit={(event) => event.preventDefault()}
      attachmentInputKey={0}
      onAttachmentSelection={() => undefined}
      draftMediaItems={[]}
      onRemoveDraftAttachment={() => undefined}
      composerError={null}
      audienceLabel='Public'
      replyTarget={{ content: 'reply target body', audienceLabel: 'Imported' }}
      sourcePreview={{
        post: {
          object_id: 'post-1',
          envelope_id: 'envelope-post-1',
          author_pubkey: 'a'.repeat(64),
          author_name: 'alice',
          author_display_name: 'Alice',
          following: false,
          followed_by: false,
          mutual: false,
          friend_of_friend: false,
          object_kind: 'post',
          content: 'reply target body',
          content_status: 'Available',
          attachments: [],
          created_at: 1,
          reply_to: null,
          root_id: 'post-1',
          published_topic_id: null,
          origin_topic_id: null,
          repost_of: null,
          repost_commentary: null,
          is_threadable: true,
          channel_id: null,
          audience_label: 'Imported',
          reaction_summary: [],
          my_reactions: [],
        },
        context: 'timeline',
        authorLabel: 'Alice',
        authorPicture: null,
        relationshipLabel: null,
        audience: { kind: 'private', channelLabel: 'Imported' },
        threadTargetId: 'post-1',
        media: {
          objectId: 'post-1',
          kind: null,
          statusLabel: null,
          extraAttachmentCount: 0,
          state: 'ready',
          metaMime: null,
          metaBytesLabel: null,
          videoPosterPreviewSrc: null,
          videoPlaybackSrc: null,
          videoUnsupportedOnClient: false,
        },
      }}
      onClearReply={vi.fn()}
    />
  );

  expect(screen.getByText('Replying')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Clear reply' })).toHaveClass('shell-icon-button');
  expect(screen.queryByRole('button', { name: 'Clear' })).not.toBeInTheDocument();
  expect(screen.getByText('Original post')).toBeInTheDocument();
  expect(screen.getAllByText('reply target body').length).toBeGreaterThan(0);
  expect(screen.getAllByText('Imported').length).toBeGreaterThan(0);
});

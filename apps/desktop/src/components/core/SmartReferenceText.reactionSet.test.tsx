import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import { ReactionSetText, SmartReferenceText } from './SmartReferenceText';

const HASH = 'a'.repeat(64);

// #1232 AC-4: 投稿の本文のセットの共有用の文字列は「取り込む」ボタンになる。
test('a reaction set link in a post activates the import with its set hash', async () => {
  const onActivateReference = vi.fn();
  render(
    <SmartReferenceText
      text={`try kukuri:reaction-set:${HASH}`}
      onActivateReference={onActivateReference}
    />
  );

  await userEvent.setup().click(screen.getByRole('button', { name: 'Import reaction set' }));
  expect(onActivateReference).toHaveBeenCalledWith({ kind: 'reaction_set', setHash: HASH });
});

// DM の本文は、セットの共有用の文字列だけをボタンにし、他の参照や改行は文字のまま示す。
test('a direct message turns only reaction set links into import buttons', async () => {
  const onImport = vi.fn(async () => undefined);
  const { container } = render(
    <ReactionSetText
      text={`see kukuri:topic:general\nkukuri:reaction-set:${HASH} please`}
      onImport={onImport}
    />
  );

  expect(screen.getAllByRole('button')).toHaveLength(1);
  expect(container.textContent).toBe('see kukuri:topic:general\nImport reaction set please');
  await userEvent.setup().click(screen.getByRole('button', { name: 'Import reaction set' }));
  expect(onImport).toHaveBeenCalledWith(HASH);
});

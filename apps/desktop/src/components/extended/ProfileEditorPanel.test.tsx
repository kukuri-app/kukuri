import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import { ProfileEditorPanel } from './ProfileEditorPanel';

const PUBKEY = '79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798';

const props = {
  authorLabel: 'Alice', status: 'ready' as const, saving: false, dirty: true, error: null,
  hasPicture: false, pictureInputKey: 0, onFieldChange: vi.fn(), onPictureSelect: vi.fn(),
  onPictureClear: vi.fn(), onSave: vi.fn(), onReset: vi.fn(),
};
const fields = (nip05: string) => ({ displayName: 'Alice', name: 'alice', about: '', nip05 });

afterEach(() => vi.unstubAllGlobals());

// #1670 AC-1.3: 入力中の名前と自分の公開鍵で、ドメインに置く nostr.json の内容を示してコピーできる。
test('shows the nostr.json content for the typed identifier and copies it', () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } });
  render(<ProfileEditorPanel {...props} fields={fields(' Alice@Example.com ')} localPubkey={PUBKEY} />);

  const document = `{"names":{"alice":"${PUBKEY}"}}`;
  expect(screen.getByRole('textbox', { name: /^Domain verification/ })).toHaveValue(' Alice@Example.com ');
  expect(screen.getByText(/on example\.com returns the following, @example\.com appears/)).toBeInTheDocument();
  expect(screen.getByText(document)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Copy content' }));
  expect(writeText).toHaveBeenCalledWith(document);
  expect(screen.getByRole('button', { name: 'Save Profile' })).toBeEnabled();
});

// #1670 AC-1.2: 形に合わない識別子は保存の前に理由を出し、保存できない。空なら欄の無い profile として保存できる。
test.each(['alice@localhost', 'a b@example.com', 'alice@192.0.2.1'])(
  'blocks saving an invalid identifier %s',
  (value) => {
    render(<ProfileEditorPanel {...props} fields={fields(value)} localPubkey={PUBKEY} />);
    expect(screen.getByText(/Enter it as name@example\.com/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save Profile' })).toBeDisabled();
  }
);

test('saves without an identifier and hides the field without the own public key', () => {
  const { rerender } = render(<ProfileEditorPanel {...props} fields={fields('  ')} localPubkey={PUBKEY} />);
  expect(screen.queryByText(/Enter it as name@example\.com/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Save Profile' })).toBeEnabled();

  rerender(<ProfileEditorPanel {...props} fields={fields('')} />);
  expect(screen.queryByRole('textbox', { name: /^Domain verification/ })).not.toBeInTheDocument();
});

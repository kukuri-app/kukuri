import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import i18n from '@/i18n';
import type { AccountTransferStatus } from '@/lib/api/types.generated';

import { AccountTransferPanel } from './AccountTransferPanel';

const identityApi = vi.hoisted(() => ({
  createAccountTransferInvite: vi.fn(),
  openAccountTransfer: vi.fn(),
  getAccountTransferStatus: vi.fn(),
  decideAccountTransfer: vi.fn(),
  cancelAccountTransfer: vi.fn(),
}));

vi.mock('@/lib/api/identity', () => identityApi);

const LINK = 'kukuri://transfer#v1.ZXhhbXBsZQ';
let status: AccountTransferStatus;

beforeEach(() => {
  status = { state: 'idle' };
  identityApi.getAccountTransferStatus.mockImplementation(async () => status);
  identityApi.cancelAccountTransfer.mockResolvedValue(undefined);
  identityApi.decideAccountTransfer.mockResolvedValue(undefined);
  identityApi.openAccountTransfer.mockResolvedValue(undefined);
  identityApi.createAccountTransferInvite.mockImplementation(async () => {
    status = { state: 'waiting', expires_at_ms: Date.now() + 5 * 60 * 1000 };
    return { link: LINK, expires_at_ms: status.expires_at_ms };
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

test('the source shows the QR code and link, then confirms the matching code', async () => {
  const user = userEvent.setup();
  const { unmount } = render(<AccountTransferPanel role='source' />);
  expect(await screen.findByRole('img', { name: 'QR code of the transfer link' })).toBeInTheDocument();
  expect(screen.getByLabelText('Transfer link')).toHaveValue(LINK);
  expect(screen.getByRole('timer')).toHaveTextContent(/[45]:\d\d left/);
  expect(identityApi.createAccountTransferInvite).toHaveBeenCalledTimes(1);

  status = { state: 'confirming', role: 'source', code: '482915', local_accepted: false };
  expect(await screen.findByText('482 915', {}, { timeout: 2000 })).toBeInTheDocument();
  identityApi.decideAccountTransfer.mockImplementation(async () => {
    status = { state: 'confirming', role: 'source', code: '482915', local_accepted: true };
  });
  await user.click(screen.getByRole('button', { name: 'Codes match' }));
  expect(identityApi.decideAccountTransfer).toHaveBeenCalledWith(true);
  expect(await screen.findByText('Waiting for the other device to confirm…')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Codes match' })).not.toBeInTheDocument();

  status = { state: 'transferring', role: 'source', items: 5 };
  expect(await screen.findByText('Sending keys and settings (5)…', {}, { timeout: 2000 })).toBeInTheDocument();
  status = { state: 'completed', role: 'source', account_id: null, history: null };
  expect(await screen.findByText(/The other device has saved the account/, {}, { timeout: 2000 })).toBeInTheDocument();
  expect(screen.queryByTestId('account-transfer-history-result')).not.toBeInTheDocument();
  unmount();
  expect(identityApi.cancelAccountTransfer).toHaveBeenCalled();
});

test('the target hands over the received account when asked and explains a storage failure', async () => {
  const user = userEvent.setup();
  const onCompleted = vi.fn();
  const { unmount } = render(<AccountTransferPanel role='target' initialLink={LINK} onCompleted={onCompleted} />);
  identityApi.openAccountTransfer.mockImplementation(async () => {
    status = { state: 'transferring', role: 'target', items: 64 };
  });
  await user.click(screen.getByRole('button', { name: 'Connect' }));
  expect(await screen.findByText('Receiving and saving keys and settings (64)…', {}, { timeout: 2000 })).toBeInTheDocument();
  expect(onCompleted).not.toHaveBeenCalled();
  status = { state: 'completed', role: 'target', account_id: 'cccccccccccccccc', history: null };
  expect(await screen.findByText(/Everything was received/, {}, { timeout: 2000 })).toBeInTheDocument();
  // AC-5: 完了の画面の説明を読めるように、押すまでは渡さない。
  await new Promise((resolve) => setTimeout(resolve, 700));
  expect(onCompleted).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Use this account' }));
  expect(onCompleted).toHaveBeenCalledExactlyOnceWith('cccccccccccccccc');
  unmount();

  render(<AccountTransferPanel role='target' initialLink={LINK} onCompleted={onCompleted} />);
  identityApi.openAccountTransfer.mockImplementation(async () => {
    status = { state: 'failed', role: 'target', reason: 'storage' };
  });
  await user.click(screen.getByRole('button', { name: 'Connect' }));
  expect(await screen.findByText(/couldn't save the account/, {}, { timeout: 2000 })).toBeInTheDocument();
  expect(onCompleted).toHaveBeenCalledTimes(1);
});

test('the target pastes the link, connects and can reject a mismatching code', async () => {
  const user = userEvent.setup();
  render(<AccountTransferPanel role='target' />);
  const connect = screen.getByRole('button', { name: 'Connect' });
  expect(connect).toBeDisabled();
  await user.type(screen.getByRole('textbox', { name: /^Transfer link/ }), LINK);
  identityApi.openAccountTransfer.mockImplementation(async () => {
    status = { state: 'confirming', role: 'target', code: '000123', local_accepted: false };
  });
  await user.click(connect);
  expect(identityApi.openAccountTransfer).toHaveBeenCalledWith(LINK, null);
  expect(await screen.findByText('000 123', {}, { timeout: 2000 })).toBeInTheDocument();

  identityApi.decideAccountTransfer.mockImplementation(async () => {
    status = { state: 'failed', role: 'target', reason: 'rejected' };
  });
  await user.click(screen.getByRole('button', { name: "Codes don't match" }));
  expect(identityApi.decideAccountTransfer).toHaveBeenCalledWith(false);
  expect(await screen.findByText("The confirmation codes didn't match, so the transfer was stopped.")).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(screen.getByRole('button', { name: 'Connect' })).toBeInTheDocument();
});

test('an unusable link and a failed connection are explained', async () => {
  const user = userEvent.setup();
  identityApi.openAccountTransfer.mockRejectedValueOnce(new Error('account transfer link expired'));
  render(<AccountTransferPanel role='target' initialLink={LINK} />);
  await user.click(screen.getByRole('button', { name: 'Connect' }));
  const invalid = /This link can't be used/;
  expect(await screen.findByText(invalid)).toBeInTheDocument();

  identityApi.openAccountTransfer.mockImplementation(async () => {
    status = { state: 'connecting' };
  });
  await user.click(screen.getByRole('button', { name: 'Connect' }));
  expect(await screen.findByText('Connecting to the other device…')).toBeInTheDocument();
  await act(async () => {
    status = { state: 'failed', role: 'target', reason: 'unreachable' };
  });
  await waitFor(() => expect(screen.getByText(/Couldn't connect to the other device/)).toBeInTheDocument(), { timeout: 2000 });
});

test('the source can prepare a new link after a failure', async () => {
  const user = userEvent.setup();
  identityApi.createAccountTransferInvite.mockRejectedValueOnce(new Error('missing active iroh stack'));
  render(<AccountTransferPanel role='source' />);
  expect(await screen.findByText("Couldn't prepare the link. Please try again.")).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByLabelText('Transfer link')).toHaveValue(LINK);
});

test('the source still shows a link after the development re-mount cancels the first one', async () => {
  identityApi.cancelAccountTransfer.mockImplementation(async () => { status = { state: 'idle' }; });
  render(<StrictMode><AccountTransferPanel role='source' /></StrictMode>);
  expect(await screen.findByLabelText('Transfer link')).toHaveValue(LINK);
  await new Promise((resolve) => setTimeout(resolve, 1200));
  expect(screen.getByLabelText('Transfer link')).toHaveValue(LINK);
  expect(identityApi.createAccountTransferInvite).toHaveBeenCalledTimes(2);
});

// #1211 AC-3（3g）: 移行先は接続の前に履歴の範囲を選ぶ（既定は移さない）。履歴の間は進捗と「やめる」を出し、完了は
// 履歴が終わってから（その時点で受け取ったアカウントを渡す）。要約は移行元に無かった数と、途中で止まったときの続きを示す。
test('the target chooses a history range and is completed only after the history', async () => {
  const user = userEvent.setup();
  const onCompleted = vi.fn();
  render(<AccountTransferPanel role='target' initialLink={LINK} onCompleted={onCompleted} />);
  const range = screen.getByRole('combobox', { name: /^Post history/ });
  expect(range).toHaveValue('none');
  expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual(["Don't move", 'Last 30 days', 'Last year', 'All']);
  await user.selectOptions(range, 'month');
  identityApi.openAccountTransfer.mockImplementation(async () => {
    status = { state: 'history', role: 'target', account_id: 'cccccccccccccccc', posts: 12, unavailable: 0 };
  });
  await user.click(screen.getByRole('button', { name: 'Connect' }));
  expect(identityApi.openAccountTransfer).toHaveBeenCalledWith(LINK, 'month');
  expect(await screen.findByText('Receiving post history (12)…', {}, { timeout: 2000 })).toBeInTheDocument();
  expect(screen.getByText('Keys and settings have been moved.')).toBeInTheDocument();
  await new Promise((resolve) => setTimeout(resolve, 700));
  expect(onCompleted).not.toHaveBeenCalled();

  identityApi.cancelAccountTransfer.mockImplementation(async () => {
    status = { state: 'completed', role: 'target', account_id: 'cccccccccccccccc', history: { posts: 12, unavailable: 2, stopped: 'cancelled' } };
  });
  await user.click(screen.getByRole('button', { name: 'Stop receiving history' }));
  expect(await screen.findByText('Receiving post history stopped partway (12).')).toBeInTheDocument();
  expect(screen.getByText("Texts or attachments that weren't on the other device couldn't be moved (2).")).toBeInTheDocument();
  expect(screen.getByText(/choose the same range on the receiving device to continue/)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Use this account' }));
  expect(onCompleted).toHaveBeenCalledExactlyOnceWith('cccccccccccccccc');
});

test('the source shows the history it sends and the finished summary', async () => {
  render(<AccountTransferPanel role='source' />);
  expect(await screen.findByLabelText('Transfer link')).toHaveValue(LINK);
  status = { state: 'history', role: 'source', account_id: null, posts: 3, unavailable: 0 };
  expect(await screen.findByText('Sending post history (3)…', {}, { timeout: 2000 })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Stop sending history' })).toBeEnabled();
  status = { state: 'completed', role: 'source', account_id: null, history: { posts: 40, unavailable: 0, stopped: null } };
  expect(await screen.findByText('Sent post history (40).', {}, { timeout: 2000 })).toBeInTheDocument();
  expect(screen.queryByText(/couldn't be moved/)).not.toBeInTheDocument();
  expect(screen.queryByText(/continue where it stopped/)).not.toBeInTheDocument();
});

// #1211 AC-5: 接続の前に移るもの・移らないもの・移行元に残ることを示し、移行先の完了の後に移っていないものを示す。
test('both devices explain what moves before connecting and the target lists what was not moved', async () => {
  const { unmount } = render(<AccountTransferPanel role='source' />);
  await screen.findByRole('img', { name: 'QR code of the transfer link' });
  const sourceScope = screen.getByTestId('account-transfer-scope');
  expect(sourceScope).toHaveTextContent('Moves: your account key, profile, follows and blocks');
  expect(sourceScope).toHaveTextContent("Doesn't move: your muted list, DM and notification history");
  expect(sourceScope).toHaveTextContent('The account stays on the source device.');
  status = { state: 'completed', role: 'source', account_id: null, history: null };
  await screen.findByText(/The other device has saved the account/, {}, { timeout: 2000 });
  expect(screen.queryByTestId('account-transfer-scope')).not.toBeInTheDocument();
  unmount();

  const user = userEvent.setup();
  render(<AccountTransferPanel role='target' initialLink={LINK} />);
  expect(screen.getByTestId('account-transfer-scope')).toHaveTextContent('Moves: your account key');
  identityApi.openAccountTransfer.mockImplementation(async () => {
    status = { state: 'completed', role: 'target', account_id: 'cccccccccccccccc', history: null };
  });
  await user.click(screen.getByRole('button', { name: 'Connect' }));
  await screen.findByText(/Everything was received/, {}, { timeout: 2000 });
  const done = screen.getByTestId('account-transfer-scope');
  expect(done).toHaveTextContent('Not moved: your muted list, DM and notification history');
  expect(done).not.toHaveTextContent('Moves:');
  expect(screen.getByRole('button', { name: 'Use this account' })).toBeVisible();
});

test.each([
  ['ja', '移るもの:', '移らないもの:', '移っていないもの:', 'このアカウントを使う', '移行元の端末のアカウントは、そのまま残ります。'],
  ['zh-CN', '会迁移：', '不会迁移：', '未迁移：', '使用此账号', '原设备上的账号会保留。'],
])('the transfer screen explains what moves in %s', async (language, moves, notMoved, notMovedDone, use, kept) => {
  await i18n.changeLanguage(language);
  try {
    const { unmount } = render(<AccountTransferPanel role='source' />);
    await screen.findByRole('img', { name: i18n.t('settings:accountTransfer.source.qrLabel') });
    const sourceScope = screen.getByTestId('account-transfer-scope');
    expect(sourceScope).toHaveTextContent(moves);
    expect(sourceScope).toHaveTextContent(kept);
    unmount();

    const user = userEvent.setup();
    render(<AccountTransferPanel role='target' initialLink={LINK} />);
    const scope = screen.getByTestId('account-transfer-scope');
    expect(scope).toHaveTextContent(moves);
    expect(scope).toHaveTextContent(notMoved);
    identityApi.openAccountTransfer.mockImplementation(async () => {
      status = { state: 'completed', role: 'target', account_id: 'cccccccccccccccc', history: null };
    });
    await user.click(screen.getByRole('button', { name: i18n.t('settings:accountTransfer.target.connect') }));
    expect(await screen.findByRole('button', { name: use }, { timeout: 2000 })).toBeVisible();
    expect(screen.getByTestId('account-transfer-scope')).toHaveTextContent(notMovedDone);
  } finally {
    await i18n.changeLanguage('en');
  }
});

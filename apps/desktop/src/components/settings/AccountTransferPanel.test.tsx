import { StrictMode } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

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
  status = { state: 'completed', role: 'source', account_id: null };
  expect(await screen.findByText(/The other device has saved the account/, {}, { timeout: 2000 })).toBeInTheDocument();
  unmount();
  expect(identityApi.cancelAccountTransfer).toHaveBeenCalled();
});

test('the target reports the received account once and explains a storage failure', async () => {
  const user = userEvent.setup();
  const onCompleted = vi.fn();
  const { unmount } = render(<AccountTransferPanel role='target' initialLink={LINK} onCompleted={onCompleted} />);
  identityApi.openAccountTransfer.mockImplementation(async () => {
    status = { state: 'transferring', role: 'target', items: 64 };
  });
  await user.click(screen.getByRole('button', { name: 'Connect' }));
  expect(await screen.findByText('Receiving and saving keys and settings (64)…', {}, { timeout: 2000 })).toBeInTheDocument();
  expect(onCompleted).not.toHaveBeenCalled();
  status = { state: 'completed', role: 'target', account_id: 'cccccccccccccccc' };
  expect(await screen.findByText(/Everything was received/, {}, { timeout: 2000 })).toBeInTheDocument();
  await new Promise((resolve) => setTimeout(resolve, 700));
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
  expect(identityApi.openAccountTransfer).toHaveBeenCalledWith(LINK);
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

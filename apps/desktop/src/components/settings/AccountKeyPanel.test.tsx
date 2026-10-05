import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import type {
  AccountKeyImportPreview,
  AccountsSnapshot,
} from '@/lib/api/types.generated';

import i18n from '@/i18n';

import { AccountKeyPanel } from './AccountKeyPanel';

const identityApi = vi.hoisted(() => ({
  listAccounts: vi.fn(),
  exportAccountKey: vi.fn(),
  previewAccountKeyImport: vi.fn(),
  importAccountKey: vi.fn(),
  switchAccount: vi.fn(),
}));

vi.mock('@/lib/api/identity', () => identityApi);

const ACTIVE_PUBKEY = 'aa'.repeat(32);
const OTHER_PUBKEY = 'bb'.repeat(32);
const SECRET_HEX = 'cc'.repeat(32);

const snapshot: AccountsSnapshot = {
  active_account_id: ACTIVE_PUBKEY.slice(0, 16),
  accounts: [
    {
      id: ACTIVE_PUBKEY.slice(0, 16),
      pubkey: ACTIVE_PUBKEY,
      label: 'main',
      created_at: 1,
      last_used_at: 2,
    },
    {
      id: OTHER_PUBKEY.slice(0, 16),
      pubkey: OTHER_PUBKEY,
      label: null,
      created_at: 3,
      last_used_at: 4,
    },
  ],
};

beforeEach(() => {
  identityApi.listAccounts.mockResolvedValue(snapshot);
  identityApi.exportAccountKey.mockResolvedValue({
    export: 'kukuri-account-key.v1.ZW5jcnlwdGVk',
    public_key: ACTIVE_PUBKEY,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

test('lists accounts with fingerprints and marks the active one', async () => {
  render(<AccountKeyPanel />);

  expect(await screen.findByText(ACTIVE_PUBKEY)).toBeInTheDocument();
  expect(screen.getByText(OTHER_PUBKEY)).toBeInTheDocument();
  expect(screen.getByText('Active')).toBeInTheDocument();
  // アクティブアカウントには切替ボタンが出ない。
  expect(
    screen.queryByTestId(`switch-account-${ACTIVE_PUBKEY.slice(0, 16)}`)
  ).not.toBeInTheDocument();
  expect(screen.getByTestId(`switch-account-${OTHER_PUBKEY.slice(0, 16)}`)).toBeInTheDocument();
});

test('export requires acknowledging the warning and a matching strong passphrase', async () => {
  const user = userEvent.setup();
  render(<AccountKeyPanel />);
  await screen.findByText(ACTIVE_PUBKEY);

  expect(
    screen.getByText(/Anyone who obtains this export and its passphrase/)
  ).toBeInTheDocument();
  expect(screen.getByTestId('export-passphrase')).toBeDisabled();
  expect(screen.getByTestId('export-submit')).toBeDisabled();

  await user.click(screen.getByTestId('export-acknowledge'));
  await user.type(screen.getByTestId('export-passphrase'), 'short');
  await user.type(screen.getByTestId('export-passphrase-confirm'), 'short');
  expect(screen.getByTestId('export-submit')).toBeDisabled();

  await user.clear(screen.getByTestId('export-passphrase'));
  await user.clear(screen.getByTestId('export-passphrase-confirm'));
  await user.type(screen.getByTestId('export-passphrase'), 'long enough passphrase');
  await user.type(screen.getByTestId('export-passphrase-confirm'), 'different passphrase');
  expect(screen.getByTestId('export-submit')).toBeDisabled();
  expect(screen.getByText('The passphrases do not match.')).toBeInTheDocument();

  await user.clear(screen.getByTestId('export-passphrase-confirm'));
  await user.type(screen.getByTestId('export-passphrase-confirm'), 'long enough passphrase');
  expect(screen.getByTestId('export-submit')).toBeEnabled();

  await user.click(screen.getByTestId('export-submit'));
  await waitFor(() => {
    expect(screen.getByTestId('export-envelope')).toHaveValue(
      'kukuri-account-key.v1.ZW5jcnlwdGVk'
    );
  });
  expect(identityApi.exportAccountKey).toHaveBeenCalledWith('long enough passphrase');
  // 平文秘密鍵は DOM のどこにも現れない。
  expect(document.body.textContent).not.toContain(SECRET_HEX);
  expect(screen.queryByText(SECRET_HEX)).not.toBeInTheDocument();
});

test('import requires a fingerprint preview and rejects already registered keys', async () => {
  const user = userEvent.setup();
  const preview: AccountKeyImportPreview = {
    version: 1,
    kdf: 'argon2id',
    public_key: OTHER_PUBKEY,
    already_registered: true,
  };
  identityApi.previewAccountKeyImport.mockResolvedValue(preview);
  render(<AccountKeyPanel />);
  await screen.findByText(ACTIVE_PUBKEY);

  expect(screen.queryByTestId('import-submit')).not.toBeInTheDocument();
  await user.type(screen.getByTestId('import-input'), 'kukuri-account-key.v1.something');
  await user.click(screen.getByTestId('import-preview-button'));

  expect(await screen.findByTestId('import-preview')).toBeInTheDocument();
  expect(
    screen.getByText('This key is already registered on this device.')
  ).toBeInTheDocument();
  await user.type(screen.getByTestId('import-passphrase'), 'whatever pass');
  expect(screen.getByTestId('import-submit')).toBeDisabled();
});

test('successful import offers switching to the new account', async () => {
  const user = userEvent.setup();
  const newRecord = {
    id: OTHER_PUBKEY.slice(0, 16),
    pubkey: OTHER_PUBKEY,
    label: null,
    created_at: 5,
    last_used_at: 6,
  };
  identityApi.previewAccountKeyImport.mockResolvedValue({
    version: 1,
    kdf: 'argon2id',
    public_key: OTHER_PUBKEY,
    already_registered: false,
  });
  identityApi.importAccountKey.mockResolvedValue(newRecord);
  identityApi.switchAccount.mockResolvedValue(newRecord);
  const reload = vi.fn();
  vi.stubGlobal('location', { ...window.location, reload });

  render(<AccountKeyPanel />);
  await screen.findByText(ACTIVE_PUBKEY);

  await user.type(screen.getByTestId('import-input'), 'kukuri-account-key.v1.something');
  await user.click(screen.getByTestId('import-preview-button'));
  await screen.findByTestId('import-preview');
  await user.type(screen.getByTestId('import-passphrase'), 'correct passphrase');
  await user.click(screen.getByTestId('import-submit'));

  expect(await screen.findByTestId('import-success')).toBeInTheDocument();
  expect(identityApi.importAccountKey).toHaveBeenCalledWith(
    'kukuri-account-key.v1.something',
    'correct passphrase',
    undefined
  );

  await user.click(screen.getByTestId('import-switch-now'));
  await waitFor(() => {
    expect(identityApi.switchAccount).toHaveBeenCalledWith(OTHER_PUBKEY.slice(0, 16));
  });
  await waitFor(() => {
    expect(reload).toHaveBeenCalled();
  });
  vi.unstubAllGlobals();
});

// #1217 AC-5: Web では、このブラウザの保存の状態と、消えたときの戻し方を示す。許可が無いのに「消えない」と示さない。
test.each([
  [true, "This browser does not delete this site's data automatically."],
  [false, "This browser may delete this site's data automatically"],
])('the web build shows whether this browser keeps the site data (persisted: %s)', async (persisted, state) => {
  Object.defineProperty(navigator, 'storage', {
    configurable: true,
    value: { persisted: vi.fn().mockResolvedValue(persisted) },
  });
  try {
    render(<AccountKeyPanel showBrowserStorage />);
    const notice = await screen.findByTestId('browser-storage-notice');
    expect(notice).toHaveTextContent(state);
    expect(notice).toHaveTextContent('importing it from Add account');
    expect(notice).toHaveTextContent('Not all history, such as posts, comes back.');
  } finally {
    Reflect.deleteProperty(navigator, 'storage');
  }
});

test('the desktop build does not show the browser storage notice', async () => {
  render(<AccountKeyPanel />);
  await screen.findByText(ACTIVE_PUBKEY);
  expect(screen.queryByTestId('browser-storage-notice')).not.toBeInTheDocument();
});

// #1211 AC-5: 別の端末で使う方法の対象の差。backup・restore の行は、それを開ける desktop だけに出す。
test('the account settings compare the ways to use the account on another device', async () => {
  const { unmount } = render(<AccountKeyPanel onOpenDeviceBackup={() => undefined} />);
  const methods = await screen.findByTestId('account-key-methods');
  expect(methods).toHaveTextContent('Move to another device (Add account > Move between devices)');
  expect(methods).toHaveTextContent('follows and blocks');
  expect(methods).toHaveTextContent('Export the account key: writes out only the account key.');
  expect(methods).toHaveTextContent('Backup & restore: moves all the data on this device.');
  unmount();

  render(<AccountKeyPanel />);
  expect(await screen.findByTestId('account-key-methods')).not.toHaveTextContent('Backup & restore');
});

test.each([
  ['ja', 'アカウントを別の端末で使うには、次の方法があります。', 'フォローとブロック', 'バックアップと復元'],
  ['zh-CN', '要在其他设备上使用账号，有以下方法。', '关注和屏蔽', '备份与恢复'],
])('the account settings compare the ways in %s', async (language, intro, moved, backup) => {
  await i18n.changeLanguage(language);
  try {
    const { unmount } = render(<AccountKeyPanel onOpenDeviceBackup={() => undefined} />);
    const methods = await screen.findByTestId('account-key-methods');
    expect(methods).toHaveTextContent(intro);
    expect(methods).toHaveTextContent(moved);
    expect(methods).toHaveTextContent(backup);
    unmount();

    render(<AccountKeyPanel />);
    const web = await screen.findByTestId('account-key-methods');
    expect(web).toHaveTextContent(intro);
    expect(web).not.toHaveTextContent(backup);
  } finally {
    await i18n.changeLanguage('en');
  }
});

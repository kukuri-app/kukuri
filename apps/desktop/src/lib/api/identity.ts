import type {
  AccountDisplay,
  InitialProfileRequest,
  CreateAccountRequest,
  Profile,
  AccountKeyExport,
  AccountKeyImportPreview,
  AccountRecord,
  AccountsSnapshot,
  AccountTransferLink,
  AccountTransferStatus,
  DecideAccountTransferRequest,
  ExportAccountKeyRequest,
  ImportAccountKeyRequest,
  OpenAccountTransferRequest,
  PreviewAccountKeyImportRequest,
  SwitchAccountRequest,
} from './types.generated';

import { invokeDesktop } from './invoke/desktop';
import { isDesktopMockActive } from './invoke/dispatch';

// #859: アカウント鍵の export / import と複数アカウント管理。DesktopApi 外の
// スタンドアロンコマンド(appConsent.ts と同じ形)。mock ビルドでは in-memory の
// アカウント一覧を返す。平文秘密鍵はどの経路にも現れない。

const MOCK_ACTIVE_ACCOUNT: AccountRecord = {
  id: 'a1b2c3d4e5f60718',
  pubkey: 'a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90',
  label: null,
  created_at: 1_756_684_800_000,
  last_used_at: 1_756_684_800_000,
};

const mockAccounts: AccountsSnapshot = {
  active_account_id: MOCK_ACTIVE_ACCOUNT.id,
  accounts: [MOCK_ACTIVE_ACCOUNT],
};

const MOCK_EXPORT_PREFIX = 'kukuri-account-key.v1.';
let mockAccountInitialized = false;

export async function listAccounts(): Promise<AccountsSnapshot> {
  if (isDesktopMockActive()) {
    if (!mockAccountInitialized && window.__KUKURI_DESKTOP__) {
      const profile = await window.__KUKURI_DESKTOP__.getMyProfile();
      MOCK_ACTIVE_ACCOUNT.id = profile.pubkey.slice(0, 16);
      MOCK_ACTIVE_ACCOUNT.pubkey = profile.pubkey;
      MOCK_ACTIVE_ACCOUNT.label = profile.display_name ?? null;
      mockAccounts.active_account_id = MOCK_ACTIVE_ACCOUNT.id;
      mockAccountInitialized = true;
    }
    return {
      active_account_id: mockAccounts.active_account_id,
      accounts: mockAccounts.accounts.map((account) => ({ ...account })),
    };
  }
  return invokeDesktop<AccountsSnapshot>('list_accounts');
}

export async function getAccountDisplay(): Promise<AccountDisplay[]> {
  if (isDesktopMockActive()) return mockAccounts.accounts.map((a) => ({ id: a.id, name: null, display_name: a.label, picture: null, unavailable: false }));
  return invokeDesktop<AccountDisplay[]>('get_account_display');
}

export async function createAccount(accountId: string, operationId: string): Promise<AccountRecord> {
  if (isDesktopMockActive()) {

    const id = operationId.replaceAll('-', '').slice(0, 16);
    const existing = mockAccounts.accounts.find((a) => a.id === id);
    if (existing && mockAccounts.active_account_id === id) return existing;
    if (mockAccounts.active_account_id !== accountId) throw new Error('account is no longer active');
    const record = { id, pubkey: id.repeat(4), label: null, created_at: Date.now(), last_used_at: Date.now() };
    mockAccounts.accounts.push(record); mockAccounts.active_account_id = id;
    return record;
  }
  return invokeDesktop<AccountRecord>('create_account', { request: { account_id: accountId, operation_id: operationId } satisfies CreateAccountRequest });
}

export async function logoutAccount(accountId: string): Promise<AccountRecord> {
  if (isDesktopMockActive()) {
    if (accountId !== mockAccounts.active_account_id) throw new Error('account is no longer active');
    mockAccounts.accounts = mockAccounts.accounts.filter((a) => a.id !== accountId);
    if (!mockAccounts.accounts.length) {
      const id = crypto.randomUUID().replaceAll('-', '').slice(0, 16);
      mockAccounts.accounts.push({ id, pubkey: id.repeat(4), label: null, created_at: Date.now(), last_used_at: Date.now() });
    }
    const next = mockAccounts.accounts[0];
    mockAccounts.active_account_id = next.id;
    return next;
  }
  return invokeDesktop<AccountRecord>('logout_account', { request: { account_id: accountId } satisfies SwitchAccountRequest });
}

export async function getProfileSetupRequired(accountId: string): Promise<boolean> {
  if (isDesktopMockActive()) return false;
  return invokeDesktop<boolean>('get_profile_setup_required', { request: { account_id: accountId } satisfies SwitchAccountRequest });
}

export async function saveInitialProfile(request: InitialProfileRequest): Promise<Profile> {
  return invokeDesktop<Profile>('save_initial_profile', { request });
}

export async function exportAccountKey(passphrase: string): Promise<AccountKeyExport> {
  if (isDesktopMockActive()) {
    return {
      export: `${MOCK_EXPORT_PREFIX}bW9jay1lbmNyeXB0ZWQtZW52ZWxvcGU`,
      public_key: MOCK_ACTIVE_ACCOUNT.pubkey,
    };
  }
  return invokeDesktop<AccountKeyExport>('export_account_key', {
    request: { passphrase } satisfies ExportAccountKeyRequest,
  });
}

export async function previewAccountKeyImport(
  exportText: string
): Promise<AccountKeyImportPreview> {
  if (isDesktopMockActive()) {
    if (!exportText.startsWith(MOCK_EXPORT_PREFIX)) {
      throw new Error('unsupported account key export format or version');
    }
    return {
      version: 1,
      kdf: 'argon2id',
      public_key: 'f0e1d2c3b4a59687f0e1d2c3b4a59687f0e1d2c3b4a59687f0e1d2c3b4a59687',
      already_registered: false,
    };
  }
  return invokeDesktop<AccountKeyImportPreview>('preview_account_key_import', {
    request: { export: exportText } satisfies PreviewAccountKeyImportRequest,
  });
}

export async function importAccountKey(
  exportText: string,
  passphrase: string,
  label?: string
): Promise<AccountRecord> {
  if (isDesktopMockActive()) {
    const preview = await previewAccountKeyImport(exportText);
    const record: AccountRecord = {
      id: preview.public_key.slice(0, 16),
      pubkey: preview.public_key,
      label: label ?? null,
      created_at: Date.now(),
      last_used_at: Date.now(),
    };
    if (mockAccounts.accounts.some((account) => account.pubkey === record.pubkey)) {
      throw new Error(`account with public key \`${record.pubkey}\` already exists`);
    }
    mockAccounts.accounts.push(record);
    return { ...record };
  }
  return invokeDesktop<AccountRecord>('import_account_key', {
    request: {
      export: exportText,
      passphrase,
      label: label ?? null,
    } satisfies ImportAccountKeyRequest,
  });
}

export async function switchAccount(accountId: string): Promise<AccountRecord> {
  if (isDesktopMockActive()) {
    const record = mockAccounts.accounts.find((account) => account.id === accountId);
    if (!record) {
      throw new Error(`unknown account \`${accountId}\``);
    }
    mockAccounts.active_account_id = accountId;
    return { ...record };
  }
  return invokeDesktop<AccountRecord>('switch_account', {
    request: { account_id: accountId } satisfies SwitchAccountRequest,
  });
}

// #1211: QR・専用リンクの移行。リンクは招待の秘密を含むので、log・URL の query へ出さない。
// mock ビルドでは、接続と相手の承認を即座に済ませ、1.5 秒の転送中の後に完了にする（移行先は今のアカウントを受けた
// ことにして、切り替えない）。
let mockTransfer: AccountTransferStatus = { state: 'idle' };
let mockTransferDoneAt = 0;

export async function createAccountTransferInvite(): Promise<AccountTransferLink> {
  if (isDesktopMockActive()) {
    const expires_at_ms = Date.now() + 5 * 60 * 1000;
    mockTransfer = { state: 'waiting', expires_at_ms };
    return { link: 'kukuri://transfer#v1.bW9jay1hY2NvdW50LXRyYW5zZmVyLWludml0ZQ', expires_at_ms };
  }
  return invokeDesktop<AccountTransferLink>('create_account_transfer_invite');
}

export async function openAccountTransfer(link: string): Promise<void> {
  if (isDesktopMockActive()) {
    if (!link.trim().startsWith('kukuri://transfer#v1.')) throw new Error('not an account transfer link');
    mockTransfer = { state: 'confirming', role: 'target', code: '482915', local_accepted: false };
    return;
  }
  return invokeDesktop<void>('open_account_transfer', { request: { link } satisfies OpenAccountTransferRequest });
}

export async function getAccountTransferStatus(): Promise<AccountTransferStatus> {
  if (isDesktopMockActive()) {
    if (mockTransfer.state === 'transferring' && Date.now() >= mockTransferDoneAt) {
      const { role } = mockTransfer;
      mockTransfer = { state: 'completed', role, account_id: role === 'target' ? mockAccounts.active_account_id : null };
    }
    return mockTransfer;
  }
  return invokeDesktop<AccountTransferStatus>('get_account_transfer_status');
}

export async function decideAccountTransfer(accept: boolean): Promise<void> {
  if (isDesktopMockActive()) {
    if (mockTransfer.state !== 'confirming') throw new Error('account transfer is not awaiting confirmation');
    mockTransferDoneAt = Date.now() + 1500;
    mockTransfer = accept ? { state: 'transferring', role: mockTransfer.role, items: 12 } : { state: 'failed', role: mockTransfer.role, reason: 'rejected' };
    return;
  }
  return invokeDesktop<void>('decide_account_transfer', { request: { accept } satisfies DecideAccountTransferRequest });
}

export async function cancelAccountTransfer(): Promise<void> {
  if (isDesktopMockActive()) { mockTransfer = { state: 'idle' }; return; }
  return invokeDesktop<void>('cancel_account_transfer');
}

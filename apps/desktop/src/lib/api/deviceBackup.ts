import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/plugin-dialog';

import { DESKTOP_LOCALE_STORAGE_KEY } from '@/i18n';
import { DESKTOP_THEME_STORAGE_KEY } from '@/lib/theme';
import { COLUMN_DRAFT_STORAGE_KEY } from '@/shell/columnDraftPersistence';
import { COMMUNITY_INDEX_NODE_PREFERENCE_STORAGE_KEY } from '@/shell/communityIndexNodePreference';
import { SAVED_WORKSPACE_LAYOUTS_STORAGE_KEY } from '@/shell/savedWorkspaceLayouts';
import { WORKSPACE_LAYOUT_STORAGE_KEY } from '@/shell/workspacePersistence';

import type {
  CreateDeviceBackupRequest,
  DeviceBackupPreview,
  DeviceBackupProgress,
  DeviceBackupRestoreResult,
  DeviceBackupSummary,
  PreviewDeviceBackupRequest,
  RestoreDeviceBackupRequest,
} from './types.generated';
import { invokeDesktop } from './invoke/desktop';
import { isDesktopMockActive } from './invoke/dispatch';

export const DEVICE_BACKUP_PROGRESS_EVENT = 'kukuri://device-backup-progress';

const PORTABLE_FRONTEND_STORAGE_KEYS = [
  DESKTOP_LOCALE_STORAGE_KEY,
  DESKTOP_THEME_STORAGE_KEY,
  COLUMN_DRAFT_STORAGE_KEY,
  COMMUNITY_INDEX_NODE_PREFERENCE_STORAGE_KEY,
  SAVED_WORKSPACE_LAYOUTS_STORAGE_KEY,
  WORKSPACE_LAYOUT_STORAGE_KEY,
] as const;

export function capturePortableFrontendState(storage: Storage = window.localStorage) {
  const state: Record<string, string> = {};
  for (const key of PORTABLE_FRONTEND_STORAGE_KEYS) {
    const value = storage.getItem(key);
    if (value !== null) state[key] = value;
  }
  return state;
}

export function applyPortableFrontendState(
  values: Record<string, string>,
  storage: Storage = window.localStorage
) {
  const allowed = new Set<string>(PORTABLE_FRONTEND_STORAGE_KEYS);
  for (const [key, value] of Object.entries(values)) {
    if (allowed.has(key)) storage.setItem(key, value);
  }
}

type PortableFrontendSnapshot = Map<string, string | null>;

function capturePortableFrontendSnapshot(storage: Storage): PortableFrontendSnapshot {
  return new Map(PORTABLE_FRONTEND_STORAGE_KEYS.map((key) => [key, storage.getItem(key)]));
}

function restorePortableFrontendSnapshot(
  snapshot: PortableFrontendSnapshot,
  storage: Storage
): void {
  for (const [key, value] of snapshot) {
    if (value === null) storage.removeItem(key);
    else storage.setItem(key, value);
  }
}

export async function getPendingDeviceRestoreFrontendState(): Promise<Record<string, string> | null> {
  if (isDesktopMockActive()) return null;
  return invokeDesktop<Record<string, string> | null>('get_pending_device_restore_frontend_state');
}

export async function acknowledgePendingDeviceRestoreFrontendState(): Promise<void> {
  if (isDesktopMockActive()) return;
  await invokeDesktop<void>('acknowledge_pending_device_restore_frontend_state');
}

/**
 * activation済みの復元だけをlocalStorageへ反映する。
 *
 * backend markerをackするまでは再実行可能で、applyまたはackが失敗した場合は
 * 今回触れたportable stateを呼出前へ戻す。process停止時はmarkerが残るため、
 * 次回のReady startupで同じ値へfinish-forwardできる。
 */
export async function applyPendingDeviceRestoreFrontendState(
  storage: Storage = window.localStorage
): Promise<boolean> {
  const pending = await getPendingDeviceRestoreFrontendState();
  if (pending === null) return false;

  const snapshot = capturePortableFrontendSnapshot(storage);
  try {
    applyPortableFrontendState(pending, storage);
    await acknowledgePendingDeviceRestoreFrontendState();
    return true;
  } catch (error) {
    restorePortableFrontendSnapshot(snapshot, storage);
    throw error;
  }
}

export async function chooseDeviceBackupDestination(): Promise<string | null> {
  if (isDesktopMockActive()) return 'C:\\mock\\kukuri-account.kukuri-backup';
  return save({
    defaultPath: `kukuri-account-${new Date().toISOString().slice(0, 10)}.kukuri-backup`,
    filters: [{ name: 'kukuri device backup', extensions: ['kukuri-backup'] }],
  });
}

export async function chooseDeviceBackupSource(): Promise<string | null> {
  if (isDesktopMockActive()) return 'C:\\mock\\kukuri-account.kukuri-backup';
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [{ name: 'kukuri device backup', extensions: ['kukuri-backup'] }],
  });
  return typeof selected === 'string' ? selected : null;
}

export async function createDeviceBackup(
  path: string,
  passphrase: string,
  frontendState: Record<string, string>
): Promise<DeviceBackupSummary> {
  if (isDesktopMockActive()) {
    return { path, public_key: 'a1'.repeat(32), bytes: 4096 };
  }
  return invokeDesktop<DeviceBackupSummary>('create_device_backup_command', {
    request: {
      path,
      passphrase,
      frontend_state: frontendState,
    } satisfies CreateDeviceBackupRequest,
  });
}

export async function previewDeviceBackup(
  path: string,
  passphrase: string
): Promise<DeviceBackupPreview> {
  if (isDesktopMockActive()) {
    return {
      public_key: 'a1'.repeat(32),
      account_label: 'Mock account',
      created_at: Math.floor(Date.now() / 1000),
      app_version: '0.1.8',
      content_bytes: 4096,
      existing_account_id: null,
      included: ['account_key', 'sqlite', 'protected_content'],
      requires_reconsent: ['app_legal_documents', 'age_attestation'],
    };
  }
  return invokeDesktop<DeviceBackupPreview>('preview_device_backup_command', {
    request: { path, passphrase } satisfies PreviewDeviceBackupRequest,
  });
}

export async function restoreDeviceBackup(
  path: string,
  passphrase: string,
  replaceExisting: boolean,
  applyFrontendState: boolean
): Promise<DeviceBackupRestoreResult> {
  if (isDesktopMockActive()) {
    return {
      account: {
        id: 'a1'.repeat(8),
        pubkey: 'a1'.repeat(32),
        label: 'Mock account',
        created_at: Date.now(),
        last_used_at: Date.now(),
      },
      frontend_state: applyFrontendState ? capturePortableFrontendState() : {},
    };
  }
  return invokeDesktop<DeviceBackupRestoreResult>('restore_device_backup_command', {
    request: {
      path,
      passphrase,
      replace_existing: replaceExisting,
      apply_frontend_state: applyFrontendState,
    } satisfies RestoreDeviceBackupRequest,
  });
}

export async function cancelDeviceBackup(): Promise<void> {
  if (isDesktopMockActive()) return;
  await invokeDesktop<void>('cancel_device_backup');
}

export async function listenDeviceBackupProgress(
  handler: (progress: DeviceBackupProgress) => void
): Promise<UnlistenFn> {
  if (isDesktopMockActive()) return () => {};
  return listen<DeviceBackupProgress>(DEVICE_BACKUP_PROGRESS_EVENT, (event) => handler(event.payload));
}

import { DESKTOP_DISTRIBUTION } from './distribution';

// Store が更新を管理する build（Microsoft Store・Google Play）は、channel に配布の名前を出す。
export const RELEASE_CHANNEL = DESKTOP_DISTRIBUTION === 'direct' ? 'preview' : DESKTOP_DISTRIBUTION;
// manifest は自分で更新する build（direct）だけが表示する。
export const RELEASE_MANIFEST_NAME = 'latest-preview.json';
export const RELEASE_FEEDBACK_URL =
  'https://github.com/kukuri-app/kukuri/issues/new?template=preview-feedback.md';
export const RELEASE_LATEST_URL = 'https://github.com/kukuri-app/kukuri/releases/latest';
export const RELEASE_QUICKSTART_URL =
  'https://github.com/kukuri-app/kukuri/blob/main/docs/runbooks/mvp-user-quickstart.md';
export const RELEASE_RUNBOOK_URL =
  'https://github.com/kukuri-app/kukuri/blob/main/docs/runbooks/release.md';
export const THIRD_PARTY_NOTICES_URL =
  'https://github.com/kukuri-app/kukuri/blob/main/docs/THIRD_PARTY_NOTICES.md';
export const OS_NOTIFICATION_SETTINGS_STORAGE_KEY = 'kukuri:os-notification-settings:v1';

export type UpdateStatus =
  | 'idle'
  | 'checking'
  | 'up_to_date'
  | 'available'
  | 'downloading'
  | 'ready_to_restart'
  | 'installing'
  | 'failed';

export type UpdateErrorKind = 'network' | 'manifest' | 'signature' | 'install' | 'missing' | 'unknown'
  | 'debCancelled' | 'debAuthorization' | 'debInstall';

export type UpdateState = {
  status: UpdateStatus;
  currentVersion: string;
  availableVersion?: string | null;
  downloadedBytes?: number;
  contentLength?: number | null;
  lastError?: string | null;
  /** 直近の更新確認が完了した時刻(ms)。結果が前回と同じでも確認ごとに更新する。 */
  lastCheckedAt?: number | null;
};

export type OsNotificationSettings = {
  enabled: boolean;
  directMessages: boolean;
  mentionsAndReplies: boolean;
  followsAndReposts: boolean;
  quietMode: boolean;
  previewBody: boolean;
};

export const DEFAULT_OS_NOTIFICATION_SETTINGS: OsNotificationSettings = {
  enabled: false,
  directMessages: true,
  mentionsAndReplies: true,
  followsAndReposts: false,
  quietMode: false,
  previewBody: false,
};

export function loadOsNotificationSettings(): OsNotificationSettings {
  if (typeof window === 'undefined') {
    return DEFAULT_OS_NOTIFICATION_SETTINGS;
  }
  const rawValue = window.localStorage.getItem(OS_NOTIFICATION_SETTINGS_STORAGE_KEY);
  if (!rawValue) {
    return DEFAULT_OS_NOTIFICATION_SETTINGS;
  }
  try {
    const parsed = JSON.parse(rawValue) as Partial<OsNotificationSettings>;
    return {
      ...DEFAULT_OS_NOTIFICATION_SETTINGS,
      ...parsed,
    };
  } catch {
    return DEFAULT_OS_NOTIFICATION_SETTINGS;
  }
}

export function saveOsNotificationSettings(settings: OsNotificationSettings): void {
  if (typeof window === 'undefined') {
    return;
  }
  window.localStorage.setItem(OS_NOTIFICATION_SETTINGS_STORAGE_KEY, JSON.stringify(settings));
  window.dispatchEvent(new Event(OS_NOTIFICATION_SETTINGS_STORAGE_KEY));
}

export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export function classifyUpdateError(errorMessage?: string | null): UpdateErrorKind {
  if (!errorMessage) {
    return 'unknown';
  }
  const normalized = errorMessage.toLowerCase();
  if (normalized.includes('deb_update_auth_cancelled')) return 'debCancelled';
  if (normalized.includes('deb_update_auth_unavailable') || normalized.includes('deb_update_root_forbidden')) return 'debAuthorization';
  if (normalized.includes('deb_update_install_failed')) return 'debInstall';
  if (normalized.includes('deb_update_format_mismatch') || normalized.includes('deb_update_package_mismatch')) return 'manifest';

  if (
    normalized.includes('release json') ||
    normalized.includes('manifest') ||
    normalized.includes('latest-preview.json') ||
    normalized.includes('invalid release') ||
    normalized.includes('valid release')
  ) {
    return 'manifest';
  }
  if (
    normalized.includes('signature') ||
    normalized.includes(' sig') ||
    normalized.includes('.sig') ||
    normalized.includes('verify') ||
    normalized.includes('verification') ||
    normalized.includes('ed25519')
  ) {
    return 'signature';
  }
  if (
    normalized.includes('install') ||
    normalized.includes('installer') ||
    normalized.includes('restart') ||
    normalized.includes('apply update')
  ) {
    return 'install';
  }
  if (/download request failed with status:\s*404\b/.test(normalized)) {
    return 'missing';
  }
  if (
    normalized.includes('network') ||
    normalized.includes('fetch') ||
    normalized.includes('request') ||
    normalized.includes('connection') ||
    normalized.includes('connect') ||
    normalized.includes('timeout') ||
    normalized.includes('dns') ||
    normalized.includes('offline')
  ) {
    return 'network';
  }

  return 'unknown';
}

export function buildSafeDiagnosticReport(input: {
  appVersion: string;
  updateState: UpdateState;
  osNotificationPermission: string;
  osNotificationSettings: OsNotificationSettings;
  userAgent: string;
  platform: string;
  syncConnected: boolean;
  deliveryState: string;
  discoveryMode: string;
  activePath: string;
  peerCount: number;
  subscribedTopicCount: number;
  unreadNotificationCount: number;
  communityNodeStatuses: Array<{
    base_url: string;
    session_phase?: string | null;
    retry_after?: number | null;
    restart_required: boolean;
    last_error?: string | null;
  }>;
  lastSyncError?: string | null;
  lastDiscoveryError?: string | null;
}): string {
  const lines = [
    '# kukuri diagnostic report',
    '',
    `app_version: ${input.appVersion}`,
    `release_channel: ${RELEASE_CHANNEL}`,
    `platform: ${input.platform}`,
    `user_agent: ${input.userAgent}`,
    `sync_connected: ${input.syncConnected ? 'yes' : 'no'}`,
    `delivery_state: ${input.deliveryState}`,
    `discovery_mode: ${input.discoveryMode}`,
    `active_path: ${input.activePath}`,
    `peer_count: ${input.peerCount}`,
    `subscribed_topic_count: ${input.subscribedTopicCount}`,
    `unread_notification_count: ${input.unreadNotificationCount}`,
    `update_status: ${input.updateState.status}`,
    `update_current_version: ${input.updateState.currentVersion}`,
    `update_available_version: ${input.updateState.availableVersion ?? 'none'}`,
    `update_last_error: ${input.updateState.lastError ?? 'none'}`,
    `os_notification_permission: ${input.osNotificationPermission}`,
    `os_notifications_enabled: ${input.osNotificationSettings.enabled ? 'yes' : 'no'}`,
    `last_sync_error: ${input.lastSyncError ?? 'none'}`,
    `last_discovery_error: ${input.lastDiscoveryError ?? 'none'}`,
    '',
    'community_nodes:',
  ];

  if (input.communityNodeStatuses.length === 0) {
    lines.push('- none');
  } else {
    for (const status of input.communityNodeStatuses) {
      lines.push(
        [
          `- base_url: ${status.base_url}`,
          `session_phase: ${status.session_phase ?? 'unknown'}`,
          `retry_after: ${status.retry_after ?? 'none'}`,
          `restart_required: ${status.restart_required ? 'yes' : 'no'}`,
          `last_error: ${status.last_error ?? 'none'}`,
        ].join('; ')
      );
    }
  }

  lines.push(
    '',
    'redaction:',
    '- secret keys, auth tokens, private channel capability secrets, invite/share tokens, DM bodies, and local DB paths are not included.'
  );

  return lines.join('\n');
}

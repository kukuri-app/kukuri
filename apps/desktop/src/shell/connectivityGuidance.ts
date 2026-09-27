import type { SyncStatus, TopicSyncStatus } from '@/lib/api';
import type { SyncStatusRead } from './slices/connectivity';
import { diagnosticErrorSummary, diagnosticValueLabel } from './diagnosticLabels';

type Translate = (key: string, options?: Record<string, unknown>) => string;
export type ConnectivityGuidance = {
  label: string;
  description: string;
  nextStep: string;
  tone: 'accent' | 'warning';
  loaded: boolean;
  refreshing: boolean;
  readError: string | null;
  errorSummary: string | null;
};

// The protocol snapshot remains unchanged; these states describe only the UI.
export function connectivityGuidance(
  sync: SyncStatus,
  read: SyncStatusRead,
  t: Translate,
  topic?: { id: string; diagnostic?: TopicSyncStatus },
): ConnectivityGuidance {
  const value = topic ? topic.diagnostic : sync;
  const live = !!value && value.peer_count > 0 && value.delivery_state === 'Live';
  const disabled = topic && sync.gossip_disabled_topics?.includes(topic.id);
  const state = !read.loaded ? 'unknown'
    : disabled ? 'paused'
    : !value ? (sync.subscribed_topics.includes(topic!.id) ? 'preparing' : 'unsubscribed')
    : live ? 'live'
    : value.delivery_state === 'DurableReady' ? 'durable'
    : value.delivery_state === 'DurableRecovering' ? 'recovering' : 'offline';
  const error = value?.last_error;
  const stateLabel = t(`settings:connectionGuidance.states.${state}`);
  const label = read.loaded && read.error
    ? t('settings:connectionGuidance.previousLabel', { state: stateLabel }) : stateLabel;
  return {
    label: live && read.loaded && !disabled
      ? `${label} · ${diagnosticValueLabel('path', value!.active_path, t, false)}` : label,
    description: t(topic && live && !disabled && read.loaded ? 'settings:connectionGuidance.topicLive' : `settings:connectionGuidance.descriptions.${state}`),
    nextStep: t(`settings:connectionGuidance.steps.${state}`),
    tone: live && read.loaded && !read.error && !disabled ? 'accent' : 'warning',
    loaded: read.loaded,
    refreshing: read.refreshing,
    readError: read.error ? t(`settings:connectionGuidance.${read.loaded ? 'stale' : 'failed'}`) : null,
    errorSummary: read.loaded ? diagnosticErrorSummary(error, t) : null,
  };
}

export function discoveryGuidance(sync: SyncStatus, read: SyncStatusRead, t: Translate): ConnectivityGuidance {
  const connected = sync.discovery.connected_peer_count > 0;
  const base = connectivityGuidance(sync, read, t);
  const error = sync.discovery.last_discovery_error;
  return {
    ...base,
    label: read.loaded ? (read.error
      ? t('settings:connectionGuidance.previousLabel', { state: t(`settings:connectionGuidance.discovery.${connected ? 'found' : 'waiting'}`) })
      : t(`settings:connectionGuidance.discovery.${connected ? 'found' : 'waiting'}`)) : base.label,
    description: read.loaded ? t('settings:connectionGuidance.discovery.description') : base.description,
    nextStep: read.loaded ? t('settings:connectionGuidance.steps.offline') : base.nextStep,
    tone: connected && read.loaded && !read.error ? 'accent' : 'warning',
    errorSummary: read.loaded ? diagnosticErrorSummary(error, t) : null,
  };
}

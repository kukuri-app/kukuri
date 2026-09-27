import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { useTranslation } from 'react-i18next';
import { createInitialConnectivitySlice } from '@/shell/slices/connectivity';
import { createDesktopShellStore } from '@/shell/store';
import { useSettingsViewModels } from '@/shell/viewModels/useSettingsViewModels';
import { ConnectivityPanel } from './ConnectivityPanel';
import { DiscoveryPanel } from './DiscoveryPanel';
import { getResolvedLocale } from '@/i18n/format';
import type { SyncStatus } from '@/lib/api';
import { SettingsStoryFrame } from './SettingsStoryFrame';

type State = 'unknown' | 'loading' | 'failed' | 'stale' | 'offline' | 'recovering' | 'durable' | 'live' | 'topics' | 'discovery';
function DiagnosticStory({ state, width = 'wide', operationError = false }:
  { state: State; width?: 'wide' | 'narrow'; operationError?: boolean }) {
  const { t, i18n } = useTranslation(['settings', 'common']);
  const [initial] = useState(() => createDesktopShellStore().getState());
  const [ticket, setTicket] = useState('');
  const [seed, setSeed] = useState('candidate-peer');
  const [refreshed, setRefreshed] = useState(false);
  const base = createInitialConnectivitySlice().syncStatus;
  const live = state === 'live' || refreshed;
  const sync: SyncStatus = { ...base, connected: live, peer_count: live ? 1 : 0,
    delivery_state: live ? 'Live' as const : state === 'durable' ? 'DurableReady' as const : 'DurableRecovering' as const,
    configured_peer_count: 1, subscribed_topics: ['general'], gossip_disabled_topics: ['test'],
    last_error: live ? null : 'topic join pending: timed out waiting for initial topic join',
    discovery: { ...base.discovery, docs_assist_peer_count: 1 } };
  if (state === 'offline') sync.delivery_state = base.delivery_state;
  const view = useSettingsViewModels({ ...initial, syncStatus: sync,
    error: operationError ? 'failed to import peer ticket: invalid endpoint id' : null,
    syncStatusRead: { loaded: !['unknown', 'loading', 'failed'].includes(state) || refreshed,
      refreshing: state === 'loading', error: ['failed', 'stale'].includes(state) && !refreshed },
    trackedTopics: state === 'topics' ? ['general', 'dev', 'test'] : [], topicDiagnostics: {},
    peerTicket: ticket, discoverySeedInput: seed, locale: getResolvedLocale(i18n.resolvedLanguage), theme: 'dark', t });
  const actions = { onRefreshDiagnostics: () => setRefreshed(true), onOpenCommunityNode: () => undefined };
  return <SettingsStoryFrame width={width}><main className='max-w-3xl p-4'>{state === 'discovery'
    ? <DiscoveryPanel view={view.discoveryPanelView} {...actions} saveDisabled resetDisabled onSeedPeersChange={setSeed} onSave={() => {}} onReset={() => {}} />
    : <ConnectivityPanel view={view.connectivityPanelView} {...actions} onPeerTicketInputChange={setTicket} onImportPeer={() => {}} />}</main></SettingsStoryFrame>;
}
const meta = { title: 'Settings/ConnectivityPanel', component: DiagnosticStory,
  parameters: { layout: 'fullscreen' }, args: { state: 'recovering' } } satisfies Meta<typeof DiagnosticStory>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Ready: Story = { args: { state: 'live' } };
export const NarrowError: Story = { args: { state: 'offline', width: 'narrow', operationError: true } };
export const Unknown: Story = { args: { state: 'unknown' } };
export const Loading: Story = { args: { state: 'loading' } };
export const Failed: Story = { args: { state: 'failed' } };
export const Stale: Story = { args: { state: 'stale' } };
export const Offline: Story = { args: { state: 'offline' } };
export const Recovering: Story = { args: { state: 'recovering' } };
export const Durable: Story = { args: { state: 'durable' } };
export const Live: Story = { args: { state: 'live' } };
export const MissingTopics: Story = { args: { state: 'topics' } };
export const Discovery: Story = { args: { state: 'discovery' } };

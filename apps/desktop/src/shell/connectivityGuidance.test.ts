import { expect, test } from 'vitest';
import i18n from '@/i18n';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { connectivityGuidance, discoveryGuidance } from './connectivityGuidance';
import { INITIAL_SYNC_STATUS_READ } from './slices/connectivity';

const ready = { loaded: true, refreshing: false, error: false };
const t = (key: string, options?: Record<string, unknown>) => i18n.t(key, options);
const timeout = 'topic join pending: timed out waiting for initial topic join';

for (const locale of ['ja', 'en', 'zh-CN']) {
  for (const path of ['direct_p2p', 'relay_supported_p2p', 'relay_fallback'] as const) {
    for (const delivery of ['Live', 'DurableReady', 'DurableRecovering', 'Offline'] as const) {
      test(`${locale}: ${path} / ${delivery} separates delivery from candidates and historical errors`, async () => {
        await i18n.changeLanguage(locale);
        const base = await createDesktopMockApi().getSyncStatus();
        const sync = { ...base, connected: delivery === 'Live', peer_count: delivery === 'Live' ? 1 : 0,
          active_path: path, delivery_state: delivery, last_error: timeout, configured_peer_count: 1 };
        const before = JSON.stringify(sync);
        const view = connectivityGuidance(sync, ready, t);
        const state = { Live: 'live', DurableReady: 'durable', DurableRecovering: 'recovering', Offline: 'offline' }[delivery];
        expect(view.label).toContain(t(`settings:connectionGuidance.states.${state}`));
        expect(view.tone).toBe(delivery === 'Live' ? 'accent' : 'warning');
        expect(view.errorSummary).toBe(t('settings:diagnostics.initialJoinTimeout'));
        expect(view.nextStep).not.toBe('');
        expect(view.label + view.description + view.errorSummary).not.toContain(timeout);
        if (delivery !== 'Live') expect(view.label).not.toMatch(/P2P|relay|中继|リレー/);
        expect(JSON.stringify(sync)).toBe(before);
      });
    }
  }
}

test('missing diagnostics distinguish initial, subscribed, unsubscribed and paused topics', async () => {
  const sync = await createDesktopMockApi().getSyncStatus();
  sync.subscribed_topics = ['general'];
  sync.gossip_disabled_topics = ['test'];
  expect(connectivityGuidance(sync, INITIAL_SYNC_STATUS_READ, t, { id: 'dev' }).label).toBe('Diagnostics not loaded');
  expect(connectivityGuidance(sync, ready, t, { id: 'general' }).label).toBe('Topic diagnostics pending');
  expect(connectivityGuidance(sync, ready, t, { id: 'dev' }).label).toBe('Not currently subscribed');
  expect(connectivityGuidance(sync, ready, t, { id: 'test' }).label).toBe('Live reception paused');
});

test('stale and unavailable reads do not pretend to be an unused topic', async () => {
  const sync = await createDesktopMockApi().getSyncStatus();
  const stale = connectivityGuidance(sync, { ...ready, error: true }, t);
  expect(stale.readError).toContain('previous snapshot');
  expect(stale.tone).toBe('warning');
  const failed = connectivityGuidance(sync, { ...INITIAL_SYNC_STATUS_READ, error: true }, t);
  expect(failed.label).toBe('Diagnostics not loaded');
  expect(failed.readError).toContain('could not be loaded');
});

test('discovery does not inherit the live topic result and unknown errors retain a neutral explanation', async () => {
  const sync = await createDesktopMockApi().getSyncStatus();
  sync.discovery.connected_peer_count = 0;
  sync.discovery.last_discovery_error = 'do not translate <raw> failure';
  const view = discoveryGuidance(sync, ready, t);
  expect(view.label).toBe('Waiting for discovery connections');
  expect(view.description).toContain('not individual topic delivery');
  expect(view.errorSummary).toBe(t('settings:diagnostics.error'));
  expect(sync.discovery.last_discovery_error).toBe('do not translate <raw> failure');
});

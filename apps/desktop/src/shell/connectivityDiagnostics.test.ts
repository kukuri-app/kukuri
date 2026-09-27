import { expect, test } from 'vitest';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { syncStatusBadgeLabel, topicConnectionLabel } from './presentation';

test('a historical error does not hide the current live connection', async () => {
  const status = await createDesktopMockApi().getSyncStatus();
  expect(syncStatusBadgeLabel({ ...status, connected: true, peer_count: 1,
    delivery_state: 'Live', last_error: 'timed out waiting for initial topic join' })).toBe('connected');
});

test('relay permission does not describe a disconnected topic as connected', async () => {
  const status = await createDesktopMockApi().getSyncStatus();
  const topic = { ...status.topic_diagnostics[0], joined: false, peer_count: 0,
    delivery_state: 'Offline' as const, active_path: 'relay_supported_p2p' as const };
  expect(topicConnectionLabel(topic)).toBe('idle');
});

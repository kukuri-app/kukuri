import { afterEach, expect, test, vi } from 'vitest';

import { invoke } from '@tauri-apps/api/core';

import { runtimeApi } from './runtimeApi';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}));

const invokeMock = vi.mocked(invoke);

afterEach(() => {
  invokeMock.mockClear();
  delete window.__KUKURI_DESKTOP__;
});

test('setTopicGossipEnabled invokes the desktop command', async () => {
  await runtimeApi.setTopicGossipEnabled('kukuri:topic:demo', false);
  expect(invokeMock).toHaveBeenCalledWith('set_topic_gossip_enabled', {
    request: { topic: 'kukuri:topic:demo', enabled: false },
  });
});

test('setChannelGossipEnabled invokes the desktop command', async () => {
  await runtimeApi.setChannelGossipEnabled('kukuri:topic:demo', 'channel-1', true);
  expect(invokeMock).toHaveBeenCalledWith('set_channel_gossip_enabled', {
    request: { topic: 'kukuri:topic:demo', channel: 'channel-1', enabled: true },
  });
});

test('searchCommunityNodeIndex invokes the typed index command', async () => {
  await runtimeApi.searchCommunityNodeIndex({
    base_url: 'https://node.example',
    query: 'hello',
    scope_kind: 'public_topic',
    scope_id: 'rust',
    limit: 20,
  });
  expect(invokeMock).toHaveBeenCalledWith('search_community_node_index', {
    request: {
      base_url: 'https://node.example',
      query: 'hello',
      scope_kind: 'public_topic',
      scope_id: 'rust',
      limit: 20,
    },
  });
});

test('resolveCommunityIndexPosts invokes the local canonical resolver', async () => {
  const entry = {
    key: 'public_topic:rust:post-1',
    topic: 'rust',
    object_id: 'post-1',
    author_pubkey: 'author-1',
    channel_ref: { kind: 'public' } as const,
  };

  await runtimeApi.resolveCommunityIndexPosts([entry]);

  expect(invokeMock).toHaveBeenCalledWith('resolve_community_index_posts', {
    request: { entries: [entry] },
  });
});

test('bookmarkPost preserves the target channel context', async () => {
  await runtimeApi.bookmarkPost('rust', 'post-1', {
    kind: 'private_channel',
    channel_id: 'channel-1',
  });

  expect(invokeMock).toHaveBeenCalledWith('bookmark_post', {
    request: {
      topic: 'rust',
      object_id: 'post-1',
      channel_ref: { kind: 'private_channel', channel_id: 'channel-1' },
    },
  });
});

test('submitCommunityNodeIndexingRequest invokes the typed indexing request command', async () => {
  await runtimeApi.submitCommunityNodeIndexingRequest({
    base_url: 'https://node.example',
    scope_kind: 'private_channel',
    topic_id: 'kukuri:topic:demo',
    channel_id: 'channel-1',
    confirm_private_channel_secret_disclosure: true,
  });
  expect(invokeMock).toHaveBeenCalledWith('submit_community_node_indexing_request', {
    request: {
      base_url: 'https://node.example',
      scope_kind: 'private_channel',
      topic_id: 'kukuri:topic:demo',
      channel_id: 'channel-1',
      confirm_private_channel_secret_disclosure: true,
    },
  });
  await runtimeApi.revokeCommunityNodeIndexingRequest({
    base_url: 'https://node.example',
    scope_kind: 'private_channel',
    topic_id: 'kukuri:topic:demo',
    channel_id: 'channel-1',
    confirm_private_channel_secret_disclosure: false,
  });
  expect(invokeMock).toHaveBeenLastCalledWith('revoke_community_node_indexing_request', {
    request: {
      base_url: 'https://node.example',
      scope_kind: 'private_channel',
      topic_id: 'kukuri:topic:demo',
      channel_id: 'channel-1',
      confirm_private_channel_secret_disclosure: false,
    },
  });
});

test('trust and relation reads invoke viewer-bound desktop commands', async () => {
  const request = {
    base_url: 'https://node.example',
    target_pubkey: 'a'.repeat(64),
  };

  await runtimeApi.readCommunityNodeTrustUser(request);
  expect(invokeMock).toHaveBeenLastCalledWith('read_community_node_trust_user', { request });

  await runtimeApi.readCommunityNodeRelationUser(request);
  expect(invokeMock).toHaveBeenLastCalledWith('read_community_node_relation_user', { request });

  await runtimeApi.listCommunityNodeRelationNeighbors({
    base_url: 'https://node.example',
    limit: 20,
  });
  expect(invokeMock).toHaveBeenLastCalledWith('list_community_node_relation_neighbors', {
    request: { base_url: 'https://node.example', limit: 20 },
  });
});

test('distance opt-out commands preserve the configured node target', async () => {
  await runtimeApi.getCommunityNodeRelationOptout('https://node.example');
  expect(invokeMock).toHaveBeenLastCalledWith('get_community_node_relation_optout', {
    request: { base_url: 'https://node.example' },
  });

  await runtimeApi.setCommunityNodeRelationOptout('https://node.example');
  expect(invokeMock).toHaveBeenLastCalledWith('set_community_node_relation_optout', {
    request: { base_url: 'https://node.example' },
  });

  await runtimeApi.clearCommunityNodeRelationOptout('https://node.example');
  expect(invokeMock).toHaveBeenLastCalledWith('clear_community_node_relation_optout', {
    request: { base_url: 'https://node.example' },
  });
});

test('readCommunityNodeIndexingStatus invokes the typed indexing status command', async () => {
  await runtimeApi.readCommunityNodeIndexingStatus({
    base_url: 'https://node.example',
    scope_kind: 'public_topic',
    topic_id: 'kukuri:topic:demo',
    channel_id: null,
    confirm_private_channel_secret_disclosure: false,
  });
  expect(invokeMock).toHaveBeenCalledWith('read_community_node_indexing_status', {
    request: {
      base_url: 'https://node.example',
      scope_kind: 'public_topic',
      topic_id: 'kukuri:topic:demo',
      channel_id: null,
      confirm_private_channel_secret_disclosure: false,
    },
  });
});

// WP-S5 characterization テスト(presentation.ts 純関数 contract ②)。
// 後続 WP-H6(shell の store スライス化・selector 化・prop drilling 除去)の安全網として、
// 「観測した現挙動」を生リテラルで固定する。このテストが落ちたら疑うべきは直近の変更で
// ありテストではない。ラベル関数の期待値は setup.ts が毎テスト locale='en' に固定する
// 前提の en 訳文直値(文言そのものが契約)。TZ 依存の日時整形のみ構造 assert に留める。
import { describe, expect, it } from 'vitest';

import type {
  AuthorSocialView,
  ChannelAccessTokenPreview,
  CommunityNodeConfig,
  CommunityNodeNodeStatus,
  JoinedPrivateChannelView,
  PostView,
  ReactionStateView,
} from '../lib/api/types';
import {
  audienceLabelForChannelRef,
  audienceLabelForTimelineScope,
  authorDisplayLabel,
  communityNodeAuthLabel,
  communityNodeConnectivityUrlsLabel,
  communityNodeConsentLabel,
  communityNodeConsentView,
  communityNodeNextStepLabel,
  communityNodeRetryAfterLabel,
  communityNodeSessionActivationLabel,
  communityNodeSessionPhaseLabel,
  formatBytes,
  formatCount,
  formatListLabel,
  isHex64,
  joinedChannelFromAccessTokenPreview,
  localizeAudienceLabel,
  mergeAuthorView,
  mergeCommunityNodeStatus,
  mergeCommunityNodeStatuses,
  mergeKnownAuthors,
  patchReactionStateIntoPosts,
  privateComposeTarget,
  privateTimelineScope,
  shortPubkey,
  syncCommunityNodeConfigWithStatus,
  upsertCommunityNodeStatus,
  upsertJoinedChannel,
} from './presentation';

function baseStatus(
  overrides: Partial<CommunityNodeNodeStatus> = {}
): CommunityNodeNodeStatus {
  return {
    base_url: 'https://node.example',
    auth_state: { authenticated: true },
    // #857: 既定はローカル同意済み。未同意ケースは overrides で明示する。
    local_consent: {
      records: [
        {
          policy_slug: 'terms',
          policy_version: 1,
          accepted_at: 1_700_000_000,
          language: 'ja',
          app_version: '0.1.8',
        },
      ],
      withdrawn_at: null,
    },
    invite_code_saved: false,
    restart_required: false,
    ...overrides,
  };
}

describe('mergeCommunityNodeStatus', () => {
  it('clears last_error when the node recovers (next reports null)', () => {
    const previous = baseStatus({ last_error: 'community node timeout' });
    const next = baseStatus({ last_error: null });

    const merged = mergeCommunityNodeStatus(previous, next);

    expect(merged.last_error).toBeNull();
  });

  it('keeps a fresh last_error while the node is still failing', () => {
    const previous = baseStatus({ last_error: 'old failure' });
    const next = baseStatus({ last_error: 'new failure' });

    const merged = mergeCommunityNodeStatus(previous, next);

    expect(merged.last_error).toBe('new failure');
  });

  it('preserves resolved URL fallbacks from the previous status', () => {
    const resolvedUrls = {
      public_base_url: 'https://node.example',
      connectivity_urls: ['https://node.example/connect'],
    };
    const previous = baseStatus({
      resolved_urls: resolvedUrls,
      last_error: 'old failure',
    });
    const next = baseStatus({ last_error: null });

    const merged = mergeCommunityNodeStatus(previous, next);

    expect(merged.resolved_urls).toEqual(resolvedUrls);
    expect(merged.last_error).toBeNull();
  });

  it('inherits the previous consent_state only while next is authenticated', () => {
    const previous = baseStatus({
      consent_state: { all_required_accepted: true, items: [] },
    });

    // 認証済みの next(consent_state 未報告)は previous へフォールバックする
    const authenticated = mergeCommunityNodeStatus(previous, baseStatus());
    expect(authenticated.consent_state).toEqual({ all_required_accepted: true, items: [] });

    // 未認証の next は previous を引き継がず next.consent_state ?? null → null
    const unauthenticated = mergeCommunityNodeStatus(
      previous,
      baseStatus({ auth_state: { authenticated: false } })
    );
    expect(unauthenticated.consent_state).toBeNull();

    // 未認証でも next 自身が consent_state を持てばそれがそのまま使われる
    const unauthenticatedWithOwn = mergeCommunityNodeStatus(
      previous,
      baseStatus({
        auth_state: { authenticated: false },
        consent_state: { all_required_accepted: false, items: [] },
      })
    );
    expect(unauthenticatedWithOwn.consent_state).toEqual({
      all_required_accepted: false,
      items: [],
    });
  });
});

// ---- ここから WP-S5 追記分(builder は入力 fixture 専用。期待値側は生リテラルで書く)----

function buildAuthor(
  overrides: Partial<AuthorSocialView> & { author_pubkey: string }
): AuthorSocialView {
  return {
    name: null,
    display_name: null,
    about: null,
    picture_asset: null,
    updated_at: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    friend_of_friend_via_pubkeys: [],
    muted: false,
    blocking: false,
    blocked_by: false,
    ...overrides,
  };
}

function buildJoinedChannel(
  overrides: Partial<JoinedPrivateChannelView> = {}
): JoinedPrivateChannelView {
  return {
    topic_id: 'topic-1',
    channel_id: 'channel-1',
    label: 'Channel One',
    creator_pubkey: 'creator-pubkey',
    owner_pubkey: 'owner-pubkey',
    joined_via_pubkey: null,
    audience_kind: 'invite_only',
    is_owner: false,
    current_epoch_id: 'epoch-1',
    archived_epoch_ids: [],
    sharing_state: 'open',
    rotation_required: false,
    participant_count: 1,
    stale_participant_count: 0,
    ...overrides,
  };
}

function buildPost(overrides: Partial<PostView> & { object_id: string }): PostView {
  return {
    envelope_id: `envelope-${overrides.object_id}`,
    author_pubkey: 'author-pubkey',
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    content: 'hello',
    content_status: 'Available',
    attachments: [],
    created_at: 1000,
    is_threadable: true,
    audience_label: 'Public',
    ...overrides,
  };
}

describe('mergeAuthorView', () => {
  it('prefers incoming fields and falls back to current for missing ones', () => {
    const current = buildAuthor({
      author_pubkey: 'author-a',
      name: 'alice',
      display_name: 'Alice',
      about: 'bio',
      following: true,
      muted: true,
      friend_of_friend_via_pubkeys: ['via-1'],
    });

    const merged = mergeAuthorView(current, {
      author_pubkey: 'author-a',
      display_name: 'Alice Renamed',
      followed_by: true,
    });
    expect(merged.display_name).toBe('Alice Renamed');
    expect(merged.followed_by).toBe(true);
    expect(merged.name).toBe('alice');
    expect(merged.about).toBe('bio');
    expect(merged.following).toBe(true);
    expect(merged.muted).toBe(true);
    expect(merged.friend_of_friend_via_pubkeys).toEqual(['via-1']);
  });

  it('fills defaults when there is no current view', () => {
    expect(mergeAuthorView(undefined, { author_pubkey: 'author-b', name: 'bob' })).toEqual({
      author_pubkey: 'author-b',
      name: 'bob',
      display_name: null,
      about: null,
      picture_asset: null,
      updated_at: null,
      following: false,
      followed_by: false,
      mutual: false,
      friend_of_friend: false,
      friend_of_friend_via_pubkeys: [],
      muted: false,
      blocking: false,
      blocked_by: false,
    });
  });

  it('lets an explicit incoming false overwrite current true (?? semantics)', () => {
    const current = buildAuthor({ author_pubkey: 'author-a', following: true });
    const merged = mergeAuthorView(current, { author_pubkey: 'author-a', following: false });
    expect(merged.following).toBe(false);
  });
});

describe('mergeKnownAuthors', () => {
  it('skips nullish entries and merges the rest into a copy without mutating input', () => {
    const current: Record<string, AuthorSocialView> = {
      'author-a': buildAuthor({ author_pubkey: 'author-a', name: 'alice' }),
    };

    const result = mergeKnownAuthors(current, [
      null,
      undefined,
      { author_pubkey: 'author-b', name: 'bob' },
      { author_pubkey: 'author-a', muted: true },
    ]);

    expect(Object.keys(result).sort()).toEqual(['author-a', 'author-b']);
    // 既存 author は上書き分だけ更新され、他フィールドは維持。新規 author は defaults 込み
    expect(result['author-a'].name).toBe('alice');
    expect(result['author-a'].muted).toBe(true);
    expect(result['author-b'].name).toBe('bob');
    expect(result['author-b'].following).toBe(false);
    // 入力 record は破壊されない
    expect(current['author-a'].muted).toBe(false);
    expect(current['author-b']).toBeUndefined();
  });
});

describe('mergeCommunityNodeStatuses', () => {
  it('keeps only nodes present in next, merging each with its previous status', () => {
    const resolvedUrls = {
      public_base_url: 'https://a.example',
      connectivity_urls: ['https://a.example/connect'],
    };
    const previous = [
      baseStatus({ base_url: 'https://a.example', resolved_urls: resolvedUrls }),
      baseStatus({ base_url: 'https://b.example' }),
    ];
    const next = [
      baseStatus({ base_url: 'https://c.example' }),
      baseStatus({ base_url: 'https://a.example' }),
    ];

    const merged = mergeCommunityNodeStatuses(previous, next);
    // next の並び順で、next に無い b は落ちる
    expect(merged.map((status) => status.base_url)).toEqual([
      'https://c.example',
      'https://a.example',
    ]);
    // previous があるものは resolved URL を引き継ぎ、無いものは merge の defaults
    expect(merged[1].resolved_urls).toEqual(resolvedUrls);
    expect(merged[0].session_phase).toBe('idle');
    expect(merged[0].retry_after).toBeNull();
  });
});

describe('upsertCommunityNodeStatus', () => {
  it('inserts or replaces by base_url, keeping merge fallbacks and sorting by base_url', () => {
    const current = [
      baseStatus({ base_url: 'https://c.example' }),
      baseStatus({ base_url: 'https://a.example' }),
    ];

    const inserted = upsertCommunityNodeStatus(current, baseStatus({ base_url: 'https://b.example' }));
    expect(inserted.map((status) => status.base_url)).toEqual([
      'https://a.example',
      'https://b.example',
      'https://c.example',
    ]);

    const replaced = upsertCommunityNodeStatus(
      current,
      baseStatus({ base_url: 'https://a.example', last_error: 'boom' })
    );
    expect(replaced.map((status) => status.base_url)).toEqual([
      'https://a.example',
      'https://c.example',
    ]);
    expect(replaced[0].last_error).toBe('boom');
  });
});

describe('syncCommunityNodeConfigWithStatus', () => {
  it('applies status resolved_urls to the matching node only', () => {
    const resolvedUrls = {
      public_base_url: 'https://a.example',
      connectivity_urls: ['https://a.example/connect'],
    };
    const config: CommunityNodeConfig = {
      nodes: [
        { base_url: 'https://a.example' },
        { base_url: 'https://b.example' },
      ],
    };
    const status = baseStatus({ base_url: 'https://a.example', resolved_urls: resolvedUrls });

    const synced = syncCommunityNodeConfigWithStatus(config, status);

    expect(synced.nodes[0]).toEqual({
      base_url: 'https://a.example',
      resolved_urls: resolvedUrls,
    });
    // base_url が一致しない node は変更されない
    expect(synced.nodes[1]).toEqual({ base_url: 'https://b.example' });
  });

  it('falls back to resolved_urls=null when both sides are missing', () => {
    const config: CommunityNodeConfig = { nodes: [{ base_url: 'https://a.example' }] };

    const synced = syncCommunityNodeConfigWithStatus(config, baseStatus({ base_url: 'https://a.example' }));

    expect(synced.nodes[0]).toEqual({
      base_url: 'https://a.example',
      resolved_urls: null,
    });
  });
});

describe('upsertJoinedChannel', () => {
  it('appends an unknown channel; a replaced channel moves to the end', () => {
    const channels = [
      buildJoinedChannel({ channel_id: 'channel-1', label: 'One' }),
      buildJoinedChannel({ channel_id: 'channel-2', label: 'Two' }),
    ];

    const appended = upsertJoinedChannel(
      channels,
      buildJoinedChannel({ channel_id: 'channel-3', label: 'Three' })
    );
    expect(appended.map((channel) => channel.channel_id)).toEqual([
      'channel-1',
      'channel-2',
      'channel-3',
    ]);

    const replaced = upsertJoinedChannel(
      channels,
      buildJoinedChannel({ channel_id: 'channel-1', label: 'One Renamed' })
    );
    expect(replaced.map((channel) => [channel.channel_id, channel.label])).toEqual([
      ['channel-2', 'Two'],
      ['channel-1', 'One Renamed'],
    ]);
  });
});

describe('joinedChannelFromAccessTokenPreview', () => {
  it('maps a grant preview to a friend_only channel with sponsor as joined_via', () => {
    const preview: ChannelAccessTokenPreview = {
      kind: 'grant',
      topic_id: 'topic-1',
      channel_id: 'channel-1',
      channel_label: 'Grant Channel',
      owner_pubkey: 'owner-pubkey',
      sponsor_pubkey: 'sponsor-pubkey',
      epoch_id: 'epoch-1',
    };

    expect(joinedChannelFromAccessTokenPreview(preview)).toEqual({
      topic_id: 'topic-1',
      channel_id: 'channel-1',
      label: 'Grant Channel',
      creator_pubkey: 'owner-pubkey',
      owner_pubkey: 'owner-pubkey',
      joined_via_pubkey: 'sponsor-pubkey',
      audience_kind: 'friend_only',
      is_owner: false,
      current_epoch_id: 'epoch-1',
      archived_epoch_ids: [],
      sharing_state: 'open',
      rotation_required: false,
      participant_count: 1,
      stale_participant_count: 0,
    });
  });

  // grant で骨格全体を固定済みのため、share / invite は kind ごとに異なるフィールドのみ固定する
  it('maps a share preview to friend_plus with participant_count 2', () => {
    const channel = joinedChannelFromAccessTokenPreview({
      kind: 'share',
      topic_id: 'topic-2',
      channel_id: 'channel-2',
      channel_label: 'Share Channel',
      owner_pubkey: 'owner-pubkey',
      sponsor_pubkey: 'sponsor-pubkey',
      epoch_id: 'epoch-2',
    });

    expect(channel.audience_kind).toBe('friend_plus');
    expect(channel.participant_count).toBe(2);
    expect(channel.creator_pubkey).toBe('owner-pubkey');
    expect(channel.joined_via_pubkey).toBe('sponsor-pubkey');
  });

  it('maps an invite preview to invite_only with inviter as creator (owner fallback)', () => {
    const preview: ChannelAccessTokenPreview = {
      kind: 'invite',
      topic_id: 'topic-3',
      channel_id: 'channel-3',
      channel_label: 'Invite Channel',
      owner_pubkey: 'owner-pubkey',
      inviter_pubkey: 'inviter-pubkey',
      epoch_id: 'epoch-3',
    };

    const channel = joinedChannelFromAccessTokenPreview(preview);
    expect(channel.audience_kind).toBe('invite_only');
    expect(channel.participant_count).toBe(1);
    expect(channel.creator_pubkey).toBe('inviter-pubkey');
    expect(channel.joined_via_pubkey).toBe('inviter-pubkey');

    // inviter 欠落時は owner が creator になり joined_via は null
    const fallback = joinedChannelFromAccessTokenPreview({
      ...preview,
      inviter_pubkey: undefined,
    });
    expect(fallback.creator_pubkey).toBe('owner-pubkey');
    expect(fallback.joined_via_pubkey).toBeNull();
  });
});

describe('patchReactionStateIntoPosts', () => {
  it('replaces reaction fields only on the post matching target_object_id', () => {
    const posts = [
      buildPost({ object_id: 'object-1' }),
      buildPost({ object_id: 'object-2', my_reactions: [] }),
    ];
    const reactionState: ReactionStateView = {
      target_object_id: 'object-2',
      source_replica_id: 'replica-1',
      reaction_summary: [
        { reaction_key_kind: 'emoji', normalized_reaction_key: 'emoji:+1', emoji: '👍', count: 3 },
      ],
      my_reactions: [
        { reaction_key_kind: 'emoji', normalized_reaction_key: 'emoji:+1', emoji: '👍' },
      ],
    };

    const patched = patchReactionStateIntoPosts(posts, reactionState);
    // 非対象 post は元のまま、対象 post は reaction 2 フィールドのみ置換
    expect(patched[0].reaction_summary).toBeUndefined();
    expect(patched[1].reaction_summary).toEqual([
      { reaction_key_kind: 'emoji', normalized_reaction_key: 'emoji:+1', emoji: '👍', count: 3 },
    ]);
    expect(patched[1].my_reactions).toEqual([
      { reaction_key_kind: 'emoji', normalized_reaction_key: 'emoji:+1', emoji: '👍' },
    ]);
    expect(patched[1].content).toBe('hello');
  });
});

describe('communityNodeConnectivityUrlsLabel', () => {
  it('joins resolved urls, else falls back by consent state and authentication', () => {
    const status = baseStatus({
      resolved_urls: {
        public_base_url: 'https://node.example',
        connectivity_urls: ['https://node.example/c1', 'https://node.example/c2'],
      },
    });
    expect(communityNodeConnectivityUrlsLabel(status)).toBe(
      'https://node.example/c1, https://node.example/c2'
    );
    // 以下は 同意未受諾 > 認証済み > それ以外、の優先順
    expect(
      communityNodeConnectivityUrlsLabel(
        baseStatus({ consent_state: { all_required_accepted: false, items: [] } })
      )
    ).toBe('pending consent acceptance');
    expect(communityNodeConnectivityUrlsLabel(baseStatus())).toBe('not resolved yet');
    expect(
      communityNodeConnectivityUrlsLabel(baseStatus({ auth_state: { authenticated: false } }))
    ).toBe('not resolved');
    expect(communityNodeConnectivityUrlsLabel(undefined)).toBe('not resolved');
  });
});

describe('communityNodeSessionPhaseLabel', () => {
  it('translates the session phase and falls back to unknown', () => {
    expect(communityNodeSessionPhaseLabel(undefined)).toBe('unknown');
    expect(communityNodeSessionPhaseLabel(baseStatus())).toBe('unknown');
    expect(communityNodeSessionPhaseLabel(baseStatus({ session_phase: 'ready' }))).toBe('ready');
    expect(communityNodeSessionPhaseLabel(baseStatus({ session_phase: 'retrying' }))).toBe(
      'retrying'
    );
  });
});

describe('communityNodeRetryAfterLabel', () => {
  it('falls back to none, else formats retry_after as a localized time', () => {
    expect(communityNodeRetryAfterLabel(undefined)).toBe('none');
    expect(communityNodeRetryAfterLabel(baseStatus())).toBe('none');

    const label = communityNodeRetryAfterLabel(
      baseStatus({ retry_after: Date.UTC(2026, 0, 1, 12, 34, 56) })
    );
    // ローカル TZ に依存して整形されるため文言は固定せず、
    // 「hh:mm:ss を含む時刻文字列になる」ことを契約として固定する。
    expect(label).toMatch(/\d{1,2}:\d{2}:\d{2}/);
  });
});

describe('communityNodeNextStepLabel', () => {
  it('walks the setup steps in priority order', () => {
    // #857: status 無し > ローカル未同意(認証より優先) > 未認証 > サーバ同意未受諾 > restart_required
    expect(communityNodeNextStepLabel(undefined)).toBe('save nodes to begin authentication');
    expect(
      communityNodeNextStepLabel(
        baseStatus({
          auth_state: { authenticated: false },
          local_consent: { records: [], withdrawn_at: null },
        })
      )
    ).toBe('accept required policies to resolve connectivity urls');
    expect(
      communityNodeNextStepLabel(baseStatus({ auth_state: { authenticated: false } }))
    ).toBe('authenticate this node');
    expect(
      communityNodeNextStepLabel(
        baseStatus({ consent_state: { all_required_accepted: false, items: [] } })
      )
    ).toBe('accept required policies to resolve connectivity urls');
    expect(communityNodeNextStepLabel(baseStatus({ restart_required: true }))).toBe(
      'unexpected restart requirement; refresh metadata or restart only as fallback'
    );
  });

  it('prioritizes an admission rejection even while a local token still looks valid', () => {
    // #708: 参加拒否後に端末側トークンが残っていても、次の操作は拒否の案内を優先する。
    expect(
      communityNodeNextStepLabel(
        baseStatus({
          auth_state: { authenticated: true, expires_at: 4102444800 },
          admission_rejection: { code: 'BANNED', message: 'node-local support denied' },
        })
      )
    ).toBe("Contact this node's operator if needed. Automatic retries are stopped.");
  });

  it('asks to refresh metadata until urls resolve, then reports active', () => {
    expect(communityNodeNextStepLabel(baseStatus())).toBe(
      'refresh metadata if connectivity urls stay unresolved'
    );
    expect(
      communityNodeNextStepLabel(
        baseStatus({
          resolved_urls: { public_base_url: 'https://node.example', connectivity_urls: [] },
        })
      )
    ).toBe('connectivity urls active on current session');
  });
});

describe('communityNodeSessionActivationLabel', () => {
  it('prioritizes restart_required over resolved urls', () => {
    const resolvedUrls = {
      public_base_url: 'https://node.example',
      connectivity_urls: ['https://node.example/c1'],
    };

    expect(
      communityNodeSessionActivationLabel(
        baseStatus({ restart_required: true, resolved_urls: resolvedUrls })
      )
    ).toBe('restart required (unexpected)');
    expect(communityNodeSessionActivationLabel(baseStatus({ resolved_urls: resolvedUrls }))).toBe(
      'active on current session'
    );
  });

  it('reports consent, metadata, and authentication states in order', () => {
    expect(communityNodeSessionActivationLabel(undefined)).toBe('unknown');
    expect(
      communityNodeSessionActivationLabel(
        baseStatus({ consent_state: { all_required_accepted: false, items: [] } })
      )
    ).toBe('waiting for consent acceptance');
    expect(communityNodeSessionActivationLabel(baseStatus())).toBe(
      'awaiting connectivity metadata'
    );
    expect(
      communityNodeSessionActivationLabel(baseStatus({ auth_state: { authenticated: false } }))
    ).toBe('not authenticated');
  });
});

describe('communityNodeAuthLabel', () => {
  it('shows yes with expires_at (or unknown) when authenticated, else no', () => {
    expect(
      communityNodeAuthLabel(baseStatus({ auth_state: { authenticated: true, expires_at: 1234 } }))
    ).toBe('yes (1234)');
    expect(communityNodeAuthLabel(baseStatus())).toBe('yes (unknown)');
    expect(communityNodeAuthLabel(baseStatus({ auth_state: { authenticated: false } }))).toBe(
      'no'
    );
    expect(communityNodeAuthLabel(undefined)).toBe('no');
  });
});

describe('communityNodeConsentLabel', () => {
  // #857: 同意状態はローカル同意記録が SSoT。
  it('maps local consent records to accepted / required / withdrawn', () => {
    expect(communityNodeConsentLabel(undefined)).toBe('required');
    expect(
      communityNodeConsentLabel(
        baseStatus({ local_consent: { records: [], withdrawn_at: null } })
      )
    ).toBe('required');
    expect(communityNodeConsentLabel(baseStatus())).toBe('accepted');
    const record = {
      policy_slug: 'terms',
      policy_version: 1,
      accepted_at: 1_750_000_000,
      language: 'ja',
      app_version: '0.1.8',
    };
    expect(
      communityNodeConsentLabel(
        baseStatus({ local_consent: { records: [record], withdrawn_at: null } })
      )
    ).toBe('accepted');
    expect(
      communityNodeConsentLabel(
        baseStatus({
          local_consent: { records: [record], withdrawn_at: null },
          consent_update_pending: true,
        })
      )
    ).toBe('required');
    expect(
      communityNodeConsentLabel(
        baseStatus({ local_consent: { records: [record], withdrawn_at: 1_750_000_001 } })
      )
    ).toBe('Consent withdrawn');
  });
});

describe('communityNodeConsentView', () => {
  const catalogEntry = (policies: Parameters<typeof communityNodeConsentView>[1]) => policies;

  it('returns an unloaded default view without a status or catalog', () => {
    expect(communityNodeConsentView(undefined, undefined)).toEqual({
      loaded: false,
      loading: false,
      loadError: null,
      withdrawn: false,
      hasLocalConsent: false,
      allRequiredAccepted: false,
      hasPendingUpdate: false,
      policies: [],
    });
  });

  it('surfaces catalog loading and load errors for retry', () => {
    expect(communityNodeConsentView(undefined, { status: 'loading' }).loading).toBe(true);
    const errored = communityNodeConsentView(undefined, { status: 'error', error: 'offline' });
    expect(errored.loaded).toBe(false);
    expect(errored.loadError).toBe('offline');
  });

  it('maps an accepted policy with a formatted acceptance timestamp', () => {
    const status = baseStatus({
      local_consent: {
        records: [
          {
            policy_slug: 'terms',
            policy_version: 3,
            accepted_at: 1750000000,
            language: 'ja',
            app_version: '0.1.8',
          },
        ],
        withdrawn_at: null,
      },
    });
    const view = communityNodeConsentView(
      status,
      catalogEntry({
        status: 'ok',
        policies: [
          {
            policy_slug: 'terms',
            policy_version: 3,
            title: 'Terms',
            body_markdown: 'Body text',
            required: true,
            is_current: true,
            effective_date: '2026-09-02',
            language: 'ja',
            reference_translation: false,
            fallback: false,
            material_change: false,
            requires_reconsent: false,
          },
        ],
      })
    );

    expect(view.loaded).toBe(true);
    expect(view.hasLocalConsent).toBe(true);
    expect(view.allRequiredAccepted).toBe(true);
    expect(view.hasPendingUpdate).toBe(false);
    expect(view.policies).toHaveLength(1);
    const policy = view.policies[0];
    expect(policy.policySlug).toBe('terms');
    expect(policy.title).toBe('Terms');
    expect(policy.body).toBe('Body text');
    expect(policy.policyVersion).toBe(3);
    expect(policy.effectiveDate).toBe('2026-09-02');
    expect(policy.language).toBe('ja');
    expect(policy.required).toBe(true);
    expect(policy.updated).toBe(false);
    // accepted_at は「秒」で持ち、ミリ秒換算して日時整形される(1750000000 秒 = 2025 年)。
    // TZ 依存の文言のため年のみ固定する(換算漏れなら 1970 年になる)。
    expect(policy.acceptedAtLabel).toMatch(/2025/);
  });

  it('flags a required re-acceptance as a pending update and ignores optional policies', () => {
    const status = baseStatus({
      local_consent: {
        records: [
          {
            policy_slug: 'privacy',
            policy_version: 1,
            accepted_at: 1750000000,
            language: 'ja',
            app_version: '0.1.8',
          },
        ],
        withdrawn_at: null,
      },
    });
    const view = communityNodeConsentView(
      status,
      catalogEntry({
        status: 'ok',
        policies: [
          {
            policy_slug: 'privacy',
            policy_version: 2,
            title: 'Privacy',
            body_markdown: 'Privacy body',
            required: true,
            is_current: true,
            reference_translation: false,
            fallback: false,
            material_change: false,
            requires_reconsent: false,
          },
          {
            policy_slug: 'optional',
            policy_version: 1,
            title: 'Optional',
            body_markdown: '',
            required: false,
            is_current: true,
            reference_translation: false,
            fallback: false,
            material_change: false,
            requires_reconsent: false,
          },
        ],
      })
    );

    expect(view.hasPendingUpdate).toBe(true);
    expect(view.allRequiredAccepted).toBe(false);
    // 旧版受諾ありの未受諾は「更新」、初回未受諾は更新ではない。
    expect(view.policies[0].updated).toBe(true);
    expect(view.policies[0].previouslyAcceptedVersion).toBe(1);
    expect(view.policies[0].acceptedAtLabel).toBeNull();
    expect(view.policies[1].updated).toBe(false);
  });

  it('requires re-acceptance when the catalog snapshot changes without a version bump', () => {
    const status = baseStatus({
      local_consent: {
        records: [
          {
            policy_slug: 'terms',
            policy_version: 1,
            policy_snapshot_revision: 'snapshot-1',
            accepted_at: 1750000000,
            language: 'ja',
            app_version: '0.1.8',
          },
        ],
        withdrawn_at: null,
      },
    });
    const view = communityNodeConsentView(
      status,
      catalogEntry({
        status: 'ok',
        policies: [
          {
            policy_slug: 'terms',
            policy_version: 1,
            policy_snapshot_revision: 'snapshot-2',
            title: 'Terms',
            body_markdown: 'Updated catalog facts',
            required: true,
            is_current: true,
            reference_translation: false,
            fallback: false,
            material_change: true,
            requires_reconsent: true,
          },
        ],
      })
    );

    expect(view.allRequiredAccepted).toBe(false);
    expect(view.hasPendingUpdate).toBe(true);
    expect(view.policies[0].previouslyAcceptedVersion).toBe(1);
  });

  it('treats withdrawn consent as not accepted while keeping history visible', () => {
    const status = baseStatus({
      local_consent: {
        records: [
          {
            policy_slug: 'terms',
            policy_version: 1,
            accepted_at: 1750000000,
            language: 'ja',
            app_version: '0.1.8',
          },
        ],
        withdrawn_at: 1750000001,
      },
    });
    const view = communityNodeConsentView(
      status,
      catalogEntry({
        status: 'ok',
        policies: [
          {
            policy_slug: 'terms',
            policy_version: 1,
            title: 'Terms',
            body_markdown: 'Body',
            required: true,
            is_current: true,
            reference_translation: false,
            fallback: false,
            material_change: false,
            requires_reconsent: false,
          },
        ],
      })
    );

    expect(view.withdrawn).toBe(true);
    expect(view.hasLocalConsent).toBe(false);
    expect(view.allRequiredAccepted).toBe(false);
  });
});

describe('privateTimelineScope', () => {
  it('builds a channel scope from a channel id, else the public scope', () => {
    expect(privateTimelineScope('channel-1')).toEqual({ kind: 'channel', channel_id: 'channel-1' });
    expect(privateTimelineScope(null)).toEqual({ kind: 'public' });
    // 空文字も falsy 扱いで public に落ちる(現挙動)
    expect(privateTimelineScope('')).toEqual({ kind: 'public' });
  });
});

describe('privateComposeTarget', () => {
  it('builds a private_channel ref from a channel id, else the public ref', () => {
    expect(privateComposeTarget('channel-1')).toEqual({
      kind: 'private_channel',
      channel_id: 'channel-1',
    });
    expect(privateComposeTarget(null)).toEqual({ kind: 'public' });
  });
});

describe('audienceLabelForChannelRef', () => {
  it('uses the joined channel label, falling back to Public / Private channel', () => {
    const joined = [buildJoinedChannel({ channel_id: 'channel-1', label: 'Ops Room' })];

    expect(
      audienceLabelForChannelRef({ kind: 'private_channel', channel_id: 'channel-1' }, joined)
    ).toBe('Ops Room');
    expect(audienceLabelForChannelRef({ kind: 'public' }, [])).toBe('Public');
    expect(
      audienceLabelForChannelRef({ kind: 'private_channel', channel_id: 'channel-x' }, [])
    ).toBe('Private channel');
  });
});

describe('audienceLabelForTimelineScope', () => {
  it('uses the joined channel label, falling back to All joined / Private channel / Public', () => {
    const joined = [buildJoinedChannel({ channel_id: 'channel-1', label: 'Ops Room' })];

    expect(
      audienceLabelForTimelineScope({ kind: 'channel', channel_id: 'channel-1' }, joined)
    ).toBe('Ops Room');
    expect(audienceLabelForTimelineScope({ kind: 'all_joined' }, [])).toBe('All joined');
    expect(audienceLabelForTimelineScope({ kind: 'channel', channel_id: 'channel-x' }, [])).toBe(
      'Private channel'
    );
    expect(audienceLabelForTimelineScope({ kind: 'public' }, [])).toBe('Public');
  });
});

describe('formatBytes', () => {
  it('formats B below 1 KB, KB below 1 MB, MB above, with en grouping', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1023)).toBe('1,023 B');
    expect(formatBytes(1024)).toBe('1.0 KB');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(1048576)).toBe('1.0 MB');
    expect(formatBytes(2 * 1024 * 1024 * 1024)).toBe('2,048.0 MB');
  });
});

describe('shortPubkey', () => {
  it('keeps the first 12 characters and returns short values as-is', () => {
    expect(shortPubkey('abcdef0123456789abcdef0123456789')).toBe('abcdef012345');
    expect(shortPubkey('abc')).toBe('abc');
  });
});

describe('authorDisplayLabel', () => {
  it('uses a localized anonymous label instead of exposing a public key', () => {
    expect(authorDisplayLabel('abcdef0123456789')).toBe('Unknown user');
    expect(authorDisplayLabel('abcdef0123456789', 'Alice', 'alice')).toBe('Alice');
  });
});

describe('isHex64', () => {
  it('accepts exactly 64 hex characters regardless of case', () => {
    expect(isHex64('a'.repeat(64))).toBe(true);
    expect(isHex64('A'.repeat(64))).toBe(true);
    expect(isHex64('0123456789abcdef'.repeat(4))).toBe(true);
    expect(isHex64('a'.repeat(63))).toBe(false);
    expect(isHex64('a'.repeat(65))).toBe(false);
    expect(isHex64('')).toBe(false);
    expect(isHex64(`g${'a'.repeat(63)}`)).toBe(false);
  });
});

describe('formatListLabel', () => {
  it('joins values with a comma and falls back to none for an empty list', () => {
    expect(formatListLabel(['alpha', 'beta'])).toBe('alpha, beta');
    expect(formatListLabel([])).toBe('none');
  });
});

describe('formatCount', () => {
  it('formats counts with en grouping', () => {
    expect(formatCount(0)).toBe('0');
    expect(formatCount(1234567)).toBe('1,234,567');
  });
});

describe('localizeAudienceLabel', () => {
  it('routes the three known labels through translations, passing others through', () => {
    // en では訳文が入力と同値(キー経由で解決される)
    expect(localizeAudienceLabel('Public')).toBe('Public');
    expect(localizeAudienceLabel('All joined')).toBe('All joined');
    expect(localizeAudienceLabel('Private channel')).toBe('Private channel');
    expect(localizeAudienceLabel('Ops Room')).toBe('Ops Room');
  });
});

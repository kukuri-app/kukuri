import type { ChannelAccessTokenKind } from '@/lib/api';
import { findNextExternalUrl } from '@/lib/externalUrls';
import { parseHashRouteLocation } from '@/shell/routes';

export type TopicLinkReference = {
  kind: 'topic';
  topic: string;
  route: string;
};

export type PostLinkReference = {
  kind: 'post';
  topic: string;
  threadId: string;
  focusObjectId: string | null;
  route: string;
};

export type LiveLinkReference = {
  kind: 'live';
  topic: string;
  channelId: string | null;
  sessionId: string;
  route: string;
};

export type GameLinkReference = {
  kind: 'game';
  topic: string;
  channelId: string | null;
  roomId: string;
  route: string;
};

export type ShareTokenReference = {
  kind: 'share_token';
  token: string;
  tokenKind: ChannelAccessTokenKind;
  metadata: ChannelAccessTokenMetadata;
};

export type ChannelAccessTokenMetadata = {
  kind: ChannelAccessTokenKind;
  topicId?: string | null;
  channelId?: string | null;
  channelLabel?: string | null;
  ownerPubkey?: string | null;
  inviterPubkey?: string | null;
  sponsorPubkey?: string | null;
  epochId?: string | null;
};

/** #1232 AC-4: カスタムリアクションのセット（`kukuri:reaction-set:<セットの blob hash>`）。押すと取り込む。 */
export type ReactionSetReference = {
  kind: 'reaction_set';
  setHash: string;
};

export type InternalSmartReference =
  | TopicLinkReference
  | PostLinkReference
  | LiveLinkReference
  | GameLinkReference
  | ShareTokenReference
  | ReactionSetReference;

export type MentionSegment = {
  kind: 'mention';
  pubkey: string;
  label: string;
};

export type SmartTextSegment =
  | {
      kind: 'text';
      text: string;
    }
  | {
      kind: 'reference';
      reference: InternalSmartReference;
    }
  | {
      kind: 'external_url';
      href: string;
    }
  | MentionSegment;

const TOPIC_PATTERN = /kukuri:topic:[A-Za-z0-9:_-]+/;
const ROUTE_PATTERN = /#\/(?:timeline|live|game)\?[^\s]+/;
const CHANNEL_ACCESS_PREVIEW_PATTERN = /kukuri:\/\/access-preview\?[^\s]+/;
// Rust の `custom_reaction_set_hashes_in_text` と同じ形（64 桁の小文字の 16 進で、英数字が続かない）。
const REACTION_SET_PATTERN = /kukuri:reaction-set:([0-9a-f]{64})(?![0-9A-Za-z])/;
const MAX_CHANNEL_ACCESS_PREVIEW_TOKEN_LENGTH = 16 * 1024;

// Internal mention token format: `@[label](pubkey)` where pubkey is 64 hex chars.
// The label disallows `]` and newlines so the token stays unambiguous to parse.
const MENTION_PATTERN_SINGLE = /@\[([^\]\n]+)\]\(([0-9a-fA-F]{64})\)/;
export const MENTION_PATTERN = /@\[([^\]\n]+)\]\(([0-9a-fA-F]{64})\)/g;

export function buildMentionToken(label: string, pubkey: string): string {
  const sanitized = label.replace(/[\]\n]/g, ' ').replace(/\s+/g, ' ').trim();
  const safeLabel = sanitized.length > 0 ? sanitized : shortenReferenceId(pubkey);
  return `@[${safeLabel}](${pubkey})`;
}

export function extractMentions(text: string): Array<{ pubkey: string; label: string }> {
  const results: Array<{ pubkey: string; label: string }> = [];
  MENTION_PATTERN.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = MENTION_PATTERN.exec(text)) !== null) {
    results.push({ label: match[1], pubkey: match[2] });
  }
  return results;
}

function buildRoute(pathname: string, params: URLSearchParams): string {
  const search = params.toString();
  return search ? `#${pathname}?${search}` : `#${pathname}`;
}

export function buildTopicLink(topic: string): string {
  const params = new URLSearchParams();
  params.set('topic', topic);
  return buildRoute('/timeline', params);
}

export function buildPostLink(
  topic: string,
  threadId: string,
  focusObjectId: string | null = null
): string {
  const params = new URLSearchParams();
  params.set('topic', topic);
  params.set('context', 'thread');
  params.set('threadId', threadId);
  if (focusObjectId) {
    params.set('focusObjectId', focusObjectId);
  }
  return buildRoute('/timeline', params);
}

export function buildLiveLink(
  topic: string,
  sessionId: string,
  channelId: string | null = null
): string {
  const params = new URLSearchParams();
  params.set('topic', topic);
  if (channelId) {
    params.set('channel', channelId);
  }
  params.set('sessionId', sessionId);
  return buildRoute('/live', params);
}

export function buildChannelAccessPreviewDeepLink(token: string): string {
  const params = new URLSearchParams();
  params.set('token', token);
  return `kukuri://access-preview?${params.toString()}`;
}

export function shortenReferenceId(value: string): string {
  const trimmed = value.trim();
  if (trimmed.length <= 18) {
    return trimmed;
  }
  return `${trimmed.slice(0, 10)}…`;
}

function tokenKindFromEnvelopeKind(kind: string | null | undefined): ChannelAccessTokenKind | null {
  if (kind === 'channel-invite') {
    return 'invite';
  }
  if (kind === 'channel-friend-grant') {
    return 'grant';
  }
  if (kind === 'channel-share') {
    return 'share';
  }
  return null;
}

function parseEnvelopeContent(value: unknown): Record<string, unknown> | null {
  if (!value) {
    return null;
  }
  if (typeof value === 'string') {
    try {
      const parsed = JSON.parse(value);
      return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : null;
    } catch {
      return null;
    }
  }
  return typeof value === 'object' ? (value as Record<string, unknown>) : null;
}

function stringField(content: Record<string, unknown> | null, key: string): string | null {
  const value = content?.[key];
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

export function parseChannelAccessTokenMetadata(rawValue: string): ChannelAccessTokenMetadata | null {
  const trimmed = rawValue.trim();
  if (!trimmed) {
    return null;
  }
  const legacyKind = trimmed.startsWith('invite:')
    ? 'invite'
    : trimmed.startsWith('grant:')
      ? 'grant'
      : trimmed.startsWith('share:')
        ? 'share'
        : null;
  if (legacyKind) {
    const lastSeparator = trimmed.lastIndexOf(':');
    const channelId = lastSeparator > -1 ? trimmed.slice(lastSeparator + 1).trim() : null;
    return {
      kind: legacyKind,
      channelId: channelId || null,
      channelLabel: channelId || null,
    };
  }
  if (!trimmed.startsWith('{')) {
    return null;
  }
  try {
    const parsed = JSON.parse(trimmed) as {
      envelope?: {
        kind?: string;
        pubkey?: string;
        content?: unknown;
        id?: string;
      };
    };
    const kind = tokenKindFromEnvelopeKind(parsed.envelope?.kind);
    if (!kind) {
      return null;
    }
    const content = parseEnvelopeContent(parsed.envelope?.content);
    return {
      kind,
      topicId: stringField(content, 'topic_id'),
      channelId: stringField(content, 'channel_id'),
      channelLabel: stringField(content, 'channel_label'),
      ownerPubkey: stringField(content, 'owner_pubkey'),
      inviterPubkey: kind === 'invite' ? parsed.envelope?.pubkey ?? null : null,
      sponsorPubkey:
        kind === 'share'
          ? stringField(content, 'sponsor_pubkey') ?? parsed.envelope?.pubkey ?? null
          : kind === 'grant'
            ? stringField(content, 'owner_pubkey')
            : null,
      epochId: stringField(content, 'epoch_id'),
    };
  } catch {
    return null;
  }
}

export function parseShareTokenKind(rawValue: string): ChannelAccessTokenKind | null {
  return parseChannelAccessTokenMetadata(rawValue)?.kind ?? null;
}

export function parseChannelAccessPreviewDeepLink(rawValue: string): ShareTokenReference | null {
  try {
    const url = new URL(rawValue.trim());
    if (
      url.protocol !== 'kukuri:' ||
      url.hostname !== 'access-preview' ||
      (url.pathname !== '' && url.pathname !== '/') ||
      url.hash ||
      url.username ||
      url.password ||
      url.port
    ) {
      return null;
    }
    const queryKeys = Array.from(url.searchParams.keys());
    if (queryKeys.length !== 1 || queryKeys[0] !== 'token') {
      return null;
    }
    const tokenValues = url.searchParams.getAll('token');
    if (tokenValues.length !== 1) {
      return null;
    }
    const token = tokenValues[0].trim();
    if (!token || token.length > MAX_CHANNEL_ACCESS_PREVIEW_TOKEN_LENGTH) {
      return null;
    }
    const tokenKind = parseShareTokenKind(token);
    if (!tokenKind) {
      return null;
    }
    return {
      kind: 'share_token',
      token,
      tokenKind,
      metadata: parseChannelAccessTokenMetadata(token) ?? { kind: tokenKind },
    };
  } catch {
    return null;
  }
}

export function parseInternalRouteLink(rawValue: string): InternalSmartReference | null {
  const { pathname, search } = parseHashRouteLocation(rawValue);
  const params = new URLSearchParams(search);
  const topic = params.get('topic')?.trim() ?? '';
  if (!topic) {
    return null;
  }
  if (pathname === '/timeline') {
    const context = params.get('context');
    const threadId = params.get('threadId')?.trim() ?? '';
    if (context === 'thread' && threadId) {
      const focusObjectId = params.get('focusObjectId')?.trim() || null;
      return {
        kind: 'post',
        topic,
        threadId,
        focusObjectId,
        route: rawValue,
      };
    }
    return {
      kind: 'topic',
      topic,
      route: rawValue,
    };
  }
  if (pathname === '/live') {
    const sessionId = params.get('sessionId')?.trim() ?? '';
    if (!sessionId) {
      return null;
    }
    return {
      kind: 'live',
      topic,
      channelId: params.get('channel')?.trim() || null,
      sessionId,
      route: rawValue,
    };
  }
  if (pathname === '/game') {
    const roomId = params.get('roomId')?.trim() ?? '';
    if (!roomId) {
      return null;
    }
    return {
      kind: 'game',
      topic,
      channelId: params.get('channel')?.trim() || null,
      roomId,
      route: rawValue,
    };
  }
  return null;
}

function parseTopicReference(rawTopic: string): TopicLinkReference {
  return {
    kind: 'topic',
    topic: rawTopic,
    route: buildTopicLink(rawTopic),
  };
}

function findNextReference(
  value: string,
  offset: number
): { index: number; length: number; segment: SmartTextSegment } | null {
  const remaining = value.slice(offset);
  const routeMatch = ROUTE_PATTERN.exec(remaining);
  const accessPreviewMatch = CHANNEL_ACCESS_PREVIEW_PATTERN.exec(remaining);
  const topicMatch = TOPIC_PATTERN.exec(remaining);
  const reactionSetMatch = REACTION_SET_PATTERN.exec(remaining);
  const mentionMatch = MENTION_PATTERN_SINGLE.exec(remaining);
  const externalUrl = findNextExternalUrl(value, offset);
  const candidates = [
    externalUrl
      ? {
          index: externalUrl.index,
          text: externalUrl.href,
          segment: {
            kind: 'external_url' as const,
            href: externalUrl.href,
          },
        }
      : null,
    accessPreviewMatch
      ? {
          index: offset + accessPreviewMatch.index,
          text: accessPreviewMatch[0],
          segment: segmentFromReference(
            parseChannelAccessPreviewDeepLink(accessPreviewMatch[0])
          ),
        }
      : null,
    routeMatch
      ? {
          index: offset + routeMatch.index,
          text: routeMatch[0],
          segment: segmentFromReference(parseInternalRouteLink(routeMatch[0])),
        }
      : null,
    topicMatch
      ? {
          index: offset + topicMatch.index,
          text: topicMatch[0],
          segment: segmentFromReference(parseTopicReference(topicMatch[0])),
        }
      : null,
    reactionSetMatch
      ? {
          index: offset + reactionSetMatch.index,
          text: reactionSetMatch[0],
          segment: segmentFromReference({ kind: 'reaction_set', setHash: reactionSetMatch[1] }),
        }
      : null,
    mentionMatch
      ? {
          index: offset + mentionMatch.index,
          text: mentionMatch[0],
          segment: {
            kind: 'mention' as const,
            label: mentionMatch[1],
            pubkey: mentionMatch[2],
          },
        }
      : null,
  ].filter(
    (
      candidate
    ): candidate is {
      index: number;
      text: string;
      segment: SmartTextSegment | null;
    } => candidate !== null
  );

  if (candidates.length === 0) {
    return null;
  }

  candidates.sort((left, right) => left.index - right.index);
  const next = candidates[0];
  if (!next.segment) {
    return null;
  }
  return {
    index: next.index,
    length: next.text.length,
    segment: next.segment,
  };
}

function segmentFromReference(
  reference: InternalSmartReference | null
): SmartTextSegment | null {
  return reference ? { kind: 'reference', reference } : null;
}

/** #1232 AC-4: DM の本文を、リアクションのセットの共有用の文字列（`setHash` を持つ）とそれ以外の文字列に分ける。 */
export function splitReactionSetLinks(text: string): Array<{ text: string; setHash: string | null }> {
  const parts: Array<{ text: string; setHash: string | null }> = [];
  let offset = 0;
  for (const match of text.matchAll(new RegExp(REACTION_SET_PATTERN, 'g'))) {
    parts.push({ text: text.slice(offset, match.index), setHash: null });
    parts.push({ text: match[0], setHash: match[1] });
    offset = match.index + match[0].length;
  }
  parts.push({ text: text.slice(offset), setHash: null });
  return parts;
}

export function parseSmartText(value: string): SmartTextSegment[][] {
  return value.split('\n').map((line) => {
    const trimmed = line.trim();
    const accessPreviewReference = parseChannelAccessPreviewDeepLink(trimmed);
    if (accessPreviewReference) {
      return [
        {
          kind: 'reference',
          reference: accessPreviewReference,
        },
      ] satisfies SmartTextSegment[];
    }

    const shareTokenKind = parseShareTokenKind(trimmed);
    if (shareTokenKind) {
      return [
        {
          kind: 'reference',
          reference: {
            kind: 'share_token',
            token: trimmed,
            tokenKind: shareTokenKind,
            metadata: parseChannelAccessTokenMetadata(trimmed) ?? { kind: shareTokenKind },
          },
        },
      ] satisfies SmartTextSegment[];
    }

    const segments: SmartTextSegment[] = [];
    let offset = 0;

    while (offset < line.length) {
      const next = findNextReference(line, offset);
      if (!next) {
        segments.push({
          kind: 'text',
          text: line.slice(offset),
        });
        break;
      }
      if (next.index > offset) {
        segments.push({
          kind: 'text',
          text: line.slice(offset, next.index),
        });
      }
      segments.push(next.segment);
      offset = next.index + next.length;
    }

    if (segments.length === 0) {
      segments.push({
        kind: 'text',
        text: '',
      });
    }
    return segments;
  });
}

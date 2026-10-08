import { describe, expect, test } from 'vitest';

import {
  buildChannelAccessPreviewDeepLink,
  parseChannelAccessPreviewDeepLink,
  parseSmartText,
} from './internalLinks';

describe('internal link parsing', () => {
  test('parses channel access preview deep links for known token kinds', () => {
    const reference = parseChannelAccessPreviewDeepLink(
      buildChannelAccessPreviewDeepLink('invite:kukuri:topic:demo:channel-1')
    );

    expect(reference).toMatchObject({
      kind: 'share_token',
      tokenKind: 'invite',
      token: 'invite:kukuri:topic:demo:channel-1',
    });
  });

  test('rejects unsupported or malformed channel access preview deep links', () => {
    expect(parseChannelAccessPreviewDeepLink('https://example.com/access-preview?token=invite:x')).toBeNull();
    expect(parseChannelAccessPreviewDeepLink('kukuri://timeline?token=invite:x')).toBeNull();
    expect(parseChannelAccessPreviewDeepLink('kukuri://access-preview/extra?token=invite:x')).toBeNull();
    expect(parseChannelAccessPreviewDeepLink('kukuri://access-preview?token=invite:x#fragment')).toBeNull();
    expect(parseChannelAccessPreviewDeepLink('kukuri://access-preview')).toBeNull();
    expect(parseChannelAccessPreviewDeepLink('kukuri://access-preview?token=unknown:x')).toBeNull();
    expect(parseChannelAccessPreviewDeepLink('kukuri://access-preview?token=invite:x&token=share:y')).toBeNull();
    expect(parseChannelAccessPreviewDeepLink('kukuri://access-preview?token=invite:x&debug=1')).toBeNull();
  });

  test('splits safe external HTTP URLs without swallowing surrounding punctuation', () => {
    expect(
      parseSmartText(
        'before https://example.test/path?q=one). after kukuri:topic:demo'
      )
    ).toEqual([
      [
        { kind: 'text', text: 'before ' },
        {
          kind: 'external_url',
          href: 'https://example.test/path?q=one',
        },
        { kind: 'text', text: '). after ' },
        {
          kind: 'reference',
          reference: {
            kind: 'topic',
            topic: 'kukuri:topic:demo',
            route: '#/timeline?topic=kukuri%3Atopic%3Ademo',
          },
        },
      ],
    ]);
  });

  test('keeps unsafe or unsupported URL-like values as text', () => {
    for (const value of [
      'javascript:alert(1)',
      'file:///tmp/private',
      'https://user:secret@example.test/path',
      'https://example.test\\@other.test',
      'https://',
    ]) {
      expect(parseSmartText(value)).toEqual([[{ kind: 'text', text: value }]]);
    }
  });

  // #1232 AC-4: 64 桁の小文字の 16 進のセットの hash だけを、取り込みの参照にする。
  test('parses reaction set links only for a full lowercase blob hash', () => {
    const hash = 'a'.repeat(64);
    const rest = ` next kukuri:reaction-set:${'A'.repeat(64)} kukuri:reaction-set:${hash}x`;
    expect(parseSmartText(`set kukuri:reaction-set:${hash}.${rest}`)).toEqual([
      [
        { kind: 'text', text: 'set ' },
        { kind: 'reference', reference: { kind: 'reaction_set', setHash: hash } },
        { kind: 'text', text: `.${rest}` },
      ],
    ]);
  });
});

// i18n キーパリティの凍結テスト(WP-S4 T5)。
//
// 全ロケールが en と同一のキー集合を持つことを固定する。fail した場合は
// 追加したキーを全ロケールへ反映すること(テストを緩めない)。
//
// 既知の限界:
// - locales/ に新しい JSON を置いても index.ts に import されなければ検出外
//   (ns 一致テストが import 漏れを部分的に緩和する)。
// - 将来 legal.json の段落構造(配列)をロケール間で意図的に変える場合は、
//   legal のみ配列を葉扱いにする設計変更が必要。
// - 複数形サフィックス(_one/_other 等)が導入された場合、CLDR カテゴリ差に
//   よる正当なキー差を誤検出し得る(現状は全ロケール未使用)。
import { describe, expect, test } from 'vitest';

import { SUPPORTED_LOCALES, resources } from './index';

const BASE_LOCALE = 'en';
const NAMESPACES = Object.keys(resources[BASE_LOCALE]).sort();
const OTHER_LOCALES = SUPPORTED_LOCALES.filter((locale) => locale !== BASE_LOCALE);

// 固有名詞、言語名、入力例、プロトコル形式は翻訳せず、そのまま提示する。
const INTENTIONALLY_SHARED_ENGLISH = new Set([
  'game:fields.placeholders.participants',
  'settings:appearance.languageOptions.en',
  'settings:communityNode.baseUrlsPlaceholder',
  'settings:connectivity.peerTicketPlaceholder',
  'settings:discovery.seedPeersPlaceholder',
  'settings:reactions.searchKeyPlaceholder',
  'settings:release.update.storeChannel',
  'shell:navigation.placeholder',
]);

type Json = string | number | boolean | null | Json[] | { [key: string]: Json };

// オブジェクトは再帰、配列は index 付き([i])で再帰、それ以外は葉。
function flattenKeys(value: Json, prefix: string): string[] {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => flattenKeys(item, `${prefix}[${index}]`));
  }
  if (typeof value === 'object' && value !== null) {
    return Object.entries(value).flatMap(([key, child]) =>
      flattenKeys(child, prefix ? `${prefix}.${key}` : key)
    );
  }
  return [prefix];
}

function flattenLeaves(value: Json, prefix: string): Array<[string, string]> {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => flattenLeaves(item, `${prefix}[${index}]`));
  }
  if (typeof value === 'object' && value !== null) {
    return Object.entries(value).flatMap(([key, child]) =>
      flattenLeaves(child, prefix ? `${prefix}.${key}` : key)
    );
  }
  return typeof value === 'string' ? [[prefix, value]] : [];
}

function placeholders(text: string): string[] {
  return [...text.matchAll(/\{\{(\w+)\}\}/g)].map((match) => match[1]).sort();
}

function naturalLanguage(text: string): string {
  return text.replace(/\{\{[^}]+\}\}/gu, '');
}

function qualifiedLeaves(locale: keyof typeof resources): Array<[string, string]> {
  return NAMESPACES.flatMap((namespace) =>
    flattenLeaves(resources[locale][namespace as keyof (typeof resources)['en']] as Json, '').map(
      ([key, value]) => [`${namespace}:${key}`, value] as [string, string]
    )
  );
}

test('every locale exposes the same namespaces as en', () => {
  for (const locale of OTHER_LOCALES) {
    expect(Object.keys(resources[locale]).sort()).toEqual(NAMESPACES);
  }
});

describe.each(OTHER_LOCALES)('locale %s', (locale) => {
  test.each(NAMESPACES)('namespace %s has the same key set as en', (namespace) => {
    const enKeys = new Set(
      flattenKeys(resources[BASE_LOCALE][namespace as keyof (typeof resources)['en']] as Json, '')
    );
    const localeKeys = new Set(
      flattenKeys(resources[locale][namespace as keyof (typeof resources)['en']] as Json, '')
    );
    const missing = [...enKeys].filter((key) => !localeKeys.has(key)).sort();
    const extra = [...localeKeys].filter((key) => !enKeys.has(key)).sort();
    expect(missing).toEqual([]);
    expect(extra).toEqual([]);
  });

  test.each(NAMESPACES)('namespace %s keeps en placeholders', (namespace) => {
    const enLeaves = new Map(
      flattenLeaves(resources[BASE_LOCALE][namespace as keyof (typeof resources)['en']] as Json, '')
    );
    const localeLeaves = new Map(
      flattenLeaves(resources[locale][namespace as keyof (typeof resources)['en']] as Json, '')
    );
    for (const [key, enValue] of enLeaves) {
      const localeValue = localeLeaves.get(key);
      if (localeValue === undefined) continue; // キー欠落は上のテストが報告する
      expect(placeholders(localeValue), `${namespace}:${key}`).toEqual(placeholders(enValue));
    }
  });
});

test.each(['en', 'zh-CN'] as const)('locale %s does not contain Japanese kana', (locale) => {
  const kana = /[\u3040-\u30ff]/u;
  const violations = NAMESPACES.flatMap((namespace) =>
    flattenLeaves(resources[locale][namespace as keyof (typeof resources)['en']] as Json, '')
      .filter(([, value]) => kana.test(value))
      .map(([key, value]) => `${namespace}:${key}=${value}`)
  );

  expect(violations).toEqual([]);
});

test('Japanese product UI does not contain known untranslated product terms', () => {
  const forbidden = [
    /Community Node/u,
    /Community Index/u,
    /\bcommunity node\b/iu,
    /\bTimeline\b/u,
    /\bExplore\b/u,
    /\bLive\b/u,
    /\bGame\b/u,
    /\bpeer ticket\b/iu,
    /\bdiscovery diagnostics\b/iu,
    /\bconsent\b/iu,
    /\bcapability\b/iu,
    /\bproximity\b/iu,
    /\bnode-local\b/iu,
    /\bidentity\b/iu,
    /\bruntime contract\b/iu,
  ];
  const violations = NAMESPACES.filter((namespace) => namespace !== 'legal').flatMap((namespace) =>
    flattenLeaves(resources.ja[namespace as keyof (typeof resources)['ja']] as Json, '')
      .filter(([, value]) => forbidden.some((pattern) => pattern.test(value)))
      .map(([key, value]) => `${namespace}:${key}=${value}`)
  );

  expect(violations).toEqual([]);
});

test.each([
  ['en', /\bauthors?\b/iu],
  ['ja', /著者/u],
  ['zh-CN', /作者/u],
] as const)('locale %s calls people users instead of authors', (locale, forbidden) => {
  const violations = qualifiedLeaves(locale)
    .filter(([, value]) => forbidden.test(naturalLanguage(value)))
    .map(([key, value]) => `${key}=${value}`);

  expect(violations).toEqual([]);
});

test.each([
  ['en', /\bidentity\b/iu],
  ['ja', /(?:\bidentity\b|アイデンティティ|身元情報)/iu],
  ['zh-CN', /\bidentity\b/iu],
] as const)('locale %s explains accounts without abstract identity copy', (locale, forbidden) => {
  const violations = qualifiedLeaves(locale)
    .filter(([, value]) => forbidden.test(naturalLanguage(value)))
    .map(([key, value]) => `${key}=${value}`);

  expect(violations).toEqual([]);
});

test('Japanese user-facing explanations do not expose implementation words', () => {
  const forbidden = [
    /\bnode\b/iu,
    /\bcontent\b/iu,
    /\broute(?:d)?\b/iu,
    /\bfollow-?up\b/iu,
    /\bemail\b/iu,
    /\bhandle\b/iu,
    /\bfollower\b/iu,
    /\bpolicy\b/iu,
    /\bdiscovery seeds?\b/iu,
    /\bendpoint id\b/iu,
    /\baddr hints?\b/iu,
    /\bsearch key\b/iu,
    /\bHosting\b/u,
    /\bspawn\b/iu,
    /\bheartbeat\b/iu,
    /\bowner\b/iu,
    /\bbudget\b/iu,
    /\bConnection(?:s)?\b/u,
    /\bcomponent(?:s)?\b/iu,
    /\brevision\b/iu,
    /\bresource\b/iu,
    /\bscene\b/iu,
    /\bGrace period\b/iu,
    /\bPersistent prop\b/iu,
    /\binteraction\b/iu,
    /\bFog\b/u,
    /\bmilli\b/iu,
    /\bmicro\b/iu,
    /\bmanifest\b/iu,
    /\bsnapshot\b/iu,
    /\bhost\b/iu,
    /\brigid body\b/iu,
    /\bguest prop\b/iu,
    /\bslot\b/iu,
    /\bblob\b/iu,
    /\bepoch\b/iu,
    /\bHUD\b/u,
    /\bGossip\b/iu,
    /\bDB open\b/iu,
    /\bkey\b/iu,
  ];
  const violations = qualifiedLeaves('ja')
    .filter(([key, value]) => {
      if (INTENTIONALLY_SHARED_ENGLISH.has(key)) return false;
      return forbidden.some((pattern) => pattern.test(naturalLanguage(value)));
    })
    .map(([key, value]) => `${key}=${value}`);

  expect(violations).toEqual([]);
});

test.each(SUPPORTED_LOCALES)('locale %s uses the typographic ellipsis', (locale) => {
  const violations = qualifiedLeaves(locale)
    .filter(([, value]) => value.includes('...'))
    .map(([key, value]) => `${key}=${value}`);

  expect(violations).toEqual([]);
});

test('English common errors use sentence case and terminal punctuation', () => {
  const violations = flattenLeaves(resources.en.common.errors as Json, '')
    .filter(([, value]) => !/^[A-Z]/u.test(value) || !/[.!?]$/u.test(value))
    .map(([key, value]) => `common:errors.${key}=${value}`);

  expect(violations).toEqual([]);
});

test.each(SUPPORTED_LOCALES)('locale %s keeps count-aware English noun keys pluralized', (locale) => {
  const requiredPluralKeys = [
    'common:feed.pendingPosts',
    'shell:context.threadSummary',
    'shell:messages.conversationCount',
    'shell:notifications.summary',
    'profile:overview.followedCount',
    'profile:overview.followingCount',
    'profile:overview.mutedCount',
    'metaverse:recovery.offlineCountdown',
    'metaverse:hosting.resyncResult',
    'metaverse:connections.topology',
    'metaverse:hud.knownPeers',
  ];
  const keys = new Set(qualifiedLeaves(locale).map(([key]) => key));
  const missing = requiredPluralKeys.flatMap((key) => {
    const one = `${key}_one`;
    const other = `${key}_other`;
    return [one, other].filter((candidate) => !keys.has(candidate));
  });

  expect(missing).toEqual([]);
});

test.each(OTHER_LOCALES)('locale %s does not leave English UI copy untranslated', (locale) => {
  const violations = NAMESPACES.flatMap((namespace) => {
    const enLeaves = new Map(
      flattenLeaves(resources[BASE_LOCALE][namespace as keyof (typeof resources)['en']] as Json, '')
    );
    return flattenLeaves(
      resources[locale][namespace as keyof (typeof resources)['en']] as Json,
      ''
    )
      .filter(([key, value]) => {
        const qualifiedKey = `${namespace}:${key}`;
        return (
          value === enLeaves.get(key) &&
          /[A-Za-z]{3}/u.test(value) &&
          !INTENTIONALLY_SHARED_ENGLISH.has(qualifiedKey)
        );
      })
      .map(([key, value]) => `${namespace}:${key}=${value}`);
  });

  expect(violations).toEqual([]);
});

test('i18n init namespaces match the bundled resources', () => {
  // index.ts の ns 配列と resources のキーがずれると、バンドル済みなのに
  // 参照されない(またはその逆の)名前空間が生まれる。
  const initNamespaces = [
    'common',
    'shell',
    'settings',
    'profile',
    'channels',
    'live',
    'game',
    'legal',
    'metaverse',
  ]
    .slice()
    .sort();
  expect(NAMESPACES).toEqual(initNamespaces);
});

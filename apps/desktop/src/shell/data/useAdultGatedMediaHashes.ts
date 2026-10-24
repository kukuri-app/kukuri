import { useEffect, useMemo, useState } from 'react';

import { isAdultLabeledPost, isGatingContentAdvisory } from '@/shell/media';
import {
  postGateableMediaHashes,
  resolvePostAdvisory,
  type TimelineContentAdvisoryIndex,
  type AdvisoryPostSource,
} from '@/shell/contentAdvisories';

const SETTLED_LOOKUP = { active: false, settled: {} };
const EMPTY_HASHES: string[] = [];

/// #1107: 表示設定 OFF の間にゲートする添付 blob hash を、表示中の投稿から集める。
///
/// blob は複数の投稿から参照されうる。ある投稿で成人向け(自己申告・advisory)と扱われた blob は、
/// 同じ blob を参照する advisory の無い投稿でも取得・表示しない。投稿単位で判定すると、
/// 同じ blob の表示と破棄が参照元ごとに入れ替わり、ちらつきと取得の繰り返しになる。
export function collectAdultGatedMediaHashes({
  posts,
  timelineContentAdvisories,
  additionalHashes,
}: {
  /// 表示経路に現れる投稿(タイムライン系と「見つける」の解決済み投稿)。
  posts: readonly AdvisoryPostSource[];
  timelineContentAdvisories: TimelineContentAdvisoryIndex;
  /// 投稿から導けないゲート対象(「見つける」の index 応答が持つ advisory など)。
  additionalHashes: readonly string[];
}): string[] {
  const hashes = new Set<string>();
  const add = (hash: string) => {
    const trimmed = hash.trim();
    if (trimmed) hashes.add(trimmed);
  };
  for (const hash of additionalHashes) add(hash);
  // 投稿が表示されていなくても、blob への advisory があればその blob はゲートする。
  for (const [key, entries] of Object.entries(timelineContentAdvisories)) {
    if (!key.startsWith('blob_cid:')) continue;
    if (entries.some((entry) => isGatingContentAdvisory(entry.advisory))) {
      add(key.slice('blob_cid:'.length));
    }
  }
  for (const post of posts) {
    // 照会中の扱いは取得側が別に持つ。ここでは確定した advisory だけを見る。
    if (
      !isAdultLabeledPost(post) &&
      !resolvePostAdvisory(post, timelineContentAdvisories, SETTLED_LOOKUP).advisory
    ) {
      continue;
    }
    for (const hash of postGateableMediaHashes(post)) add(hash);
  }
  return [...hashes].sort();
}

/// #1107: 表示設定 OFF の間のゲート対象 blob hash。
///
/// - OFF の間に一度ゲートした blob は、ON へ戻すまでゲートし続ける。ゲートの根拠になった投稿が
///   可視範囲から外れても、同じ blob を参照する別の投稿で表示・取得を再開しない。
/// - ON では空。ON へ戻した時点で保持していた集合も捨てる。
/// - 返す配列は内容が同じ間は同じ参照を保つ(取得・破棄の effect を不要に再実行しない)。
export function useAdultGatedMediaHashes({
  adultContentEnabled,
  posts,
  timelineContentAdvisories,
  additionalHashes,
}: {
  adultContentEnabled: boolean;
  posts: readonly AdvisoryPostSource[];
  timelineContentAdvisories: TimelineContentAdvisoryIndex;
  additionalHashes: readonly string[];
}): string[] {
  const current = useMemo(
    () =>
      adultContentEnabled
        ? EMPTY_HASHES
        : collectAdultGatedMediaHashes({ posts, timelineContentAdvisories, additionalHashes }),
    [additionalHashes, adultContentEnabled, posts, timelineContentAdvisories]
  );
  const [retained, setRetained] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    if (adultContentEnabled) {
      setRetained((previous) => (previous.size === 0 ? previous : new Set()));
      return;
    }
    setRetained((previous) => {
      if (current.every((hash) => previous.has(hash))) return previous;
      return new Set([...previous, ...current]);
    });
  }, [adultContentEnabled, current]);

  // 保持済みの集合へ現在の集合を合わせる(保持の反映は effect 後になるため、現在の集合も必ず含める)。
  // 内容が同じ間は同じ参照を返すため、署名から配列を組み立てる(hash は `|` を含まない)。
  const signature = adultContentEnabled
    ? ''
    : [...new Set([...retained, ...current])].sort().join('|');
  return useMemo(() => (signature ? signature.split('|') : EMPTY_HASHES), [signature]);
}

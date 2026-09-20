import type { AuthorTrustGate, PostView } from '@/lib/api';
import {
  INITIAL_TIMELINE_ADVISORY_LOOKUP_STATE,
  type TimelineAdvisoryLookupState,
  type TimelineContentAdvisoryIndex,
} from '@/shell/contentAdvisories';

/// メディア(blob object URL・非対応動画)(WP-H6 PR3 のドメインスライス)。
export type MediaSliceState = {
  mediaObjectUrls: Record<string, string | null>;
  // #1207: 失敗が確定した hash のうち、再取得を試行中のもの。失敗表示を残したまま操作を止める。
  mediaRetryingHashes: Record<string, true>;
  unsupportedVideoManifests: Record<string, true>;
  // #858: 成人向け表現の表示設定(既定 OFF)。canonical source は Rust 側の
  // ローカル JSON で、ここは表示・プリフェッチ判定用の mirror。
  adultContentEnabled: boolean;
  // #1052: 「見つける」Column が表示中の、ローカル解決済み(署名付き)投稿。
  // メディアのプリフェッチ対象と成人向け取得ゲートの判定に使う一時状態で、
  // 結果の失効・Column の終了で空になる(永続化しない)。
  communityIndexResolvedPosts: PostView[];
  // #1055: 設定済み Community Node の content advisory が付いた添付 blob hash のうち、
  // 表示設定 OFF でゲート中のもの。Rust 側取得ゲート(`blob_media_payload`)の client 側 mirror で、
  // プリフェッチの起点をどこに持つ表示経路からも要求しないために使う(ADR 0046 §6.2)。
  // 一時状態であり永続化しない(ADR 0028 §8.10)。
  advisoryGatedMediaHashes: string[];
  // #1056: タイムライン系の可視 subject を採用 node へ一括照会した結果(subject key → advisory)と、
  // 照会の進み具合。どちらも一時状態で永続化しない(ADR 0028 §8.10)。照会中の投稿のメディアは
  // 取得せずスケルトンにし、advisory の確定後に代替表示か通常表示へ切り替える。
  timelineContentAdvisories: TimelineContentAdvisoryIndex;
  timelineAdvisoryLookup: TimelineAdvisoryLookupState;
  // #1061: 採用 CN の信頼値による著者の表示判断（著者 pubkey → 判断）。一時状態で永続化しない。
  authorTrustGates: Record<string, AuthorTrustGate>;
};

export function createInitialMediaSlice(): MediaSliceState {
  return {
    mediaObjectUrls: {},
    mediaRetryingHashes: {},
    unsupportedVideoManifests: {},
    adultContentEnabled: false,
    communityIndexResolvedPosts: [],
    advisoryGatedMediaHashes: [],
    timelineContentAdvisories: {},
    timelineAdvisoryLookup: INITIAL_TIMELINE_ADVISORY_LOOKUP_STATE,
    authorTrustGates: {},
  };
}

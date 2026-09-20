import type * as React from 'react';

import type { AuthorTrustGateView } from '@/shell/authorTrustGates';
import type {
  AdvisorySubjectKind,
  AuthorSocialView,
  Basis,
  ChannelAudienceKind,
  ContentProvenance,
  PostView,
  SafetyCategory,
} from '@/lib/api';

export type TopicChannelSummary = {
  channelId: string;
  label: string;
  audienceKind: ChannelAudienceKind;
  active: boolean;
  // Whether this channel is currently connected to the gossip network.
  // Absent is treated as connected.
  gossipJoined?: boolean;
};

export type TopicDiagnosticSummary = {
  topic: string;
  active: boolean;
  publicActive?: boolean;
  removable: boolean;
  connectionLabel: string;
  peerCount: number | null;
  lastReceivedLabel: string;
  // Raw last-received timestamp used for "updated" sorting in the nav list.
  // Null/absent means nothing has been received yet.
  lastReceivedAt?: number | null;
  // Whether this topic is currently connected to the gossip network.
  // Absent is treated as connected.
  gossipJoined?: boolean;
  channels?: TopicChannelSummary[];
};

export type ComposerDraftAttachmentView = {
  key: string;
  label: string;
  mime: string;
  byteSizeLabel: string;
};

export type ComposerDraftMediaView = {
  id: string;
  sourceName: string;
  previewUrl: string;
  attachments: ComposerDraftAttachmentView[];
};

export type PostMediaView = {
  objectId: string;
  kind: 'image' | 'video' | null;
  statusLabel?: string | null;
  extraAttachmentCount: number;
  // #1056: `pending` は Community Node への advisory 照会が未決(取得しない。スケルトン表示)。
  state: 'loading' | 'ready' | 'unavailable' | 'gated' | 'pending';
  // #1207: `unavailable` のとき、利用者の明示再試行で取り直す hash と、その再試行中かどうか。
  retryHashes?: string[];
  retrying?: boolean;
  // #1055: `gated` の判定元。`advisory` は Community Node の推定(ADR 0046 §6)であり、
  // 投稿者の自己申告(`self_label`)とは文言を分ける。
  // #1107: `shared_media` は、この投稿自体は対象外だが、同じ blob が別の投稿で成人向けと
  // 扱われているためメディアだけを代替表示にしている。
  gatedBy?: 'self_label' | 'advisory' | 'shared_media';
  metaMime?: string | null;
  metaBytesLabel?: string | null;
  imagePreviewSrc?: string | null;
  imageGalleryItems?: Array<{
    hash: string;
    src: string | null;
    // #1207: 自動取得が上限に達した / その再取得を試行中。
    failed?: boolean;
    retrying?: boolean;
    mime: string;
    provenance?: ContentProvenance;
  }>;
  currentImageIndex?: number;
  videoPosterPreviewSrc?: string | null;
  videoPlaybackSrc?: string | null;
  // 動画添付を media として通報するときの識別子(video_manifest の blob ハッシュ。無ければ
  // poster のハッシュ)。観測元は `provenance` をそのまま使う(#697)。
  videoReportHash?: string | null;
  videoUnsupportedOnClient: boolean;
  videoProps?: React.VideoHTMLAttributes<HTMLVideoElement>;
  // どの source を正本とし、どの community node capability（media_cache 等）経由で
  // 観測したか。media cache 由来の通報ルーティングに使う。
  provenance?: ContentProvenance;
};

export type ReferencedAuthorMeta = {
  pubkey: string;
  label: string;
  picture?: string | null;
};

// Resolved author info for rendering a mention hover card (in the composer
// suggestion list and in rendered posts).
export type MentionAuthorView = {
  pubkey: string;
  label: string;
  displayName?: string | null;
  name?: string | null;
  aboutPreview?: string | null;
  picture?: string | null;
};

// A candidate offered while typing `@` in the composer.
export type MentionCandidate = {
  pubkey: string;
  label: string;
  displayName?: string | null;
  name?: string | null;
  about?: string | null;
  picture?: string | null;
};

/// #1055: 代替表示に出す content advisory の説明。Community Node の node-local な推定であり、
/// 投稿の canonical でも署名対象でもない(ADR 0046 §6.1)。断定表現にせず issuer を必ず伴う。
export type ContentAdvisoryView = {
  /// 発行 node の manifest `node_id`(署名鍵の x-only 公開鍵 hex)。
  issuerNodeId: string;
  /// index を返した node の base URL。manifest 名を引けなかった場合の表示と申し立てに使う。
  nodeBaseUrl: string;
  /// manifest から解決した表示名。取得できなければ null。
  nodeName?: string | null;
  category: SafetyCategory;
  label: string;
  confidence?: number | null;
  basis: Basis;
  /// 異議申し立ての対象になる risk signal id。
  signalId: string;
  subjectKind: AdvisorySubjectKind;
  subjectId: string;
};

export type PostCardView = {
  post: PostView;
  // index previewの表示用PostViewと、操作対象の正本PostViewを分離する。
  actionPost?: PostView | null;
  context: 'timeline' | 'thread';
  authorLabel: string;
  authorPicture?: string | null;
  relationshipLabel: string | null;
  audienceChipLabel?: string | null;
  threadTargetId: string;
  threadTopicId?: string | null;
  canOpenThread?: boolean;
  canReply?: boolean;
  canRepost?: boolean;
  canReact?: boolean;
  media: PostMediaView;
  repostSourceAuthor?: ReferencedAuthorMeta | null;
  replyParentAuthor?: ReferencedAuthorMeta | null;
  suppressReplyPreview?: boolean;
  showUnavailableDiagnostics?: boolean;
  // #1061: 採用 CN の信頼値による折りたたみ（著者または引用元の著者）。
  trustGate?: AuthorTrustGateView | null;
  // #858: 成人向けラベル付き投稿で、表示設定 OFF のため本文・メディアを代替表示にする。
  adultContentGated?: boolean;
  // #1055: 代替表示の根拠。両方あるときは投稿者自身の申告(`self_label`)を根拠として示す。
  gatedBy?: 'self_label' | 'advisory';
  // #1055: 代替表示の本文文言。未指定なら共通の「成人向け」文言を使う。canonical 解決が
  // 済んでいない entry で、待機中であることを保ったまま代替表示にするために使う。
  gatedBodyText?: string | null;
  // #1055: 成人向けゲートの根拠が Community Node の content advisory(ADR 0028 §8.6)である
  // 場合の説明。issuer / category / confidence / basis を出し、異議申し立てへ導線を持つ。
  // 投稿者の自己申告ラベルだけが根拠のときは undefined。
  contentAdvisory?: ContentAdvisoryView | null;
  mentionAuthors?: Record<string, MentionAuthorView>;
  // 正本（通常は author_docs）と、index / moderation / cache 等の観測経路を分離して保持する。
  // 通報ルーティング（#310）・content details・default node boundary 説明に使う。
  provenance?: ContentProvenance;
  // 投影など完全な PostView を持たない表示元では、実在する識別子だけを明示する。
  // 未指定は通常 PostView の post / envelope / author 識別子を使う。
  identifierCopy?: {
    postId?: string;
    envelopeId?: string;
    authorId?: string;
  };
  // 通常投稿以外の読み取り専用 surface が共通通報導線を使うための狭い上書き。
  reportSubjectKind?: 'post' | 'search_result' | 'recommendation';
  allowReadOnlyReport?: boolean;
};

export type ThreadPanelState = {
  selectedThreadId: string | null;
  summary: string;
  emptyCopy: string;
};

export type AuthorRelationshipSummary = {
  label: string | null;
  following: boolean;
  followedBy: boolean;
  mutual: boolean;
  friendOfFriend: boolean;
  muted: boolean;
  blocking?: boolean;
  blockedBy?: boolean;
  viaPubkeys: string[];
  isSelf: boolean;
  canFollow: boolean;
  followActionLabel: 'Follow' | 'Unfollow';
  muteActionLabel: 'Mute' | 'Unmute';
  blockActionLabel?: 'Block' | 'Unblock';
};

export type AuthorDetailView = {
  author: AuthorSocialView | null;
  displayLabel: string;
  pictureSrc?: string | null;
  summary: AuthorRelationshipSummary | null;
  canMessage?: boolean;
  authorError?: string | null;
  // profile の canonical source は author_docs。community node はあくまで観測経路であり
  // truth source ではないことを provenance で表す。
  provenance?: ContentProvenance;
};

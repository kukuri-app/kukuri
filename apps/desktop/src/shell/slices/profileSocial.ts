import { type ProfileConnectionsView } from '@/components/shell/types';
import type {
  AuthorSocialView,
  PostView,
  Profile,
  ProfileInput,
  TimelineCursor,
} from '@/lib/api';

import { type AsyncPanelState, DEFAULT_ASYNC_PANEL_STATE } from '@/shell/slices/shared';

/// プロフィール・作者詳細・ソーシャルグラフ(WP-H6 PR3 のドメインスライス)。

export type SocialConnectionsState = Record<ProfileConnectionsView, AuthorSocialView[]>;
export type KnownAuthorsByPubkey = Record<string, AuthorSocialView>;

export type ProfileSocialSliceState = {
  localProfile: Profile | null;
  profileTimeline: PostView[];
  profileTimelineNextCursor: TimelineCursor | null;
  profileTimelineWindowHeadCursor: TimelineCursor | null;
  profileTimelineLoadingMore: boolean;
  profileTimelineLoadMoreError: string | null;
  knownAuthorsByPubkey: KnownAuthorsByPubkey;
  socialConnections: SocialConnectionsState;
  socialConnectionsPanelState: AsyncPanelState;
  profileDraft: ProfileInput;
  profileDirty: boolean;
  profileError: string | null;
  profilePanelState: AsyncPanelState;
  profileHasLoaded: boolean;
  profileTimelineEvicted: boolean;
  profileRefreshing: boolean;
  profileSaveRevision: number;
  profileSaving: boolean;
  selectedAuthorPubkey: string | null;
  selectedAuthor: AuthorSocialView | null;
  selectedAuthorTimeline: PostView[];
  authorTimelinesByPubkey: Record<string, PostView[]>;
  authorTimelineNextCursorByPubkey: Record<string, TimelineCursor | null>;
  authorTimelineWindowHeadCursorByPubkey: Record<string, TimelineCursor | null>;
  authorTimelineLoadingMoreByPubkey: Record<string, boolean>;
  authorTimelineLoadMoreErrorsByPubkey: Record<string, string | null>;
  authorErrorsByPubkey: Record<string, string | null>;
  selectedAuthorTimelineNextCursor: TimelineCursor | null;
  selectedAuthorTimelineLoadingMore: boolean;
  authorError: string | null;
};

export const DEFAULT_SOCIAL_CONNECTIONS: SocialConnectionsState = {
  following: [],
  followed: [],
  muted: [],
  blocking: [],
};

export function createInitialProfileSocialSlice(): ProfileSocialSliceState {
  return {
    localProfile: null,
    profileTimeline: [],
    profileTimelineNextCursor: null,
    profileTimelineWindowHeadCursor: null,
    profileTimelineLoadingMore: false,
    profileTimelineLoadMoreError: null,
    knownAuthorsByPubkey: {},
    socialConnections: DEFAULT_SOCIAL_CONNECTIONS,
    socialConnectionsPanelState: DEFAULT_ASYNC_PANEL_STATE,
    profileDraft: {},
    profileDirty: false,
    profileError: null,
    profilePanelState: DEFAULT_ASYNC_PANEL_STATE,
    profileHasLoaded: false,
    profileTimelineEvicted: false,
    profileRefreshing: false,
    profileSaveRevision: 0,
    profileSaving: false,
    selectedAuthorPubkey: null,
    selectedAuthor: null,
    selectedAuthorTimeline: [],
    authorTimelinesByPubkey: {},
    authorTimelineNextCursorByPubkey: {},
    authorTimelineWindowHeadCursorByPubkey: {},
    authorTimelineLoadingMoreByPubkey: {},
    authorTimelineLoadMoreErrorsByPubkey: {},
    authorErrorsByPubkey: {},
    selectedAuthorTimelineNextCursor: null,
    selectedAuthorTimelineLoadingMore: false,
    authorError: null,
  };
}

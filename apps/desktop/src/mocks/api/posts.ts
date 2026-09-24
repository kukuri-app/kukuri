import {
  type AttachmentView,
  type BookmarkedPostView,
  type DesktopApi,
  type TimelineScope,
} from '@/lib/api';

import {
  cloneBookmarkedPost,
  filterChannelScopedItems,
  withSocialPostDefaults,
} from '../desktopMockModel';
import { type MockRuntime } from '../mockRuntime';

type PostsMock = Pick<
  DesktopApi,
  | 'createPost'
  | 'createRepost'
  | 'withdrawPost'
  | 'listTimeline'
  | 'listThread'
  | 'listProfileTimeline'
  | 'listBookmarkedPostsPage'
  | 'bookmarkedPostIds'
  | 'bookmarkPost'
  | 'removeBookmarkedPost'
  | 'resolveCommunityIndexPosts'
  | 'retryPostElements'
>;

export function createPostsMock(runtime: MockRuntime): PostsMock {
  const {
    assistPeerIds,
    postsByTopic,
    authorProfileTimelines,
    syncStatus,
    bookmarkedPosts,
    visibleTimelineItems,
    isVisiblePost,
    withCurrentRelationship,
  } = runtime;
  // 新しく作る投稿の時刻。clockBase 未指定なら従来どおり連番そのもの。
  const postedAt = () => (runtime.options?.clockBase ?? 0) + runtime.sequence;
  // 自分が作った投稿には、実アプリと同じく自分のプロフィールの名前を載せる。
  // myProfile は再代入されるので、作成のたびに runtime から読む。名前が未設定のときは
  // 項目自体を載せず、従来の投稿と同じ形にする (null を明示すると既存の表示が変わる)。
  const selfAuthor = () => {
    const { name, display_name: displayName } = runtime.myProfile;
    return {
      ...(name != null ? { author_name: name } : {}),
      ...(displayName != null ? { author_display_name: displayName } : {}),
    };
  };

  return {
    async createPost(topic, content, replyTo, attachments, channelRef = { kind: 'public' }) {
      runtime.sequence += 1;
      const objectId = `${topic}-${runtime.sequence}`;
      const posts = postsByTopic[topic] ?? [];
      const channelId = channelRef.kind === 'private_channel' ? channelRef.channel_id : null;
      const rootId = replyTo
        ? posts.find((post) => post.object_id === replyTo)?.root_id ?? replyTo
        : objectId;
      const postAttachments: AttachmentView[] = (attachments ?? []).map((attachment, index) => ({
        hash: `${objectId}-attachment-${index}`,
        mime: attachment.mime,
        bytes: attachment.byte_size,
        role: attachment.role ?? 'image_original',
        status: 'Available',
      }));
      postsByTopic[topic] = [
        withSocialPostDefaults({
          object_id: objectId,
          envelope_id: `envelope-${runtime.sequence}`,
          author_pubkey: syncStatus.local_author_pubkey,
          ...selfAuthor(),
          following: false,
          followed_by: false,
          mutual: false,
          friend_of_friend: false,
          object_kind: replyTo ? 'comment' : 'post',
          content,
          content_status: 'Available',
          attachments: postAttachments,
          created_at: postedAt(),
          reply_to: replyTo ?? null,
          root_id: rootId,
          origin_topic_id: topic,
          channel_id: channelId,
          audience_label: channelId ? 'Private channel' : 'Public',
        }),
        ...posts,
      ];
      if (!channelId) {
        authorProfileTimelines[syncStatus.local_author_pubkey] = [
          withSocialPostDefaults({
            object_id: objectId,
            envelope_id: objectId,
            author_pubkey: syncStatus.local_author_pubkey,
            ...selfAuthor(),
            following: false,
            followed_by: false,
            mutual: false,
            friend_of_friend: false,
            object_kind: replyTo ? 'comment' : 'post',
            content,
            content_status: 'Available',
            attachments: postAttachments,
            created_at: postedAt(),
            reply_to: replyTo ?? null,
            root_id: rootId,
            origin_topic_id: topic,
            channel_id: null,
            audience_label: 'Public',
          }),
          ...(authorProfileTimelines[syncStatus.local_author_pubkey] ?? []).filter(
            (post) => post.object_id !== objectId
          ),
        ];
      }
      syncStatus.subscribed_topics = Array.from(new Set([...syncStatus.subscribed_topics, topic]));
      if (!syncStatus.topic_diagnostics.some((entry) => entry.topic === topic)) {
        syncStatus.topic_diagnostics.push({
          topic,
          joined: true,
          delivery_state: 'Live',
          peer_count: 1,
          connected_peers: ['peer-a'],
          docs_assist_peer_ids: assistPeerIds,
          configured_peer_ids: ['peer-a'],
          missing_peer_ids: [],
          active_path: 'direct_p2p',
          rendezvous_peer_ids: [],
          fallback_peer_ids: [],
          last_received_at: runtime.sequence,
          last_docs_activity_at: runtime.sequence,
          status_detail: 'Connected to all configured peers for this topic',
          last_error: null,
        });
      }
      return objectId;
    },
    async createRepost(topic, sourceTopic, sourceObjectId, commentary) {
      runtime.sequence += 1;
      const objectId = `${topic}-repost-${runtime.sequence}`;
      const sourcePost = (postsByTopic[sourceTopic] ?? []).find((post) => post.object_id === sourceObjectId);
      if (!sourcePost || sourcePost.channel_id) {
        throw new Error('only public posts and comments can be reposted');
      }
      const normalizedCommentary = commentary?.trim() ? commentary.trim() : null;
      if (!normalizedCommentary) {
        const existing = (postsByTopic[topic] ?? []).find(
          (post) =>
            post.object_kind === 'repost' &&
            post.author_pubkey === syncStatus.local_author_pubkey &&
            post.repost_of?.source_object_id === sourceObjectId &&
            !post.repost_commentary
        );
        if (existing) {
          return existing.object_id;
        }
      }
      const repost = withSocialPostDefaults({
        object_id: objectId,
        envelope_id: `envelope-${runtime.sequence}`,
        author_pubkey: syncStatus.local_author_pubkey,
        ...selfAuthor(),
        following: false,
        followed_by: false,
        mutual: false,
        friend_of_friend: false,
        object_kind: 'repost',
        content: normalizedCommentary ?? '',
        content_status: 'Available',
        attachments: [],
        created_at: postedAt(),
        reply_to: null,
        root_id: null,
        published_topic_id: topic,
        origin_topic_id: topic,
        repost_of: {
          source_object_id: sourceObjectId,
          source_topic_id: sourceTopic,
          source_author_pubkey: sourcePost.author_pubkey,
          source_author_name: sourcePost.author_name ?? null,
          source_author_display_name: sourcePost.author_display_name ?? null,
          source_object_kind: sourcePost.object_kind,
          content: sourcePost.content,
          attachments: sourcePost.attachments.map((attachment) => ({ ...attachment })),
          reply_to: sourcePost.reply_to ?? null,
          root_id: sourcePost.root_id ?? null,
        },
        repost_commentary: normalizedCommentary,
        is_threadable: Boolean(normalizedCommentary),
        channel_id: null,
        audience_label: 'Public',
      });
      postsByTopic[topic] = [repost, ...(postsByTopic[topic] ?? [])];
      authorProfileTimelines[syncStatus.local_author_pubkey] = [
        repost,
        ...(authorProfileTimelines[syncStatus.local_author_pubkey] ?? []).filter(
          (post) => post.object_id !== objectId
        ),
      ];
      return objectId;
    },
    async resolveCommunityIndexPosts(entries) {
      return {
        entries: entries.map((entry) => {
          const post = (postsByTopic[entry.topic] ?? []).find(
            (candidate) =>
              candidate.object_id === entry.object_id &&
              candidate.author_pubkey === entry.author_pubkey &&
              (entry.channel_ref.kind === 'public'
                ? !candidate.channel_id
                : candidate.channel_id === entry.channel_ref.channel_id)
          );
          if (!post) {
            return {
              key: entry.key,
              post: null,
              capabilities: {
                open_thread: false,
                reply: false,
                repost: false,
                quote_repost: false,
                react: false,
                copy_link: false,
                bookmark: false,
                withdraw: false,
              },
            };
          }
          const active = !post.withdrawal;
          const postOrComment = post.object_kind === 'post' || post.object_kind === 'comment';
          const isPublic = !post.channel_id;
          return {
            key: entry.key,
            post: withSocialPostDefaults({ ...post }),
            capabilities: {
              open_thread: active && post.is_threadable,
              reply: active && post.is_threadable,
              repost: active && isPublic && postOrComment,
              quote_repost: active && isPublic && postOrComment,
              react: active && postOrComment,
              copy_link: true,
              bookmark: active,
              withdraw: active && post.author_pubkey === syncStatus.local_author_pubkey,
            },
          };
        }),
      };
    },
    async withdrawPost(topic, objectId) {
      const withdrawal = {
        withdrawn_at: ++runtime.sequence,
        replacement_object_id: null,
        reason_visibility: 'public',
        reason: 'author_request',
      };
      const applyWithdrawal = (post: (typeof postsByTopic)[string][number]) =>
        post.object_id === objectId
          ? {
              ...post,
              withdrawal,
              content: '',
              attachments: [],
              repost_of: null,
              repost_commentary: null,
              is_threadable: false,
            }
          : post;
      postsByTopic[topic] = (postsByTopic[topic] ?? []).map(applyWithdrawal);
      authorProfileTimelines[syncStatus.local_author_pubkey] = (
        authorProfileTimelines[syncStatus.local_author_pubkey] ?? []
      ).map(applyWithdrawal);
      return `withdrawal-${objectId}`;
    },
    async listTimeline(topic, _cursor, _limit, scope: TimelineScope = { kind: 'public' }) {
      syncStatus.subscribed_topics = Array.from(new Set([...syncStatus.subscribed_topics, topic]));
      if (!syncStatus.topic_diagnostics.some((entry) => entry.topic === topic)) {
        syncStatus.topic_diagnostics.push({
          topic,
          joined: false,
          delivery_state: assistPeerIds.length > 0 ? 'DurableRecovering' : 'Offline',
          peer_count: 0,
          connected_peers: [],
          docs_assist_peer_ids: assistPeerIds,
          configured_peer_ids: [],
          missing_peer_ids: [],
          active_path: 'direct_p2p',
          rendezvous_peer_ids: [],
          fallback_peer_ids: [],
          last_received_at: null,
          last_docs_activity_at: null,
          status_detail:
            assistPeerIds.length > 0
              ? `docs-assisted recovery is in progress via ${assistPeerIds.length} peer(s); live topic delivery is unavailable`
              : 'No peers configured for this topic',
          last_error: null,
        });
      }
      return {
        items: visibleTimelineItems(
          filterChannelScopedItems(postsByTopic[topic] ?? [], scope)
        ),
        next_cursor: null,
      };
    },
    async listThread(topic, threadId) {
      const posts = postsByTopic[topic] ?? [];
      return {
        items: visibleTimelineItems(
          posts.filter((post) => post.root_id === threadId || post.object_id === threadId)
        ),
        next_cursor: null,
      };
    },
    async listProfileTimeline(pubkey) {
      return {
        items: visibleTimelineItems([...(authorProfileTimelines[pubkey] ?? [])]),
        next_cursor: null,
      };
    },
    async retryPostElements(objectId) {
      for (const posts of Object.values(postsByTopic)) {
        const post = posts.find((candidate) => candidate.object_id === objectId);
        if (post) return withSocialPostDefaults({ ...post });
      }
      return null;
    },
    async listBookmarkedPostsPage(cursor, before = false) {
      const rows = bookmarkedPosts.filter((item) => isVisiblePost(item.post));
      const compare = (item: BookmarkedPostView) =>
        cursor
          ? item.bookmarked_at - cursor.bookmarked_at ||
            item.post.object_id.localeCompare(cursor.source_object_id)
          : -1;
      const candidates = cursor
        ? rows.filter((item) => before ? compare(item) > 0 : compare(item) < 0)
        : rows;
      const selected = before ? candidates.slice(-21) : candidates.slice(0, 21);
      const more = selected.length > 20;
      const page = before ? selected.slice(-20) : selected.slice(0, 20);
      const cursorFor = (item: BookmarkedPostView) => ({
        bookmarked_at: item.bookmarked_at,
        source_object_id: item.post.object_id,
      });
      return {
        items: page.map((item) => cloneBookmarkedPost({ ...item, post: withCurrentRelationship(item.post) })),
        newer_cursor: before ? more && page[0] ? cursorFor(page[0]) : null : cursor && page[0] ? cursorFor(page[0]) : null,
        older_cursor: before ? cursor && page.at(-1) ? cursorFor(page.at(-1)!) : null : more && page.at(-1) ? cursorFor(page.at(-1)!) : null,
      };
    },
    async bookmarkedPostIds(objectIds) {
      const ids = new Set(objectIds);
      return bookmarkedPosts.filter((item) => ids.has(item.post.object_id)).map((item) => item.post.object_id);
    },
    async bookmarkPost(topic, objectId) {
      const existing = bookmarkedPosts.find((item) => item.post.object_id === objectId);
      if (existing) {
        return cloneBookmarkedPost(existing);
      }
      const post = (postsByTopic[topic] ?? []).find((candidate) => candidate.object_id === objectId);
      if (!post) {
        throw new Error('bookmark target was not found');
      }
      const bookmarked: BookmarkedPostView = {
        bookmarked_at: Date.now(),
        post: withSocialPostDefaults({
          ...post,
          attachments: post.attachments.map((attachment) => ({ ...attachment })),
          repost_of: post.repost_of
            ? {
                ...post.repost_of,
                attachments: post.repost_of.attachments.map((attachment) => ({ ...attachment })),
              }
            : null,
        }),
      };
      bookmarkedPosts.unshift(bookmarked);
      bookmarkedPosts.sort(
        (left, right) =>
          right.bookmarked_at - left.bookmarked_at ||
          right.post.object_id.localeCompare(left.post.object_id)
      );
      return cloneBookmarkedPost(bookmarked);
    },
    async removeBookmarkedPost(objectId) {
      const index = bookmarkedPosts.findIndex((item) => item.post.object_id === objectId);
      if (index >= 0) {
        bookmarkedPosts.splice(index, 1);
      }
    },
  };
}

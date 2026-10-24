import type { ContentAdvisory, PostView } from '@/lib/api';
import { isGatingContentAdvisory } from '@/shell/media';

/// #1056: タイムライン系の表示経路で使う content advisory(ADR 0046 §6.1 / §6.3)。
///
/// 一括照会の応答は subject(post id / blob hash)単位で保持する一時状態であり、永続化しない。
/// advisory は投稿の canonical でも署名対象でもないため、`content_labels` へは書き戻さない。
export type TimelineContentAdvisory = {
  advisory: ContentAdvisory;
  /// 照会した(= 発行した)設定済み node の base URL。表示名の解決と申し立てに使う。
  nodeBaseUrl: string;
};

/// subject key → 採用済み advisory。
export type TimelineContentAdvisoryIndex = Record<string, TimelineContentAdvisory[]>;
/// 照会の進み具合。`active` の間は、`settled` に無い subject を照会中(未決)とみなす。
/// 描画と同じ計算で判定できるため、照会の開始を待たずに取得を止められる。
export type TimelineAdvisoryLookupState = {
  /// 照会先がある、または照会先の有無が未確定(起動直後)。
  active: boolean;
  /// 照会が終わった(応答・失敗・上限時間)subject key。
  settled: Record<string, true>;
};

export const INITIAL_TIMELINE_ADVISORY_LOOKUP_STATE: TimelineAdvisoryLookupState = {
  active: true,
  settled: {},
};

export function isAdvisorySubjectPending(
  lookup: TimelineAdvisoryLookupState,
  key: string
): boolean {
  return lookup.active && !lookup.settled[key];
}

export type AdvisorySubjectRef = { kind: 'post_id' | 'blob_cid'; id: string };

export function advisorySubjectKey(kind: AdvisorySubjectRef['kind'], id: string): string {
  const normalized = kind === 'blob_cid' ? id.trim().toLowerCase() : id.trim();
  return `${kind}:${normalized}`;
}

export type AdvisoryPostSource = Pick<PostView, 'object_id' | 'attachments' | 'repost_of' | 'reply_preview'> & Partial<Pick<PostView, 'content_labels'>>;

/// 投稿カードに現れる subject(本文・添付・引用元・返信プレビュー)。
export function postAdvisorySubjects(post: AdvisoryPostSource): AdvisorySubjectRef[] {
  const subjects: AdvisorySubjectRef[] = [];
  const addPost = (id: string | null | undefined) => {
    if (id?.trim()) subjects.push({ kind: 'post_id', id: id.trim() });
  };
  const addBlobs = (attachments: { hash: string }[] | null | undefined) => {
    for (const attachment of attachments ?? []) {
      if (attachment.hash.trim()) subjects.push({ kind: 'blob_cid', id: attachment.hash.trim() });
    }
  };
  addPost(post.object_id);
  addBlobs(post.attachments);
  addPost(post.repost_of?.source_object_id);
  addBlobs(post.repost_of?.attachments);
  addPost(post.reply_preview?.object_id);
  addBlobs(post.reply_preview?.attachments);
  return subjects;
}

/// 投稿カードでゲート対象になりうる添付 hash(本文・引用元)。
export function postGateableMediaHashes(post: AdvisoryPostSource): string[] {
  return [...post.attachments, ...(post.repost_of?.attachments ?? [])].map(
    (attachment) => attachment.hash
  );
}

export type PostAdvisoryState = {
  /// 代替表示の説明に使う advisory。添付への判定(`blob_cid`)を優先する。
  advisory: TimelineContentAdvisory | null;
  /// 照会が未決の subject を含む(確定前。メディアはスケルトンにして取得しない)。
  pending: boolean;
};

export function resolvePostAdvisory(
  post: AdvisoryPostSource,
  advisories: TimelineContentAdvisoryIndex,
  lookup: TimelineAdvisoryLookupState
): PostAdvisoryState {
  let blobAdvisory: TimelineContentAdvisory | null = null;
  let postAdvisory: TimelineContentAdvisory | null = null;
  let isPending = false;
  for (const subject of postAdvisorySubjects(post)) {
    const key = advisorySubjectKey(subject.kind, subject.id);
    if (isAdvisorySubjectPending(lookup, key)) isPending = true;
    const gating = (advisories[key] ?? []).find((entry) => isGatingContentAdvisory(entry.advisory));
    if (!gating) continue;
    if (subject.kind === 'blob_cid') {
      blobAdvisory ??= gating;
    } else {
      postAdvisory ??= gating;
    }
  }
  const advisory = blobAdvisory ?? postAdvisory;
  // advisory が確定していれば未決の subject が残っていても代替表示にする(確定後の表示を優先)。
  return { advisory, pending: advisory === null && isPending };
}

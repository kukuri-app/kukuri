# ADR 0021: Local Post Bookmark Library

## Status
Proposed

## Date
2026-03-30

## Base Branch
`main`

## Related
- `docs/adr/0002-feature-data-classification-template.md`
- `docs/adr/0016-repost-data-classification.md`
- `docs/adr/0017-reaction-data-classification.md`
- `docs/adr/0018-channel-first-sidebar-and-unified-epoch-lifecycle.md`
- `docs/adr/0020-pairwise-dm-v1.md`

## Feature Data Classification
- Feature 名: local post bookmark library
- Durable / Transient: Durable local-only bookmark library + local UI state
- Canonical Source: local SQLite の bookmarked post snapshot record
- Replicated?: No
- Rebuildable From: local bookmark snapshot record only
- Public Replica / Private Replica / Local Only:
  - bookmark collection: local only
  - saved post snapshot: local only
  - source post canonical source: 既存 public/private replica
- Gossip Hint 必要有無: No
- Blob 必要有無: new blob ownership は持たず、既存 attachment/blob cache を再利用する
- SQLite projection 必要有無: Yes
- 必須 contract:
  - `local_bookmarked_posts_restore_after_restart`
  - `bookmark_private_post_remains_local_only_and_readable_after_access_loss`
  - `unbookmark_removes_only_local_bookmark_record`
  - `bookmarked_posts_are_sorted_by_bookmarked_at_desc`
  - `bookmarked_repost_renders_from_saved_snapshot_without_source_timeline_hydration`
- 必須 scenario:
  - `public bookmark -> private bookmark -> bookmark page -> unbookmark from list -> unbookmark from timeline`
  - cross-client で bookmark state が共有されないこと

## Decision
- post bookmark v1 は、timeline 上の投稿を current device にだけ保存する local archive とする。
- bookmark 保存時は source object への参照だけでなく、bookmark page が source hydration なしで card を描画できる snapshot を local SQLite に保存する。
- bookmark 対象は topic timeline 上の `post` / `comment` / `repost` とし、public/private の両方を含む。
- pairwise DM、live session、game room、settings 内 asset library は bookmark 対象外とする。
- bookmark 一覧は `#/timeline?topic=<active>&timelineView=bookmarks` に固定し、`ADR 0018` の primary section は増やさない。
- bookmark 一覧は全 topic 横断の 1 一覧とし、`bookmarked_at DESC` で並べる。
- `timelineView=bookmarks` に入ると compose dialog と detail context は閉じる。
- bookmark は local-only であり、shared replica、community-node、他 client へは複製しない。

## Snapshot Contract
- local snapshot には少なくとも次を保持する。
  - `source_object_id`
  - `source_envelope_id`
  - `source_replica_id`
  - `bookmarked_at`
  - `topic_id`
  - `channel_id`
  - `object_kind`
  - `created_at`
  - `author_pubkey`
  - text/content snapshot
  - payload ref
  - attachment snapshot
  - reply/root refs
  - repost snapshot
- reaction 集計、relationship 状態、author profile 名称などの動的情報は canonical bookmark record には含めない。手元 cache があれば read model で overlay してよい。
- private channel 投稿を bookmark した場合も、current access を失った後は local bookmark snapshot をそのまま読む。
- unbookmark は bookmark-owned local record だけを削除し、source post、shared replica、blob cache、projection cache は削除しない。

## Public Interfaces
- `kukuri-store` に `BookmarkedPostRow` と `put_bookmarked_post`, `list_bookmarked_posts`, `remove_bookmarked_post` を追加する。
- `kukuri-app-api` に `BookmarkedPostView` と `bookmark_post(topic_id, object_id)`, `list_bookmarked_posts()`, `remove_bookmarked_post(object_id)` を追加する。
- `desktop-runtime` / Tauri / `apps/desktop/src/lib/api.ts` は上記 API をそのまま公開する。
- 現行（#1221 R1-A・R5-G）: 一覧は `list_bookmarked_posts_page`（固定件数の cursor ページ）だけで読む。全件の `list_bookmarked_posts()` は撤去し、CLI の `list_bookmarked_posts` も cursor つきのページを返す。
- desktop shell は `timelineView` route state を追加し、timeline workspace 内に `feed / bookmarks` subpage を持つ。

## Consequences
- bookmark page は source topic/channel の current selection に依存せず、local snapshot だけで一覧できる。
- bookmark は local preference/archival state として扱われるため、reaction bookmark library と同じく cross-device sync は行わない。
- private bookmark は source access を失っても local archive として残るため、bookmark 自体は privacy boundary の外に再配布されない一方、端末ローカルには残り続ける。

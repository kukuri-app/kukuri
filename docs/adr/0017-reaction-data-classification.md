# ADR 0017: Reaction / Custom Reaction Data Classification

## Status
Proposed

## Date
2026-03-28

## Base Branch
`main`

## Related
- `docs/adr/0002-feature-data-classification-template.md`
- `docs/adr/0003-image-post-data-classification.md`
- `docs/adr/0010-kukuri-protocol-v1-boundary-definition.md`
- `docs/adr/0011-kukuri-protocol-v1-draft.md`
- `docs/adr/0012-topic-first_progressive_community_filtering_draft.md`
- `docs/adr/0014-uiux-dev-flow.md`
- `docs/adr/0015-profile-topic-author-index.md`
- `docs/adr/0016-repost-data-classification.md`

## Feature Data Classification

### Feature Data Classification: 投稿 Reaction
- Feature 名: post reaction
- Durable / Transient: Durable public/private topic object + local aggregate/query state
- Canonical Source:
  - public target: `topic::<topic_id>` replica の `reaction`
  - private target: current private-channel epoch replica の `reaction`
- Replicated?: Yes, `reaction` object は target と同じ replica に複製される
- Rebuildable From: target replica 上の signed `reaction` object / deterministic reaction identity / target object metadata
- Public Replica / Private Replica / Local Only:
  - public post/comment reaction: public topic replica
  - private post/comment reaction: private-channel epoch replica
  - picker open state / hover state: local only
- Gossip Hint 必要有無: No new hint type; target replica の既存 object update path を使う
- Blob 必要有無: No
- SQLite projection 必要有無: Yes, target object ごとの aggregate と `my_reactions` query を local projection に持つ
- 必須 contract:
  - `public_post_reaction_persists_and_aggregates_emoji_and_custom_keys`
  - `same_author_same_reaction_key_toggles_off`
  - `different_reaction_keys_can_coexist_on_same_target`
  - `private_channel_reaction_stays_epoch_scoped_after_rotate`
  - `profile_feed_does_not_offer_direct_reaction_affordance`
- 必須 scenario:
  - public post に emoji/custom reaction を付け、別 client に aggregate が反映されること
  - private channel で emoji/custom reaction を付け、rotate 後に archived/current epoch の可視性が期待どおりに分かれること

### Feature Data Classification: カスタムリアクション asset
- Feature 名: custom reaction asset
- Durable / Transient: Durable public author-owned asset object + blob payload
- Canonical Source: `author::<author_pubkey>` replica の `custom-reaction-asset` object と `iroh-blobs` の image payload
- Replicated?: Yes, asset metadata は public author replica に複製され、blob は既存 public blob fetch path で共有される
- Rebuildable From: author replica 上の signed asset object / normalized blob payload
- Public Replica / Private Replica / Local Only:
  - asset metadata: public author replica
  - asset image bytes: public blob
  - crop editor transient state: local only
- Gossip Hint 必要有無: No new hint type; author replica hydration を使う
- Blob 必要有無: Yes
- SQLite projection 必要有無: No, v1 では author replica query を canonical read path とする
- 必須 contract:
  - `custom_reaction_asset_is_author_owned_public_blob_backed_object`
  - `animated_gif_custom_reaction_preserves_gif_mime_after_normalization`
  - `custom_reaction_snapshot_renders_without_author_asset_hydration`
- 必須 scenario:
  - A が custom reaction asset を作成し、public post に使えること
  - B が A の custom reaction asset を bookmark 後に別 post で再利用できること

### Feature Data Classification: ブックマーク済みカスタムリアクション library
- Feature 名: bookmarked custom reaction library
- Durable / Transient: Durable local-only library + local picker/query state
- Canonical Source: local SQLite の bookmark record
- Replicated?: No
- Rebuildable From: local bookmark record。source asset metadata は再取得できても bookmark collection 自体は local state を正とする
- Public Replica / Private Replica / Local Only:
  - bookmark collection: local only
  - settings UI state: local only
- Gossip Hint 必要有無: No
- Blob 必要有無: No new blob ownership は持たない。source asset blob を参照するだけとする
- SQLite projection 必要有無: Yes
- 必須 contract:
  - `local_bookmarks_restore_saved_custom_reactions_after_restart`
- 必須 scenario:
  - `Settings > Reactions` で bookmarked custom reactions の一覧、削除、picker 反映ができること

## 1. Current Main Snapshot

current `main` には `reaction` object kind、custom reaction asset、bookmark 済み reaction library のいずれもない。

この状態では次が満たせない。

- 投稿に対して lightweight な emoji reaction を durable に集計すること
- custom image を author-owned reusable asset として保存し、他人が bookmark して使い回すこと
- private channel epoch rotate 後も Reaction の可視性を epoch 境界に揃えること
- timeline/thread と settings の責務を分けた専用 reaction management UI を持つこと

一方で current architecture には次の基盤がある。

- public/private post object を target replica に正本化する path
- private channel current/archived epoch replica と epoch-aware read path
- `author::<author_pubkey>` replica に author-owned public index を置く pattern
- `docs + blobs + local SQLite projection` の責務分離

したがって reaction v1 は、post header への埋め込みや `create_post` overload ではなく、独立 object kind と独立 asset/library contract として導入する。

## 2. Decision

reaction v1 は `post` / `comment` に対する durable reaction に固定する。

- public/private の `post` / `comment` は対象に含む
- `repost`、`live-session`、`game-session`、profile feed 上の virtual item は対象外
- profile feed は read-only を維持し、Reaction は `Open original topic` 後の通常文脈でのみ行う

### 2.1 Reaction key

v1 の reaction key は次の 2 種類に固定する。

- Unicode emoji
- custom asset reference

`like` 専用の object、key、API は作らない。`👍` を含む quick reaction は UX preset であり、保存上は通常の emoji reaction と同一である。

### 2.2 Toggle semantics

同一 author は同一 target に対して、同一 reaction key を `0/1` 件だけ持てる。

- 同じ key を再押下した場合は toggle off として扱う
- 異なる key は同じ target 上で併用できる
- duplicate stack は許可しない

この一意性は `(target_object_id, author_pubkey, normalized_reaction_key)` で決める。

toggle の durable identity は deterministic `reaction_id` とし、同じ組み合わせに対する最新 state が常に 1 件へ収束するようにする。off は別 object を増やさず、同じ `reaction_id` に対する `deleted` state で表現する。

### 2.3 Canonical source

reaction record の canonical source は target object と同じ replica に置く。

- public target: `topic::<topic_id>`
- private target: current private-channel epoch replica

これにより public/private の visibility 境界、late join/backfill、epoch rotate 後の archive/current split を target object と同じ read path で扱う。

### 2.4 Custom reaction asset

custom reaction asset は public author-owned object とする。

- metadata は `author::<author_pubkey>` replica に保存する
- image payload は `iroh-blobs` に保存する
- asset 自体は public reusable とし、public/private どちらの Reaction からも参照できる
- v1 では immutable asset とし、edit/delete lifecycle は later scope に送る

### 2.5 Bookmark library

bookmark 済み custom reaction library は local-only durable state とする。

- shared replica には保存しない
- 端末間同期はしない
- bookmark は explicit action でのみ追加する
- 閲覧だけでは自動保存しない

理由:

- bookmark collection は user preference であり、public に出したくない
- current scope で new private author-sync primitive を増やさない
- `Settings > Reactions` を local preference manager として扱える

## 3. Data Model

### 3.1 Reaction object

signed envelope kind は `reaction` とする。

`reaction` object は少なくとも次を持つ。

- `reaction_id`
- `target_topic_id`
- `target_object_id`
- `target_object_kind`
- `author_pubkey`
- `reaction_key_kind` (`emoji` or `custom_asset`)
- `emoji?`
- `custom_asset_id?`
- `custom_asset_snapshot?`
- `created_at`
- `status`

validation:

- target object kind は `post` または `comment` のみ
- `emoji` と `custom_asset_id` は排他的
- `reaction_key_kind == emoji` のとき `emoji` は必須
- `reaction_key_kind == custom_asset` のとき `custom_asset_id` と `custom_asset_snapshot` は必須

doc key family は target replica 配下の `reactions/<target_object_id>/<reaction_id>/...` に固定し、timeline/thread aggregate は active reaction だけを数える。

projection の行は `.../envelope` の signed `reaction` object だけから作る。`.../state` は署名を持たないので、反映には使わない
（Issue #1252。検証の規則の正本は ADR 0052 §2）。

### 3.2 Custom reaction asset object

signed envelope kind は `custom-reaction-asset` とする。

`custom-reaction-asset` は少なくとも次を持つ。

- `asset_id`
- `owner_pubkey`
- `created_at`
- `mime`
- `blob_hash`
- `bytes`
- `width`
- `height`

保存ルール:

- persisted asset は必ず square crop 済み `128x128`
- static image input は PNG で保存する
- animated GIF input は GIF のまま保存する
- asset creation path は persisted blob がすでに normalized であることを保証する

author replica の key family は `reactions/assets/<asset_id>/...` に固定する。

### 3.3 Custom asset snapshot

custom asset を参照する reaction record と bookmark record には、次の immutable snapshot を埋め込む。

- `asset_id`
- `owner_pubkey`
- `blob_hash`
- `mime`
- `bytes`
- `width`
- `height`

viewer はこの snapshot だけで picker/timeline/thread の描画を開始できなければならない。author asset replica の hydration は追加情報取得 path であり、初回描画の前提にしない。

### 3.4 Local bookmark record

local SQLite の bookmark record は少なくとも次を持つ。

- `asset_id`
- `owner_pubkey`
- `blob_hash`
- `mime`
- `bytes`
- `width`
- `height`
- `bookmarked_at`

bookmark は source asset への pointer と display snapshot を持つが、新しい blob ownership は作らない。

## 4. Publish And Read Contract

### 4.1 Toggle reaction API

publish API は `toggle_reaction(target_topic_id, target_object_id, reaction_key, channel_ref?)` を追加する。

この API は次を行う。

- target object が `post` / `comment` であることを確認する
- `channel_ref` から public/private の replica を決定する
- `(target_object_id, author_pubkey, normalized_reaction_key)` から deterministic `reaction_id` を導出する
- active なら same `reaction_id` を `deleted` へ更新し、inactive なら `active` へ更新する
- local projection を再集計して `reaction_summary[]` と `my_reactions[]` を返せるようにする

### 4.2 Custom asset creation API

asset 作成 API は `create_custom_reaction_asset(upload, crop_rect)` を追加する。

この API は次を行う。

- upload を square crop する
- `128x128` に resize する
- animated GIF は animation を維持したまま GIF として保存する
- non-animated raster image は PNG として保存する
- normalized blob を `iroh-blobs` へ put する
- `author::<author_pubkey>` replica に `custom-reaction-asset` object を保存する

crop UI は user-adjustable square crop とし、初期位置は centered default に固定する。

### 4.3 Bookmark APIs

bookmark library の API は次を追加する。

- `list_bookmarked_custom_reactions()`
- `bookmark_custom_reaction(asset_snapshot)`
- `remove_bookmarked_custom_reaction(asset_id)`

bookmark は local SQLite への insert/delete のみを行い、shared replica には副作用を持たない。

### 4.4 Read model additions

`PostView` には少なくとも次を追加する。

- `reaction_summary[]`
- `my_reactions[]`

`reaction_summary[]` の item は少なくとも次を持つ。

- `reaction_key_kind`
- `emoji?`
- `custom_asset_snapshot?`
- `count`

`my_reactions[]` は viewer 自身が active にしている normalized reaction key の一覧とする。

custom asset / bookmark view は少なくとも次を持つ。

- `asset_id`
- `owner_pubkey`
- `blob_hash`
- `mime`
- `bytes`
- `width`
- `height`

## 5. UX Boundary

reaction affordance は次の surface に出す。

- timeline card
- thread pane

reaction picker は少なくとも次の source を束ねる。

- quick emoji preset
- 任意 emoji 入力
- 自分の custom reaction assets
- bookmark 済み custom reaction assets

専用管理 UI は `Settings > Reactions` に置く。

ここで v1 は次を扱う。

- bookmarked custom reactions の一覧
- bookmark 削除
- picker に出る saved assets の確認

profile feed は read-only を維持する。

- profile feed 上では direct reaction affordance を出さない
- `Open original topic` 後の通常 topic/thread 文脈でのみ Reaction できる

## 6. Privacy And Scope Boundary

private channel reaction は target と同じ epoch boundary に従う。

- current epoch participant は current epoch reaction を見られる
- archived epoch を読める participant は旧 reaction も見られる
- rotate 後 newcomer は旧 epoch reaction を見られない

custom asset 自体は public object だが、private channel 内でその asset を使った事実は private reaction record に閉じる。public に出るのは asset metadata/blob だけであり、どの private target に使われたかは public replica へ書かない。

v1 では次を扱わない。

- reactor list
- reaction notification
- bookmark の端末間同期
- custom asset の edit/delete lifecycle
- pack/search/discovery
- moderation/reporting

## 7. Validation

v1 の required contract は上記各「Feature Data Classification」block（投稿 Reaction / カスタムリアクション asset / ブックマーク済みカスタムリアクション library）の 必須 contract を単一の正とし、ここでは再掲しない。

required scenario は少なくとも次である。

- A が custom reaction asset を作成して public post に付け、B がそれを bookmark して別 public post に再利用できること
- private channel で emoji/custom reaction を付け、rotate 後に archived/current epoch の可視性が期待どおりに分かれること
- `Settings > Reactions` で bookmarked custom reactions の一覧、削除、picker 反映ができること

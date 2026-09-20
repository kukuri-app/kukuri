# ADR 0016: Public Repost / Quote Repost

## Status
Proposed

## Date
2026-03-28

## Base Branch
`main`

## Related
- `docs/adr/0011-kukuri-protocol-v1-draft.md`
- `docs/adr/0012-topic-first_progressive_community_filtering_draft.md`
- `docs/adr/0013-social-graph-foundation-draft.md`
- `docs/adr/0015-profile-topic-author-index.md`

## Feature Data Classification
- Feature 名: public repost / quote repost
- Durable / Transient: Durable public topic object + durable public author-owned index + local read/query state
- Canonical Source:
  - target topic 側: `topic::<target_topic_id>` replica の `repost`
  - profile topic 側: `author::<author_pubkey>` replica の `profile-repost`
- Replicated?: Yes, `repost` object と `profile-repost` doc は public replica に複製される
- Rebuildable From: source public object / source snapshot / signed repost envelope / author replica index
- Public Replica / Private Replica / Local Only:
  - `repost`: public topic replica
  - `profile-repost`: public author replica
  - composer / thread open state: local only
- Gossip Hint 必要有無: No new hint type; topic 側は既存 `TopicObjectsChanged`、author 側は既存 `ProfileUpdated`
- Blob 必要有無: source attachment snapshot は envelope に保持し、blob fetch は既存 path を再利用
- SQLite projection 必要有無: Yes, repost read model を topic timeline / thread projection に含める
- 必須 contract:
  - `create_same_topic_repost_persists_repost_object_and_profile_repost_doc`
  - `create_cross_topic_repost_renders_from_target_topic_without_tracking_source_topic`
  - `simple_repost_is_unique_per_author_target_and_original`
  - `quote_repost_allows_multiple_distinct_quotes_for_same_original`
  - `quote_repost_opens_own_thread_and_simple_repost_cannot_be_reply_parent`
  - `profile_timeline_merges_profile_posts_and_profile_reposts`
  - `public_comment_can_be_reposted_with_source_context_snapshot`
  - `private_channel_post_cannot_be_reposted_publicly`
- 必須 scenario:
  - A が topic X の public post / comment を topic Y へ repost / quote repost し、B は Y だけ tracked していてもカードを読めること
  - repost / quote repost が profile feed にも出ること
  - quote repost だけが独立 thread を開けること

## 1. Current Main Snapshot

current `main` には repost object kind がなく、topic をまたぐ再配信は forward や manual copy と区別できない。

この状態では次が満たせない。

- same-topic repost と cross-topic repost を同じ contract で扱うこと
- repost を profile feed に自動集約すること
- quote repost を独立 thread として扱い、simple repost は original thread に戻すこと
- viewer が source topic を tracked していなくても repost card を target topic だけで描画すること

一方で current architecture には次の基盤がある。

- public topic object の signed envelope
- author replica 上の public profile index
- topic timeline / thread / profile timeline を返す read model

したがって repost v1 は、`create_post` の overload ではなく独立 object kind と独立 publish API として導入する。

## 2. Decision

v1 の repost は `public topics only` に固定する。

- repost target は public `post` / `comment` のみ
- private channel / private audience object は対象外
- live session / game room は対象外

`commentary` が空なら simple repost、空でなければ quote repost と定義する。

### 2.1 Canonical source

repost の canonical source は 2 本立てにする。

- target topic 上の正本: `topic::<target_topic_id>` replica の `repost` object
- profile topic 上の正本: `author::<author_pubkey>` replica の `profile-repost` doc

original object の canonical source は移動しない。original の正本は常に source topic replica に残る。

### 2.2 Publish boundary

publish API は `create_post` の overload ではなく `create_repost` 系 API を追加する。

publish 時は次を行う。

- `topic::<target_topic_id>` に `repost` object を保存する
- `author::<author_pubkey>` に `profile-repost` doc を保存する

manual に profile topic へ直接 repost する UI は作らない。profile feed への反映は public repost の副作用である。

### 2.3 Simple / quote normalization

- simple repost は `(author_pubkey, target_topic_id, source_object_id)` ごとに 1 件へ正規化する
  - 既存の simple repost の検索は projection の索引（著者 + repost 元）で行う。topic の replica は走査しない（ADR 0052 §4）
  - 取り下げ済みの simple repost は既存の repost として扱わない。取り下げた後は、同じ投稿をもう一度 simple repost できる
- quote repost は別投稿として複数許可する
- quote repost の commentary は v1 では text-only
- quote repost 自体への追加 attachment は持たない

## 3. Data Model

### 3.1 Topic repost object

`repost` object は少なくとも次を持つ。

- `target_topic_id`
- `source_topic_id`
- `source_object_id`
- `source_author_pubkey`
- `source_object_kind`
- `source_reply_to?`
- `source_root_id?`
- `source snapshot`
- `commentary payload?`
- `created_at`

cross-topic repost でも target topic だけでカード描画できるよう、source object の最小 snapshot を repost object 自体へ埋め込む。

### 3.2 Profile repost doc

`profile-repost` doc は少なくとも次を持つ。

- `author_pubkey`
- `profile_topic_id`
- `published_topic_id`
- `object_id`
- `created_at`
- `commentary payload?`
- `source snapshot`
- `envelope_id`

viewer が source topic を tracked していなくても profile feed を描画できるよう、`profile-repost` doc も source snapshot を保持する。

## 4. Read Model

`PostView` 相当の read model は少なくとも次を返す。

- `published_topic_id`
- `origin_topic_id` は「この item が publish された topic」を指す compatibility alias として残す
- `repost_of`
- `repost_commentary?`
- `is_threadable`

`repost_of` は少なくとも次を含む。

- `source_object_id`
- `source_topic_id`
- `source_author_pubkey`
- `source_object_kind`
- `source content snapshot`
- `source attachments snapshot`
- `source reply/thread refs`

`list_profile_timeline` は `profile-post` と `profile-repost` を merge して返す。

## 5. Thread And UX Boundary

- quote repost だけを threadable にする
- simple repost は reply parent にできない
- simple repost を開く UI は original thread / original topic へ遷移させる
- profile topic は引き続き virtual / read-only feed とする

これにより `0015` の「profile feed は `profile-post` だけを集約する」という制約は superseded される。一方で profile feed の read-only 方針自体は維持する。

## 6. Privacy Boundary

private channel repost は later scope に送る。

理由:

- current `0012` の privacy boundary と衝突させない
- private audience の capability / encryption / relay semantics を repost v1 に混ぜない
- v1 を public durable object に固定して canonical read path を先に固める

## 7. Validation

必須 contract は上記「Feature Data Classification」を単一の正とし、ここでは再掲しない。

required scenario は次である。

- A が topic X の public post / comment を topic Y に repost する
- A が同じ original を quote repost する
- B は topic Y だけ tracked していても repost / quote repost card を読める
- B は profile feed でも同じ repost / quote repost を見られる
- B は quote repost だけ独立 thread を開け、simple repost は original thread に戻る

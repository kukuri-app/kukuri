# ADR 0013: social-graph foundation

## Status
Accepted

## Date
2026-03-20

## Base Branch
`main`

## Related
- `docs/adr/0011-kukuri-protocol-v1-draft.md`
- `docs/adr/0012-topic-first_progressive_community_filtering_draft.md`

## Feature Data Classification
- Feature 名: social-graph foundation
- Durable / Transient: Durable public graph state + local durable projection
- Canonical Source: public author docs replica (`author::<pubkey>`) に保存された author-signed profile / outgoing follow edge
- Replicated?: Yes, public author graph is replicated; derived relationship cache is local rebuildable
- Rebuildable From: author replicas + signed envelopes + local projection rebuild
- Public Replica / Private Replica / Local Only: public author replica for profile / follow edge, local only projection for relationship query cache
- Gossip Hint 必要有無: Yes, best-effort only
- Blob 必要有無: No
- SQLite projection 必要有無: Yes
- 必須 contract:
  - `store_profile_upsert_latest_wins`
  - `author_relationship_projection_rebuild_roundtrip`
  - `social_graph_derives_friend_of_friend_and_clears_after_unfollow`
  - `friend_only_channel_restore_keeps_archived_epoch_history`
  - `post card shows friend of friend badge and author name fallback`
  - `author detail shows via authors and follow action updates relationship`
  - `local profile editor saves profile draft`
- 必須 scenario:
  - Linux 実機 2-3 台で `nickname/profile 表示 -> follow -> mutual -> unfollow -> friend of friend -> restart 後復元` を確認する

## 1. Current Main Snapshot

social-graph v1 は 2026-03-20 時点で current `main` に入っており、もはや future plan ではなく baseline である。

現行実装は次を持つ。

- public author replica (`author::<pubkey>`) を social graph の canonical source とする
- `identity-profile` と `follow-edge` の signed envelope を author-owned object として扱う
- profile publish / hydrate、follow / unfollow、relationship projection rebuild を `app-api` / `desktop-runtime` / desktop UI まで通す
- local projection から `following`, `followed_by`, `mutual`, `friend_of_friend`, `friend_of_friend_via_pubkeys` を導出する
- `friend_only` / `friend_plus` の policy 入力として `mutual` を供給する

したがって本 ADR は、social graph を「これから導入する案」ではなく、current `main` の境界を固定する accepted ADR として扱う。

## 2. Decision

social-graph v1 は、public author replica を canonical source にした author-owned graph として維持する。

### 2.1 v1 scope

v1 に含めるもの:

- public profile
- directed follow edge
- follow revoke
- local projection による `following` / `followed_by` / `mutual` 判定
- local projection による `friend_of_friend` 判定
- `friend_of_friend` の read-only 表示

v1 に含めないもの:

- `friend_of_friend` を policy truth に使う gating
- recommendation / ranking
- server-managed social graph
- private note / local alias / mute の同期（blockはADR-0043でauthor replicaへ同期する）
- community-node 上の follow registry

### 2.2 author replica を social graph の正本にする

各 author は public な author replica を持つ。

```text
author::<author_pubkey>
```

この replica には、その author が所有する public social objects を置く。

最低限:

- `profile/latest`
- `graph/follows/<target_pubkey>`
- `envelopes/<envelope_id>`

incoming edge は正本として持たない。`followed_by`, `mutual`, `friend_of_friend` は local projection で導出する。

### 2.3 authority は docs write 権ではなく署名で決める

current `main` の docs replica は deterministic secret で開けるため、replica への write capability 自体は social graph の権限境界ではない。

authority は次で決める。

- profile は `envelope.pubkey == author_pubkey`
- follow edge は `subject_pubkey == envelope.pubkey`
- revoke も同一 author の署名でなければ無効

docs replica は配布・同期の媒体であり、trust anchor ではない。

## 3. Data Model

### 3.1 Envelope kinds

少なくとも次の signed envelope を使う。

- `identity-profile`
- `follow-edge`

revoke は `follow-edge` の `status = revoked` で表現する。

### 3.2 Minimal document shapes

profile doc は少なくとも次を持つ。

- `author_pubkey`
- `name`
- `display_name`
- `about`
- `picture`
- `updated_at`
- `envelope_id`

follow edge doc は少なくとも次を持つ。

- `subject_pubkey`
- `target_pubkey`
- `status`
- `updated_at`
- `envelope_id`

### 3.3 Local projection

SQLite projection は少なくとも次を持つ。

- profile cache
- `follow_edges`
- `author_relationship_cache`

`author_relationship_cache` は canonical source ではなく local query のための materialized cache であり、author replica から rebuild 可能でなければならない。

2026-09-26 追補（#1221 R4-D、[ADR 0055](0055-demand-owned-network-work.md) §4）: `author_relationship_cache` と全件 rebuild は撤去した。関係は読むときに対象 author の follow edge（`follow_edges`）を主キーで点読して導出する。friend_of_friend の意味は変えない。

導出 query は次を引ければよい。

- `following`
- `followed_by`
- `mutual`
- `friend_of_friend`
- `friend_of_friend_via_pubkeys`

`friend_of_friend` は v1 から projection と UI に入れるが、join 可否や access control の正本にはしない。

## 4. Sync Boundary

### 4.1 topic sync とは別系統で扱う

social graph は topic timeline と同じ subscription map に押し込まない。

理由:

- topic は user-facing browsing unit
- social graph は author-centric state
- lifecycle と query pattern が異なる

したがって author replica hydration / rebuild は topic subscription とは別責務として扱う。

### 4.2 Sync 対象は bounded に保つ

v1 の sync 対象は次に制限する。

- local author replica
- 明示的に follow した author の replica
- active topic で観測した author の replica
- community / invite 導線で明示的に必要になった author

既知の全 pubkey を自動 crawl しない。`friend_of_friend` のために無制限に graph crawl すると、帯域と local cache が膨らみすぎる。

### 4.3 hint は加速用であり truth ではない

profile / social graph 更新の通知に hint を使ってよいが、truth source にはしない。

current `main` では social graph 専用の新 hint type は増やさず、author replica hydration と projection rebuild の責務を優先する。

## 5. Community-Node Boundary

community-node は social graph の canonical store にならない。

維持する境界:

- community-node は auth / consent / bootstrap / relay assist
- social graph の canonical source は docs + signed envelope
- relationship 判定は local projection で計算する

community-node に follower list や friend graph を寄せると、`docs + blobs + hints` を正本とする current architecture と衝突する。

## 6. UX Boundary

social-graph v1 は current `main` に次の UX として入っている。

- profile editor
- author detail / author card
- follow / unfollow
- relationship badge (`following`, `follows you`, `mutual`)
- `friend of friend` 表示

一方で、まだこの ADR に入れないものは次である。

- `friend_of_friend` ベースの gating
- social graph ベースの recommendation / ranking
- synced mute / alias / private note（signed directional blockはADR-0043で追加済み）

`friend_only` と `friend_plus` の product semantics は `0012` に従う。特に `friend_plus` は owner の `friend_of_friend` snapshot ではなく participant-scoped `mutual` chain を正本とする。

## 7. Validation

social graph v1 の current baseline を固定する 必須 contract / manual scenario は上記「Feature Data Classification」を単一の正とし、ここでは再掲しない。current baseline ではこれらを満たしており、Linux 実機 2-3 台での manual verification も確認済みである。

## 8. Follow-up Boundary

social graph v1 自体の実装残はない。残っているのは social graph の上に載る次段の product / moderation 課題である。

この ADR の外側に残すもの:

- `friend_plus` candidate expansion に対する abuse / spam guardrail
- explicit member list
- approval workflow
- moderator role
- relationship explainability の追加磨き込み

これらは `0012` 側の audience / moderation follow-up として扱う。

## 9. Consequences

- social graph は current `main` の baseline であり、friend 系 audience の前提条件ではなく現行入力である
- canonical source は引き続き public author replica + signed envelope に固定する
- relationship cache は local materialized projection として rebuild 可能でなければならない
- future feature は community-node を social graph の server truth にしてはならない

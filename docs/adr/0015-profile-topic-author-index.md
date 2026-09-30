# ADR 0015: Author Profile Topic

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
- `docs/adr/0014-uiux-dev-flow.md`
- `docs/adr/0016-repost-data-classification.md`

## Feature Data Classification
- Feature 名: author profile topic
- Durable / Transient: Durable public author-owned index + local read-only query state
- Canonical Source: public author docs replica (`author::<author_pubkey>`) に保存された author-signed `profile-post`
- Replicated?: Yes, `profile-post` doc は public author replica に複製される
- Rebuildable From: author replica 上の `profile-post` doc / signed envelope / attachment ref
- Public Replica / Private Replica / Local Only: public author replica for `profile-post`, local only UI route / query state
- Gossip Hint 必要有無: No new hint type; author replica subscription / hydration を使う
- Blob 必要有無: 既存 blob ref を再利用する
- SQLite projection 必要有無: v1 では dedicated profile-topic projection を増やさず、author replica query で読む
- 必須 contract:
  - `create_public_post_persists_profile_post_doc_and_lists_profile_timeline`
  - `public_reply_is_indexed_in_profile_timeline`
  - `private_channel_post_is_not_indexed_in_profile_timeline`
  - `list_profile_timeline_ignores_profile_post_with_signer_mismatch`
  - `profile overview aggregates public posts across topics and excludes private channel posts`
  - `author detail shows profile topic posts and can open an untracked origin topic`
- 必須 scenario:
  - `profile_timeline_reads_author_public_posts_across_untracked_topics`

## 1. Current Main Snapshot

current `main` の profile overview は、active topic の public timeline を local author で filter したものとして振る舞っている。

この形では次が満たせない。

- author の public 投稿を topic をまたいで一箇所に集約すること
- viewer が origin topic を tracked していなくても author の public 投稿を読むこと
- `#/profile` と author detail を同じ author-centric feed として扱うこと

一方で current architecture は、topic timeline とは別に `author::<author_pubkey>` replica を持ち、署名付き author-owned object をここへ載せる boundary をすでに持っている。

したがって profile topic v1 は、新しい public topic replica を作るのではなく、author replica 上の public index として導入する。

## 2. Decision

profile topic v1 は、author ごとの public 投稿を束ねる virtual / system topic として扱う。

Note: `profile feed は profile-post だけを集約する` という v1 初期制約は `docs/adr/0016-repost-data-classification.md` で superseded される。read-only feed である点は維持する。

### 2.1 Logical topic id

UI / query 上の stable logical ID を次で固定する。

```text
kukuri:topic:profile:<author_pubkey>
```

この ID は virtual topic を識別するための logical handle であり、v1 では tracked topics nav の追加・削除対象にしない。

### 2.2 Canonical source

origin public post の正本は従来どおり topic replica に残す。

```text
topic::<topic_id>
```

profile topic の正本は topic replica ではなく、次の author replica に置く。

```text
author::<author_pubkey>
```

ここに author-signed `profile-post` doc を保存し、client はこれを profile topic membership と解釈する。

### 2.3 Authority boundary

P2P 上では author 以外が author replica に doc を書こうとすること自体は完全には防げない。v1 の権限境界は docs write capability ではなく署名で決める。

有効な `profile-post` は次を満たすものだけである。

- `content.author_pubkey == envelope.pubkey`
- `content.profile_topic_id == kukuri:topic:profile:<author_pubkey>`
- envelope signer と doc owner 解釈が一致する
- `object_kind` は `post` または `comment`

non-author doc、signer 不一致 doc、owner 不一致 doc、channel/private object を指す doc は無視する。

## 3. Data Model

### 3.1 Envelope kind

v1 は新しい signed envelope kind として `profile-post` を使う。

これは normal topic timeline projection 用の object ではなく、author-owned public index object である。

### 3.2 Minimal document shape

`profile-post` doc は少なくとも次を持つ。

- `author_pubkey`
- `profile_topic_id`
- `origin_topic_id`
- `object_id`
- `created_at`
- `object_kind`
- `reply_to_object_id?`
- `root_id?`
- public post card を描画できる display snapshot としての `content`
- attachment / media ref の最小情報
- `envelope_id`

v1 の attachment 表示は既存 blob fetch を再利用する。ただし profile feed の基本描画は author replica 内の snapshot だけで成立しなければならない。

## 4. Publish And Read Contract

### 4.1 Publish

public post publish 時は次を行う。

- origin topic replica へ通常の `post` / `comment` object を publish する
- 同じ author の `author::<author_pubkey>` replica へ `profile-post` doc と signed envelope を追加する

public reply も同じく profile topic に集約する。

次は `profile-post` を作らない。

- private channel post
- private reply
- live session
- game room

### 4.2 Read surface

v1 の read surface は `list_profile_timeline(author_pubkey, cursor, limit)` 相当とする。

この API は author replica を hydrate し、`profile-post` doc を created-at 降順で読む。返却 item には少なくとも次を含める。

- `origin_topic_id`
- `object_id`
- `author_pubkey`
- `content`
- `attachments`
- `reply_to`
- `root_id`

v1 では new dedicated SQLite projection は入れず、author replica query を canonical read path とする。

#### 手元に持つ投稿の表示（#1442）

相手の author replica は手元に無いことが多く、作者がオフラインなら読めない。そのときも、閲覧者が topic で受け取り署名を検証した
その作者の公開の投稿・返信・repost（既存の object projection の公開の行）をプロフィールに出す。プロフィール専用の projection は
増やさず、object projection を (著者, channel, 時刻, id) の索引の範囲で 1 ページだけ読み、author replica の行と合わせる。同じ投稿は
author replica の行を使う。private channel の行は出さない。手元の行だけで 1 ページが埋まらないときの remote の読み出しは変えない。

#### ローカル投稿後の表示（#913）

自分の端末で正常に保存された公開投稿・公開返信は、ピアが0台でもローカルのauthor replicaからプロフィールに表示する。ピア接続や遠隔への伝播を自己投稿の表示条件にしない。

Column Canvasでは、自己Profileが非アクティブでも、公開投稿の保存成功後にその一覧を再取得する。件数は取得済みの公開feedに合わせ、全期間の投稿総数や伝播完了数として扱わない。送信中・失敗したdraftは投稿元で状態を示し、確定済みのプロフィール件数には加えない。

初回取得中・取得失敗を空のfeedと同一視せず、更新中・失敗時は直前の有効な一覧を保持する。失敗時は理由と再試行手段を示す。更新によってアクティブColumn、編集draft、focusやscrollを移動しない。

## 5. UX Boundary

初回の UX 境界は次の二箇所に固定する。

- `#/profile`
- author detail

この feed は read-only とする。

- profile feed 上では reply / thread affordance を出さない
- 各カードに `origin topic` を表示する
- `Open original topic` で通常の topic 文脈へ移る

これにより、network 的には誰でも doc を書ける可能性があっても、UI と client-side interpretation によって「実質的に author だけが書ける topic」として扱う。

current の `profile overview = active topic public self timeline` は superseded target とする。

一方で次は変えない。

- topic-first browsing
- main timeline
- thread semantics
- tracked topic navigation model

## 6. Sync Boundary

profile topic は normal topic subscription map に混ぜない。

理由:

- browsing unit は引き続き topic
- profile topic は author-centric な virtual feed
- sync の truth source が topic replica ではなく author replica

したがって v1 では new gossip hint type を増やさず、author replica subscription / hydration を使う。

viewer が origin topic を購読していなくても、author replica が取れれば profile feed は読めるべきである。origin topic そのものを開くのは `Open original topic` を起点にした通常の topic subscription とする。

## 7. Validation

必須 contract / 必須 scenario は上記「Feature Data Classification」を単一の正とし、ここでは再掲しない。

required scenario は次である。

- A が複数 public topic に投稿し、B はその一部しか tracked していない状態でも、B は A の profile feed で全 public post / reply を見られる
- B は profile feed 上では返信できず、origin topic を開いた後だけ通常の thread / reply に入れる

# ADR 0032: 著者撤回と Community Node 内の送信防止

## Status

Accepted

**2026-10-09 改訂**（#1698）: §3 の判断記録の項目と、外部へ返す状態の範囲を実装（#761）に合わせた。当初の §3 は、判断記録に対象著者を持ち、状態を認証済み公開鍵と対象著者が一致する本人だけに返すとしていたが、実装は著者を持たず、状態を認証なしで返していた。本人だけに返す形と対象著者の保持は失効し、認証なしの状態取得を仕様とする。著者は自分の投稿への措置を認証なしで確かめられ、対象 ID を知る第三者にも措置の有無と根拠カテゴリが分かる。保存データと API の形は変わらないので、移行は無い。

## Date

2026-08-25

## Context

kukuri の投稿正本は author-owned な `docs` replica にあり、Community Node は network 全体の投稿を削除する authority を持たない。一方で著者は自分の投稿について以後の表示停止を宣言できる必要があり、Community Node operator は自分が提供する索引、検索、発見、推薦、moderation、blob cache に法的な送信防止判断を一貫して反映できる必要がある。

safety 判定、法的判断、著者撤回、operator policy は根拠・authority・解除条件が異なるため、同じ理由カテゴリや同じ正本へ統合してはならない。

## Feature Data Classification

- Feature 名: 著者署名付き投稿撤回
- Durable / Transient: Durable
- Canonical Source: 対象投稿と同じ author-owned `docs` replica の署名済み `post_withdrawal` envelope
- Replicated?: Yes。対象 replica の参加 peer 間だけで同期する
- Rebuildable From: 対象投稿 envelope と有効な撤回事象
- Public Replica / Private Replica / Local Only: 対象投稿と同じ public topic または private channel replica
- Gossip Hint 必要有無: 有。通知だけに使い、状態の正にはしない
- Blob 必要有無: 無。理由本文や対象本文を撤回事象へ複製しない
- SQLite projection 必要有無: 有。表示抑止の最小 ledger として再構築可能に保持する
- 必須 contract: 非著者拒否、署名改変拒否、対象 scope 一致、世代競合、hint 欠落後の収束、projection 再構築後の非表示
- 必須 scenario: 端末 A の撤回が `docs` 経由で端末 B と対応 Community Node へ届き、再起動後も本文・添付が再表示・再索引されない

- Feature 名: Community Node 内の送信防止
- Durable / Transient: Durable
- Canonical Source: 当該 node の Postgres にある node-local decision record
- Replicated?: No
- Rebuildable From: operator decision と append-only audit。対象本文からは再構築しない
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 無
- Blob 必要有無: cache が有効な場合だけ削除対象 hash と再取得拒否を保持する。侵害内容は保持しない
- SQLite projection 必要有無: 無
- 必須 contract: authority scope、適用・解除 transaction、全 index surface の fail-closed gate、cache evict／deny、認証なしの状態取得、解除後の fresh scan
- 必須 scenario: 適用後に search／discovery／recommendation／cache から消え、restart／backfill／再投影で復活せず、解除後も fresh `allow` までは復活しない

## Decision

### 1. 著者撤回は独立した署名済み事象とする

`post_withdrawal` envelope は次を署名対象に含める。

- 対象 object ID
- 対象著者公開鍵
- topic ID と任意の channel ID
- 1 以上の世代
- 任意の置換 object ID
- 理由公開範囲
- 公開する場合だけ列挙済み理由コード

撤回時刻は envelope の署名対象である `created_at` を正とする。検証時は撤回事象と元投稿 envelope の署名を検証し、撤回 signer、対象著者、元投稿著者が一致することを必須にする。非公開理由は理由コードも本文も replica へ載せない。

同じ対象の競合は世代、撤回時刻、withdrawal envelope ID の降順で決定する。撤回は取り消し不能で、置換先は元投稿を復活させない。

### 2. `docs` が正、gossip は通知

撤回事象は対象投稿と同じ replica の `withdrawals/<target_object_id>/state` に保存する。gossip hint は対象 ID を通知して再 hydration を促すだけであり、hint が欠落しても replica の再走査で同じ状態へ収束しなければならない。

対応 client は最小 ledger を SQLite projection に持ち、元投稿の本文・添付・引用 snapshot・返信先 preview・bookmark snapshot・通知 preview を既定で返さない。子返信は独立した著者所有 object なので削除せず、撤回済み親の placeholder へ接続する。

### 3. 法的な送信防止は node-local decision とする

Postgres の decision record（`cn_legal.transmission_preventions`）は対象（`post` / `blob` の種別と ID）、根拠カテゴリ、措置 capability、決定者、決定時刻、失効時刻、解除時刻、関連 report ID を保持し、対象著者は持たない。適用・解除と `cn_admin.operator_actions` への audit 追記は同じ transaction で行う。

外部へは `GET /v1/transmission-preventions/{subject_kind}/{subject_id}` で状態を返す。認証は求めず、対象を指定した誰にでも、有効かどうか、根拠カテゴリ、措置 capability、決定時刻、失効時刻、異議申立て先（当該 node の `POST /v1/report`）だけを返す。決定者、申出者、関連 report ID、非公開説明、内部 audit は返さない。

### 4. 最終判断は理由を分離したまま合成する

主表示理由の優先順位は次とする。

1. 著者撤回
2. 法的な送信防止
3. safety verdict
4. operator policy

ただし許可条件は全ゲートの論理積であり、どれか一つでも有効な抑止なら表示・索引しない。法的判断を解除しても著者撤回や safety verdict は変化しない。解除後の再取込は、有効な抑止が無いことと新しい scan の `allow` を必須にする。

indexer は本文・blob の取得前と Postgres upsert 直前に合成判定を行う。適用時は Postgres の索引真実源を先に非許可化し、query gate を即時に閉じてから ArcadeDB と cache を冪等削除する。これにより派生投影削除が一時的に失敗しても surfacing しない。ArcadeDB の写しは、適用の transaction で索引真実源から外した scope と対象を消し待ち（`cn_index.projection_evictions`）に入れ、indexer の全体の巡回が 1 回 128 件以内で消す。消せなかった行は次の巡回で消し直す。解除・失効した対象の行は、解除の後の新しい取込で戻った写しを消さないよう、投影に触れずに外す。

### 5. capability ごとの境界

| Capability | 可能な措置 | 不可能な措置 |
|---|---|---|
| community index／search／discovery／recommendation | Postgres gate、ArcadeDB 削除、再取込拒否 | 他 node の索引削除 |
| moderation／report | node-local 状態、audit、本人確認、異議申立て接続 | safety 記録との理由統合、network-wide command |
| blob cache | cache 削除、hash deny、再 cache 防止 | 第三者端末や source peer の blob 消去 |
| relay assist／bootstrap assist | manifest と文書で責任境界を説明 | 暗号化 packet の対象投稿判定、既取得データの回収 |
| Direct P2P | 対応 client が撤回事象を解釈して表示停止 | Community Node による直接経路の遮断 |

`blob_cache=true` の配備は、削除と再取得拒否を実行できる backend を readiness で必須にする。現行の scan 用一時取得経路は抑止対象を取得せず、local store を `Missing` のまま保つ。

## Consequences

- 投稿正本と user identity は author-owned のまま維持される。
- 対応 client／node では新規表示・索引・cache を止められるが、第三者端末へ既に複製された内容の完全回収は保証しない。
- 撤回済みであることを示す最小 metadata は残るが、本文、添付、非公開理由、申出内容を再配布しない。
- safety、法的判断、著者撤回、operator policy は別 record のまま監査・解除できる。

## Out of Scope

- 対応していない client／node からの強制削除
- 第三者端末の遠隔消去または暗号学的消去
- relay packet の内容検査と network-wide moderation authority

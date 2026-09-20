# ADR 0005: Live Session Data Classification

## Status
Accepted

## Feature Data Classification
- Feature 名: live session
- Durable / Transient: Durable
- Canonical Source: `iroh-docs` for current state pointer, `iroh-blobs` for manifest payload
- Replicated?: Yes
- Rebuildable From: `docs + blobs`
- Public Replica / Private Replica / Local Only: Public replica for session state pointer, local projection for viewer presence cache and joined-by-me state
- Gossip Hint 必要有無: Yes, `LiveSignal` for start/end and `LivePresence` for viewer heartbeat only
- Blob 必要有無: Yes
- SQLite projection 必要有無: Yes
- 必須 contract:
  - `late_joiner_backfills_live_session_manifest`
  - `restart_restores_live_session_manifest`
  - `live_presence_expires_without_heartbeat`
  - `ended_live_session_rejects_new_viewers`
- 必須 scenario:
  - live session panel 導入後に `create live -> join -> viewer count visible -> end -> restart -> restored ended state` を追加する

## Decision
- `docs` の topic replica に `live/<session_id>/state` を置き、current manifest blob ref と状態の index metadata だけを保存する。
- live manifest 本体は JSON blob として `iroh-blobs` に保存し、更新のたびに新しい blob hash を払い出す。
- state と manifest blob は署名を持たない。owner は manifest 全体を content にした `live-session` の envelope に署名して同じ replica の
  `envelopes/<envelope id>` へ置き、`state.last_envelope_id` がそれを指す。読む側は、署名者・id と owner の結び付け・読んだ replica との整合を確かめた
  session だけを反映し、操作にも使う（Issue #1252。規則の正本は ADR 0052 §2）。
- `LiveSignal` は `SessionStarted` と `SessionEnded` の通知だけに使い、viewer 数の正本にはしない。
- viewer presence は `LivePresence` heartbeat と local SQLite projection で扱い、restart 後に自動再 join はしない。

## Consequences
- late joiner と restart 後の復元は `docs state + manifest blob` だけで成立しなければならない。
- 終了済み session は durable state で `Ended` として復元され、新規 join を拒否しなければならない。
- viewer count は shared durable state ではなく local projection の TTL 管理で表現される。

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
- manifest は owner の署名対象となる必須の `revision` を持つ。初版は 1、更新ごとに 1 増やす。同じ key の候補と永続 projection は revision で新旧を判断し、
  `state.updated_at` や record の到着順を使わない。#1260 時点で本番 record は無いため、revision の無い旧形式は移行せず拒否する。
- `LiveSignal` は `SessionStarted` と `SessionEnded` の通知だけに使い、viewer 数の正本にはしない。
- viewer presence は `LivePresence` heartbeat と local SQLite projection で扱い、restart 後に自動再 join はしない。

### 追補: 映像・音声の descriptor（#1034 AC-2a、2026-10-13）
- manifest に任意の `media_endpoint_id`（映像・音声を送る配信者の端末の iroh endpoint id）を足す。desktop runtime（GUI・CLI）の作成では
  作った端末の id を入れ、Web の作成と旧版の manifest は持たない（media の無い session）。owner の署名と revision は既存の manifest の規則のまま。
- 無いときは JSON に出さないので、media の無い session の manifest は従来と同じ bytes になる。旧版は未知の項目を無視して読み、映像・音声を
  扱わない。視聴者は検証済みの manifest の id へだけ購読する（ADR 0067 §3）。配信ごとの署名鍵の公開鍵は #1034 AC-2b が足す。

## Consequences
- 映像・音声の搬送は [ADR 0067](0067-media-transport-moq.md) に従う。manifest の descriptor は上の追補による（分類は ADR 0067 の Feature Data Classification）。
- #1262: state/envelopeの到着による個別反映、表示中だけのmanifest取得、未取得候補と検証済みsessionの区別はADR 0052「Session の個別反映と表示要求」に従う。timerによる反映retryはしない。
- late joiner と restart 後の復元は `docs state + manifest blob` だけで成立しなければならない。
- 終了済み session は durable state で `Ended` として復元され、新規 join を拒否しなければならない。
- 過去に署名された revision を置き直しても、終了済み session の表示と操作は過去へ戻らない。
- viewer count は shared durable state ではなく local projection の TTL 管理で表現される。

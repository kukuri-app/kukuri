# ADR-0041: Metaverse resource budget

- Status: Accepted
- Date: 2026-08-28
- Issue: #794
- 後継: Player と Client の spatial audio の行の測り方と超えたときの扱いは、#1033 で [ADR 0067](0067-media-transport-moq.md) §4 に合わせて置き換えた（旧: signed PCM16 frame、mute/drop oldest）。

## Context

Dome の texture、model、collider、prop、avatar と authoritative physics は、単一 Dome の範囲を越えて host と参加 client の負荷になる。特に Community Node hosting と隣接 Dome 表示では、owner が許容した asset でも各 operator/client が安全に処理できるとは限らない。このため canonical world state とは独立した operational policy として Dome、player、host、client の4 scopeへ上限を設ける。

Metaverse は実験機能であり、asset inspection metadata の追加に伴い `world_version` を6へ上げる。旧 experimental schema の migration と互換 decode は提供しない。

## Decision

### 設定と authority

- 共通設定型は `MetaverseResourceBudgetConfig` とし、Dome、player、host、client の全項目を正の整数で指定する。不正値または安全 hard ceiling 超過は silent clamp せず起動時に拒否する。
- desktop/owner host は `KUKURI_METAVERSE_RESOURCE_BUDGET_JSON`、Community Node は `COMMUNITY_NODE_METAVERSE_RESOURCE_BUDGET_JSON` に完全な JSON object を指定する。部分 override は受理しない。
- Budget は owner-signed Preset/Instance に保存・配布しない。owner host、Community Node operator、各 desktop client がそれぞれ独立して適用する。Instance の `max_peers` が host 設定より小さい場合は小さい方を採る。
- Canonical schema は persistent prop 1024件、participant 512人までの安全上限だけを持つ。実運用の既定値64は host preflight が適用する。

### 既定 profile

| Scope | Resource | Default | Unit / measurement | Exceeded behavior |
| --- | --- | ---: | --- | --- |
| Dome | persistent props | 64 | manifest/session body count | mutation reject |
| Dome | texture stored bytes | 256 MiB | inspected texture blob sum | import/start reject |
| Dome | texture dimension | 8192 | max(width, height) px | import/start reject |
| Dome | model stored bytes | 256 MiB | inspected GLB/VRM blob sum | import/start reject |
| Dome | model triangles | 2,000,000 | glTF primitive accessor count | import/start reject |
| Dome | colliders / rigid bodies | 128 / 256 | active physics objects | mutation reject |
| Dome | snapshots | 10 | Hz | reuse latest snapshot |
| Player | guest props / bytes | 16 / 64 MiB | active owner-attributed guest state | mutation reject |
| Player | avatar asset | 32 MiB | inspected VRM bytes | renderer admission reject/fallback |
| Player | prop spawn | 12 | rolling one-minute window | rate reject |
| Player | interaction | 30 | rolling one-second window | rate reject |
| Player | input bandwidth | 256 KiB/s | signed input JSON bytes | rate reject |
| Player | connection proposal | 8 | proposer owner × receiver instance/slot per 10 min | rate reject |
| Player | impulse | 5000 | maximum absolute component, cm | mutation reject |
| Player | spatial audio | 50 frames/s, 32 KiB/s | 聞き手が受けた話者ごとの Opus の frame。1 秒分の枠を経過時間に比例して補充する | frame を捨て、拒否の件数に数える |
| Host | participants | 64 | active participant count | join reject |
| Host | simulated rigid bodies | 512 | active host physics bodies | mutation reject |
| Host | snapshot bandwidth | 4 MiB/s | signed snapshot JSON bytes | reuse latest snapshot |
| Host | session assets | 512 MiB | all inspected referenced blobs | assignment/start reject |
| Client | rendered avatars | 32 | remote avatar count | primitive fallback then hide |
| Client | texture memory | 512 MiB | width × height × 4 estimate | disable optional textures |
| Client | rendered triangles | 3,000,000 | verified model metadata sum | primitive fallback/hide |
| Client | interpolated bodies | 256 | sorted visible body count | stop low-priority interpolation |
| Client | neighbor Domes | 4 | adjacent Dome count | reduce, fallback, then hide |
| Client | metaverse cache | 1 GiB desktop / 10 GiB CN | content-addressed manifest/asset bytes | unpinned LRU GC or stage reject |
| Client | spatial audio | 16 streams, 8 jitter frames | 購読する話者（近い順）／再生前の保持（jitter frame × 20 ms） | 近い 16 人の外は購読しない／保持より遅れた group は古いものから飛ばし、超える分は書かずに捨てる |

### Asset and physics validation

- Texture は保存前と CN staging 時に同一 bytes から format、寸法、展開後 RGBA byte estimate を検査する。GLB/VRM は magic、version、declared length、bounded JSON chunk、mesh primitive/accessor count を検査する。
- `MetaverseAssetRef.budget_metadata` は検査結果でのみ生成する。未検査、declared size 不一致、metadata 不一致、解析不能、hard ceiling 超過は fail closed とする。CN は受信 metadata を信用せず blob を再検査する。
- Renderer へ渡す VRM/GLB も実 bytes を再検査する。Client planner は verified metadata がない asset を full-quality model として数えない。
- Collider は capsule/cuboid の1 body 1 colliderに限定し、寸法・prop scale は schema safety ceiling 内だけ受理する。Rapier の mass は bounded collider geometryから導出し、外部指定 mass は受け取らない。Impulse は host が mutation 前に上限を検査する。

### Enforcement and degradation

- Shared host runtime は署名と sequence を確認後、state mutation 前に全 budget/rateをpreflightする。拒否された操作はbody、participant、sequence、counterを部分更新しない。ただし abuse accounting 用の受信 byte/rate window は消費する。
- Rejection は `scope/resource/reason/observed/limit` と安定した `METAVERSE_<SCOPE>_<RESOURCE>_<REASON>` code を持つ。CN HTTP/Tauri IPCはこの型を保持し、UIは利用者向け文言へ変換する。
- Client は host admission と独立して deterministic plan を作る。current Dome shellとlocal avatarを維持し、optional texture、remote avatar model、低優先 prop interpolation、neighbor quality、非必須 entityの順に落とす。Budget超過だけでDome session全体を停止しない。

### Metrics and data classification

- Host/CN status は拒否総数とcode別count、participant/rigid-body high-water、snapshot bytes/throttle count、適用中budgetを返す。Clientは現在のdegrade tierを表示する。
- Metric label は低 cardinality の拒否codeだけとし、pubkey、participant/prop ID、blob hash、asset名、raw inputを含めない。
- Budget/metrics は operational/derived dataであり canonical docs へ同期しない。Session終了またはprocess restartでmemory countersを破棄する。

## Consequences

- Owner hostとCommunity Node hostは同じbudget contractとhost preflightを利用するが、operatorは安全 ceiling 内で異なるprofileを選べる。
- Current + 4 neighbor が最大構成でもclientは決定的に品質を落とし、current Domeの基本操作を維持できる。
- 既存ADR-0040の1 GiB/10 GiB cache値は固定仕様ではなく本ADRの既定profileとなる。pin、24時間grace、unpinned LRUだけを削除する規則は維持する。
- 実GPU memoryの計測やhardware別自動profile、Dome間prefetch/transitionそのものは対象外とする。

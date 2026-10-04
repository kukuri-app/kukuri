# ADR-0043: Dome access, block, presence, and spatial audio

- Status: Accepted
- Date: 2026-08-29
- Issue: #795

## Context

Domeは独自のmember、role、moderatorを持たず、配置先のpublic topicまたはprivate channelの資格を継承する。Dome Connection、境界通過、隣接presence、音声が別々の認可規則を持つと、資格失効後の越境やidentity漏えいが起きるため、同じaccess decisionを全経路へ適用する。

## Decision

### Access authority

- Public topicはactive subscription、private channelはcurrent epochのactive participantを参加資格とする。
- `DomeTransitionAccessDecisionV1`をpreviewとauthoritative prepareで共用する。Previewは表示専用であり、15秒のtransition ticketを発行しない。Prepareはactive topology、Instance generation、session、accessを再確認する（所有者本人の端末のprocess内のprepare。Community Node hostと、所有者以外の端末からのP2Pのprepareはtopologyを照合せず、access、block、lease、定員を確かめる。[ADR-0038](0038-dome-hosting-lease-session-lifecycle.md)、#1527）。
- Community Nodeにはchannel secretを渡さない。owner-signed policy、current participant envelope、participant-signedでtarget ownerとSpatial Contextへ束縛した10秒の`DomeSpatialAccessProofV1`を渡す。
- room eventのpublish/listも同じSpatial Context資格とdestination ownerからvisitorへのblockを確認し、失敗時はpresence、音声、identityを返さない。

### Block, mute, and report

- Blockはauthor replica上の方向付き署名済みdurable edgeである。`A blocks B`と`B blocks A`は別状態で、latest signed active/revoked recordから再構築する。
- owner間はいずれ向きのblockでもConnectionを`owners_blocked`でterminal化し、予約済みtransitionを取り消し、対象participantだけをevictする。Unblockはedgeを解除するだけでConnectionを再作成しない。
- destination ownerがvisitorをblockした場合はそのvisitorだけを拒否する。他participantのConnection利用は維持する。
- Muteは受信authorの音声をlocal playback前に破棄するだけで、topology、参加、presenceを変えない。Reportもtopologyを変更しない。

### Presence and audio

- `MetaverseRoomEventV1`の署名付きephemeral hintをpresenceと音声で共用する。Chatだけをroom historyへ保存し、presence/audio frameはdocs、blob、SQLiteへ保存しない。
- Presence/audioのTTLは10秒、presence heartbeatは5秒とする。Clientはcurrent Domeとactiveかつaccess可能な最大4隣接Domeだけをpollし、Connection/access/session変更時にcursor、presence、queued audio nodeを破棄する。
- 音声は16 kHz、mono、PCM16、1 frame最大320 sampleとする。Micは明示操作とOS permission成功後だけ開始する。
- Current Domeは話者とlistenerの直線距離、隣接Domeは「話者→隣接側opening + 現在側opening→listener」の合成距離を使う。Gainは1m以内を1、それ以遠を`1m / distance`として減衰する。
- Audio budgetはplayerのframe rate/byte rateとclientの同時stream/jitter frame上限へ含める。完全JSON設定のため追加fieldを省略したoverrideは拒否する。

## Feature Data Classification

- Durable: signed block edgeのみ。
- Transient: access proof/decision、transition reservation、presence、audio frame、playback node。
- Canonical source: blockはauthor replica、accessはtopic subscription/private channel current epoch、topologyはSpatial Context replica、sessionはactive host。
- Retention: proof/presence/audioは10秒以下。音声bytes、話者ID、block inventoryをlogまたはmetrics labelへ含めない。

## Consequences

Issue #797のparticipant keepalive access再検証、即時eviction、offline grace中のpresence/audio停止は[ADR-0045](0045-dome-offline-draining-return-home.md)で追加定義する。

- Metaverse固有のmember/role/ban modelを追加せず、既存social graphとchannel policyの変更がDomeへ反映される。
- Preview後のblock、leave、epoch rotationでもprepareが再判定するためTOCTOUで越境できない。
- 実験機能のため旧Metaverse wire/configとの後方互換は提供しない。

# ADR-0045: Dome offline、Connection draining、Return Home

- Status: Accepted
- Date: 2026-08-29
- Issue: #797

## Context

Connection recordが存在していても、host停止、通常解除、owner間block、participant access失効により通行できない場合がある。これらを一つの`closed`として扱うと、短い通信断でavatarを失う一方、安全上の失効に不要な猶予が生じる。

## Decision

### Hostとparticipant liveness

- Owner-device hostはP2Pのephemeral hint、Community Node hostはstatus応答で、active lease epochとsessionへ束縛した署名済みheartbeatを5秒ごとに発行する。
- Clientは署名、host identity、epoch、session、単調sequenceを検証し、最新値だけをmemoryに保持する。最終heartbeatから5秒超を`offline`、15秒まではgrace、15秒超を`closed/heartbeat_timeout`とする。
- Clientがgossipで受けたowner device hostのheartbeatは、Spatial ContextとInstanceで引く台帳に、contextとhostからInstanceを導けるものだけを置く。署名時刻（受信時刻より未来なら受信時刻）から30秒（graceの2倍）で捨て、contextごとに100件（部屋の一覧の窓）まで置き、満ちたcontextでは新しいhostを置かない。heartbeatでしか知らないDomeは、15〜30秒は`closed/heartbeat_timeout`を返して入室中の画面を退避させ、30秒で一覧・接続の相手・入口の候補から外れる（#1553。それまでは再起動まで残っていた）。
- Grace中は最後のauthoritative snapshotとadmitted sceneを保持し、interaction、audio/presence送信、新規transitionを停止する。同じepoch/sessionが復帰すれば再Joinせず再開する。
- Participantは5秒ごとの署名済み`KeepAlive`を送る。Community NodeではJoinと同じ短命access proofを再検証し、hostは30秒無入力のparticipantをavatar、grab、seat、reservationと共に除去する。

### Connection lifecycle

- Endpoint ownerによる通常解除はdurable `draining` recordと3秒のdeadlineを先にpublishする。drainingはtopology geometryには残すが、新規preview/prepare/通過は拒否し、未commit reservationをabortする。
- Deadline後に`revoked/owner_revoked`へ確定し、残るrecordからsubcomponentを再計算する。分裂だけでは既存participantをevictしない。
- `owners_blocked`、Instance detach/deleteなど安全またはauthority失効はdrainingを経由せず即時terminalとする。UnblockはConnectionを自動復元しない。

### EvacuationとReturn Home

- Automatic evacuationは成立済み遷移先、readyな隣接Dome、Issue #796のentry候補順でcurrentを除外して評価する。明示Return Homeはown-hostedを含むentry候補を先に評価する。
- 候補ごとに既存のauthoritative `Join`とsafe-spawnを再利用し、target snapshotにlocal avatarが現れるまでsource sceneをcurrentとして保持する。確認後だけcurrentを切り替えてsource `Leave`をbest-effortで実行する。
- 候補が無い場合はsceneを閉じてDome選択へ戻す。理由は`host_offline/access_revoked/blocked/topology_invalid/user_requested`、phaseはtransientな有界状態として扱う。
- 境界は`offline`、`draining`、`blocked`、`closed`を色、barrier形状、文言、残り時間、操作可否で区別する。

Metaverseは実験機能のため、変更したheartbeat、session input、Connection record、IPC schemaに旧形式との後方互換は設けない。

## Feature Data Classification

- Durable: Connection lifecycle status、generation、actor、reason、drain deadline。
- Transient: host heartbeat、participant keepalive/access proof、recovery phase、candidate evaluation、最後のsnapshot。
- Local-only: Issue #796のlast visited Dome。
- Retention: heartbeatは最新1件（gossipで受けたowner device hostの分は署名時刻から30秒、contextごとに100件まで。#1553）、access proofは10秒、participant timeoutは30秒。raw input、proof本文、participant identityを診断logへ保存しない。

## Consequences

- 短いhost断ではavatar/sessionを保持しつつ、安全上の失効は猶予なく閉鎖できる。
- 通常解除の3秒間は境界が閉じてもcomponent座標を保持し、terminal後だけ独立componentになる。
- Community Node間failover、自動lease再割当、prop/seatのDome間退避は行わない。

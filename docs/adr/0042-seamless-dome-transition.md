# ADR-0042: Seamless Dome transition

- Status: Accepted
- Date: 2026-08-29
- Issue: #790

## Context

ActiveなDome Connectionは恒常的なportalではなく、avatarが隣のDomeへ歩いて移動できる地続きのconnection zoneとして見せる。一方、各Domeのphysics authorityは別sessionにあり、propやseatを跨がせるとcross-Dome simulationが必要になる。初期実装はpure player avatarだけをhandoffし、準備不能な境界を閉じる。

## Decision

### Geometry and coordinates

- `fixed_dome_v1`だけをgeometryの正本とする。Connection zoneはwall midpointの2,100cmから隣接側へ1,500cm、境界面とtransition中心線はsource中心から2,850cm、隣接Dome中心は5,700cmに置く。
- Active ConnectionとADR 0037のcomponent-local座標だけからcurrent＋N／E／S／W最大4 neighborを導出する。global座標は作らない。
- Avatar位置は`component position = Dome coordinate + Dome-local position`として保ち、handoff時はtarget Dome座標を引いてtarget-local位置へ変換する。
- Environment progressはconnection zone入口を0、中心線を0.5、出口を1とする整数millionthsで導出し、lighting、ambient light、fog density、gravity strengthだけを線形補間する。

### Boundary and prefetch

- Boundary stateは`closed`、`loading`、`ready`、`denied`、`full`、`unhosted`、`error`、`stale`をtyped valueとして扱う。
- Preflightはactive host／lease／Instance generation、Spatial Context access／block、participant／body capacityの順で評価する。`ready`以外ではavatar colliderを中心線手前で止め、状態を表示する。`ready`ではdoor、portal面、遮蔽物を残さない。
- Hostとaccess preflight成功後にmanifest、asset、初期signed snapshot／presenceをprefetchする。ClientはADR 0041のneighbor／texture／triangle／cache budgetを適用し、current Domeを維持したままneighbor品質を落とす。
- Access／blockはfail-closedな共通gateとする。Issue #790はowner間`owners_blocked`とvisitor単位denyを表せるcontractを提供し、signed blockのcanonical data sourceと失効reconcileはIssue #795が接続する。

### Admission and handoff

- Destination hostはtransition id、Connection id、topology digest、participant、両Instance／generation、target lease epoch／session、expiryへ束縛した短期admission ticketを発行する。
- Provisional reservationは定員へ算入する。Prepare、refresh、commit、abortはtransition id単位で冪等とし、expiry、host restart、lease／session／generation／topology変更で無効化する。
- Source `prepare_transition`はgrabを解除し、seat stateを解除し、base avatar identity／collider以外の追加装着assetをhandoff payloadから除外する。Propとseat bodyはsource側のcollisionに残し、destinationへ送らない。
- Clientはdestination provisional reservationの完了後、hysteresis付きで中心線を進行方向へ越えた時だけcommitする。Destination commitのack後にcurrent Domeを切り替え、source leaveを冪等に完了する。Commit要求を送信した後のtransport errorは適用結果不明として扱い、target lease epoch／sessionが同じ間は同一ticketとtransformでcommitを再送する。Ack取得前にdestination reservationまたはsource prepareをabortしてはならない。
- Commit前の失敗はdestination reservationをabortしてsource側へ戻す。Commit後にsource leaveが失敗した場合はdestinationをcurrentのままsource leaveを再試行し、sourceはtransition idで旧inputをfenceする。これによりclient current Domeを一つ、authoritative参加先を最大一つへ収束させる。

## Feature Data Classification

- Durable: なし。Connection、Instance、Hosting Lease、manifestの既存canonical recordだけを参照する。
- Transient: boundary state、prefetch result、admission reservation／ticket、transition phase、source exit fence。
- Canonical source: topologyはSpatial Context replica、world definitionはInstance／Preset manifest、physicsはactive host session。
- Replicated: transition自体はNo。owner hostまたはCommunity Node hostのmemoryとclient coordinatorだけに保持する。
- Rebuildable: topology、hosting view、manifest、latest signed snapshotから再試行できる。
- Retention: reservationは15秒で失効し、commit／abort／session終了時に破棄する。source exit fenceはsourceの完了・Leave・退去で破棄する（#1538）。raw access proof、token、presence payloadは診断へ出さない。

## Consequences

Issue #797のdraining中の新規transition停止と退避時のsource/target確定順は[ADR-0045](0045-dome-offline-draining-return-home.md)で追加定義する。

- Seamlessに見えてもphysics authorityは中心線で明示的に切り替わり、cross-Dome prop simulationを導入しない。
- Host不在、access不明、block、満員、asset失敗、stale topologyではavailabilityより整合性を優先して境界を閉じる。
- Texture／materialは完全補間せず、隣接Dome表示と数値environment補間を優先する。
- ADR-0043は本ADRのaccess gate、opening座標、boundary state、abort hookを変更せずにpresence／audio／signed blockを接続する。

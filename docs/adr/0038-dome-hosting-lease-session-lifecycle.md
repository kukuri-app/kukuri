# ADR-0038: Dome Hosting Lease と session lifecycle

- Status: Accepted
- Date: 2026-08-28
- Issue: #788、#1527（owner device host への P2P の session 経路）

## Context

Dome の Durable world definition は owner が管理する。一方、実行中の avatar / prop physics は低遅延な単一 authority が必要である。owner 不在時も Dome を公開できるように、owner device または owner が選んだ Community Node の一方だけを active host とする必要がある。

## Decision

### Authority

- Durable world authority は owner であり、Preset、Instance、Hosting Lease、activation、renew、close を owner が署名する。
- Ephemeral physics authority は active Hosting Lease が指す host である。
- Community Node は lease と manifest bundle の operational mirror を持てるが、canonical owner または Durable world source にはならない。
- canonical Hosting record は Instance が所属する SpatialContext replica の append-only object とする。gossip と WebSocket は通知・転送、SQLite と Postgres は再構築可能な projection / operational mirror とする。

### Lease と一意性

`DomeHostingLeaseV1` は lease id、SpatialContext、Instance id / generation、owner、host target、manifest blob hash / version、issued / expiry、単調増加する epoch を含む。lease は owner-signed envelope として検証する。

host 切替は次の二段階で行う。

1. owner が前回より大きい epoch の lease を発行する。この時点で旧 host を fence し、状態を `Transferring` とする。
2. target host が lease digest、epoch、新しい session id を署名して accept し、owner がその acceptance を明示的に activate する。

activation 前の target と、新 epoch 発行後の旧 host は authoritative output を発行できない。owner が online に戻っただけでは Community Node の lease を変更しない。owner device への切替も同じ transfer を使う。

lower epoch、期限切れ、改ざん、Instance generation / manifest hash 不一致を拒否する。同一 Dome / epoch に異なる有効 lease がある場合は split-brain として fail closed にし、どちらも active にしない。owner がさらに大きい epoch を発行した場合だけ復旧する。

### 状態

- `Closed`: active lease がない、close 済み、または期限切れ。
- `Transferring`: 新 epoch の lease はあるが、host acceptance と owner activation が揃っていない。
- `Owner Hosted`: owner device target が activate 済み。
- `Community Node Hosted`: Community Node target が activate 済み。
- `Grace Period`: activate 済み lease に対する heartbeat が失われた、または split-brain を検出した fail-closed view。新 host 権限は発生しない。同一 host の再開か owner の新 epoch 操作でのみ解消する。

### Session lifecycle

- host は activation ごと、または process restart ごとに新しい session id を発行する。
- session は active lease epoch、Instance generation、manifest hashへ束縛する。
- participant が 0 人なら physics step を sleep する。ただし wall-clock sweeper は継続し、guest prop TTL を失効させる。
- Community Node hosted session は participant が 0 人でも lease expiry または owner close まで存続する。
- host restart は exact manifest bundle の initial state から開始し、velocity、grab / seat、transient guest prop、過去 snapshot は復元しない。
- manifest 変更を実行中 session に暗黙適用しない。owner の apply-and-restart が新 epoch と新 session を開始する。

### Physics protocol

participant は movement、grab、throw、push、sit を lease epoch / session / sequence に束縛した署名済み input として host に送る。peer が送った transform / object state は authoritative state として採用しない。

host は検証済み input を共通 Rust runtime へ適用し、host-signed snapshot を配信する。client prediction / interpolation は表示上の補助であり、active host signature、lease epoch、session id、sequence の検証に失敗した snapshot を破棄する。

#### input の期限と participant ごとの状態（#1538）

- 共通 runtime は、署名時刻 `sent_at` が host の時計で 10 秒（`DOME_ACCESS_PROOF_TTL_MILLIS`。Join と KeepAlive の access proof と同じ）を過ぎた input を `DOME_SESSION_INPUT_EXPIRED` で拒否し、署名者ごとに sequence が前回より大きい input だけを適用する。
- participant ごとの状態（budget の窓、seat、遷移の準備・到着・commit、最後の input）は参加中だけ持ち、退出（Leave、遷移の完了、30 秒の participant timeout、access 失効）の 1 か所で消す。拒否した Join と、participant でない署名者の input は状態を残さない。到着の commit は participant ごとに最後の 1 件だけを残す。
- 退出した署名者の最後の sequence だけは、受け付けた input の署名から 10 秒たつまで、participant の上限の件数まで残し（超えたら退出の古い順に捨てる）、同じ session での古い input の再送を拒否する。期限を過ぎた input は署名時刻で拒否するため、記録を消した後も再送は通らない。
- participant でなくても persistent prop を変えられる owner の sequence は、session の間保つ。
- 退出済みの署名者からの遷移の完了は、何もせず成功とする（source の完了の再送を冪等にする。ADR-0042）。遷移で出た Dome へ、後から Join で入り直せる。

### Owner device host への P2P の session 経路（#1527）

host session stream は、host の種類ごとに次の経路を使う。所有者本人の端末は process 内で処理し、Community Node host は HTTPS で処理する（どちらも変更しない）。所有者以外の端末の participant が `OwnerDevice` lease の host へ input を送る経路は、次の P2P の要求・応答とする。

#### 接続と wire

- 専用の ALPN `/kukuri/dome-session/1` の QUIC 接続を、participant の端末と host の endpoint の組ごとに 1 本使う。要求ごとに双方向 stream を 1 本開き、JSON の要求を 1 件（上限 256 KiB）送って、JSON の応答を 1 件受け取る。応答の上限は 8 MiB（rigid body の安全上限の snapshot を収める）、再同期の応答だけ 16 MiB（既定の budget の ring 100 件を収める）とする。
- 要求は次の 5 種とする。participant input と snapshot は既存の署名済みの型をそのまま運ぶ。
  - `input`: `SignedDomeSessionInputV1`。応答は `SignedDomePhysicsSnapshotV1`。
  - `resync_snapshots`: Instance、`after_sequence`、`DomeSpatialAccessProofV1`。応答は ring の `SignedDomePhysicsSnapshotV1` の列。
  - `prepare_transition`: `DomeTransitionAdmissionRequestV1` と `DomeSpatialAccessProofV1`。応答は admission ticket。
  - `commit_transition`: ticket と遷移先の位置・回転。応答は受理。
  - `abort_transition`: ticket。応答は受理。
- 拒否の応答は理由を持つ。resource budget の拒否は ADR-0041 の `MetaverseResourceRejection` の型のまま返し、participant の端末は process 内の host と同じ code で画面へ渡す。
- Join には admission 用の snapshot（stream の頻度制御で直前の snapshot を再利用しない。ADR-0044）を、それ以外の input には stream の最新の snapshot を返す。host は participant ごとの送信 queue や snapshot の push を持たず、応答は常にその時点の最新の 1 件になる。participant の画面の他者の avatar は、自分の input の応答（移動中は host の snapshot 頻度、静止中は keepalive の 5 秒ごと）で更新される。これは process 内の host と Community Node host の現行の挙動と同じである。
- 再同期は ring（最大 100 件、#793）のうち `after_sequence` より後を返す。16 MiB を超える場合は、新しい側から上限に収まる分だけを sequence 順に返す。既定の budget では ring の全件が収まり、host が budget を引き上げた場合だけ件数が減る。

#### host での確認

確認点は Community Node host と同じにし、署名・lease・sequence・budget の検証は同じ共通 runtime（`DomeSessionRuntime`）で行う。

- すべての input: participant の署名、lease の Instance・generation・epoch・session、participant ごとに単調な sequence、resource budget を検証する。
- Join と KeepAlive: owner device が自分の状態で current Spatial Context access と owner からの block を再評価する（ADR-0044）。channel の状態を持たない Community Node と違い、access proof は要らない。
- Join 以外の input: 署名者が現在の participant でなければ、runtime の状態を変えず、snapshot も返さずに拒否する。process 内の host と Community Node host の挙動は変えない。
- `resync_snapshots`: access proof の署名で要求者本人と、Spatial Context・owner への束縛と有効期限を確かめ、要求者が現在の participant であること、Join と同じ access と block を満たすことを確かめてから ring を返す。
- `prepare_transition`: access proof の署名で participant 本人と、Spatial Context・遷移先 owner への束縛と有効期限を確かめる（Community Node と共通の helper）。そのうえで owner device が topology・access・block を再評価して ticket を発行する（ADR-0042 / 0043）。
- `commit_transition` / `abort_transition`: host が発行した ticket と runtime の予約の一致、lease epoch・session・期限で判定する（ADR-0042）。ticket はこの暗号化された接続でだけ要求者へ渡る。

#### 接続先の解決

- 接続先は、現在の owner 署名の lease の `OwnerDevice.endpoint_id` だけとする。QUIC の handshake が endpoint を、lease の `host_pubkey` による snapshot の署名の検証が host を確かめる。heartbeat や他の peer の申告で接続先を変えない。
- 到達情報は既存の経路だけから得る（接続中の経路、ticket・seed・topic rendezvous・接続済みの peer から得た address、有効な場合の DHT）。この経路のために新たな address の公開はしない。
- 候補は既存の transport と同じ順で試す。relay URL を含まない直接の IP 候補（Direct P2P）を先に試し、成立しなければ設定中の relay URL を含む候補（Relay Supported P2P）を試す。hole punching が成立せず実データが relay を経由する場合は Relay Fallback として扱う。
- 接続の確立と、要求 1 件の往復は、それぞれ 10 秒（Community Node の HTTP と同じ）で打ち切る。接続できない場合は code `DOME_HOST_UNREACHABLE` の失敗とする。

#### 上限と切断

- participant 数・resource budget・keepalive 5 秒・30 秒の participant timeout は、ADR-0041 / 0045 のまま共通 runtime が適用する。接続が切れただけでは participant を除去せず、input の途絶で除去する。
- host は同時接続を 512（participant の安全上限）までに制限し、1 接続の要求は順に 1 件ずつ処理する。30 秒要求の無い接続は host が閉じる。iroh stack を作り直したときは、受け口を新しい node へ付け直す。
- 要求ごとの処理は、session の索引と、access・block の key 指定の 1 件の読み出しだけにする。participant 数・topic 数・block 台帳の件数に比例する読み出しを持たない。
- participant の端末が保持する接続は、current Dome と隣接 4 Dome の host の 5 本まで（ADR-0041 の neighbor 上限）とする。失敗した接続は捨て、次の要求で張り直す。

#### 未到達時の表示

- 稼働中（heartbeat が新しい owner hosted）の Dome は、従来どおり入室できる候補として表示する。到達できるかは接続を試すまで分からないため、事前の確認はしない。
- 入室が `DOME_HOST_UNREACHABLE` で失敗したときは、code ではなく「所有者の端末に接続できませんでした。時間をおいてもう一度お試しください。」を表示する（日本語・英語・中国語）。自動入室では次の候補へ進む（ADR-0044）。

## Feature Data Classification

| Data | Authority | Canonical store | Sync / transport | Local / Node cache | Retention / delete |
| --- | --- | --- | --- | --- | --- |
| Hosting Lease / activation / close | owner signature | SpatialContext replica | docs sync、gossip hint | SQLite projection、Postgres operational mirror | append-only。Instance tombstone / move後は無効として保持し、通常GC規則に従う |
| host acceptance | target host signature | SpatialContext replica | docs sync、gossip hint | SQLite / Postgres | 対応するlease recordと同じ |
| heartbeat | active host signature | なし | gossip / WebSocket | memory latest only | grace判定後に破棄 |
| participant input | participant signature | なし | host session stream（owner device host へは P2P の `/kukuri/dome-session/1`、Community Node host へは HTTPS） | host memory queue | 適用またはreject後に破棄。署名者ごとの最後の sequence は参加中と、退出後は受け付けた input の署名から 10 秒まで（participant の上限の件数まで）、owner は session の間 host memory に残す。raw inputをlogへ出さない |
| physics snapshot | active host signature | なし | host session stream（input と再同期への応答。owner device host からは P2P、Community Node host からは HTTPS） | client / host memory latest only | session終了または置換で破棄。ring bufferは#793 |
| guest prop expiry metadata | active host | なし | snapshot | host memory | wall-clock expiryまたはsession終了で破棄 |
| Community Node assignment mirror | owner / host署名済みrecord | なし | HTTPS | Postgres | lease expiry / close後にoperational retention規則で削除 |

秘密鍵、bearer token、participant raw input は永続化・診断出力しない。Dome id、epoch、lease digest、host id、session id、heartbeat timestamp、reject reason は診断可能とする。P2P の session 経路では、要求の種類、拒否の理由、接続の経路の分類（Direct P2P / Relay Supported P2P / Relay Fallback）も診断可能とし、access proof と ticket の本文は出力しない。

## Community Node boundary

Community Node hosting は `dome_hosting` capability を明示的に有効化したNodeだけが提供する。既定は無効とする。Node signing key の公開鍵は manifest の `node_id` と一致しなければならない。authenticated / consent 済み owner だけが assign、renew、releaseできる。

Postgres は Dome ごとに active assignment を最大一件に制限する。Node restart は有効な lease と exact manifest bundle を再検証して新 session を開始するが、ephemeral simulation state は復元しない。

#1020ではassign/activate/releaseのassignment、blob pin、runtimeを一つのlifecycle操作として直列化する。同じserver processの排他に加え、同じDBを使うserver間ではInstance IDに対するPostgres advisory transaction lockを保持する。旧epochのreleaseがDBをcloseした後に新generationのruntime・pinを消さないことを、DB closeとcleanupの間にbarrierを置くcontractで固定する。auth/consentとowner/epoch照合は従来どおり維持する。

期限切れruntimeを除去するstatus取得も同じlifecycle境界でassignmentを読む。古いstatus応答の処理が新generationのruntimeを除去しないことを並行contractで固定する。GUIの開始・停止・委譲要求は管理対象の`expected_generation`を送信し、app-apiは署名済みInstanceのgenerationと一致しない要求を最初のlease mutation前に拒否する。CLIの省略は、現在のInstanceを対象とする既存の明示操作モードを維持する。

## Consequences

Issue #797の5秒heartbeat、15秒offline grace、30秒participant timeoutは[ADR-0045](0045-dome-offline-draining-return-home.md)で追加定義する。

- ownerの明示操作なしにavailabilityを優先する自動failoverは行わない。
- transfer中またはsplit-brain検出時は一時的にDomeへ入れなくても、二重authorityより安全側を選ぶ。
- metaverseは実験機能のため、旧peer-authoritative wire contractとの互換decodeやmigrationは提供しない。
- 所有者以外の端末の participant も、owner device host の Dome へ入室・滞在・遷移できる（#1527）。この ALPN に応じない旧版の端末が host の Dome へは `DOME_HOST_UNREACHABLE` で入室できず、旧版の端末からは従来どおり入室できない。保存データの形は変えない。
- snapshot retentionとresource budgetはADR-0041、transitionはADR-0042、access / block / presence / audioはADR-0043がこのcontractを利用する。

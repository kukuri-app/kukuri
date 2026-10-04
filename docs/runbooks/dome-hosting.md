# Dome Hosting 運用

## 有効化

Community Node の `features.dome_hosting` は既定で無効。利用する Node だけ `true` にし、`COMMUNITY_NODE_DOME_HOST_SIGNING_KEY` に Node 専用の kukuri secret key を設定する。起動時に公開鍵が manifest の `node_id` と一致しない場合、user-api は fail closed で起動を拒否する。

Hosting API は既存の bearer authentication と consent gate の内側にある。lease、acceptance、activation、close はログへ本文を出さず、bearer token と participant raw input も記録しない。

## 割当と切替

1. Dome owner が desktop の Hosting panel で owner device または Community Node を明示選択する。
2. CN 委譲では client が owner-signed lease と exact Instance/Preset manifest bundle を `/v1/dome-hosting/assignments` へ送る。
3. Node の署名済み acceptance を owner が Context replica に追記し、owner activation を `/v1/dome-hosting/activate` へ返す。
4. `CommunityNodeHosted` になった後だけ Node が input を受理し、host-signed snapshot を返す。

owner が online に戻っても自動 reclaim はしない。「この端末でHostingを開始」は epoch を進める明示 switch-back/renew であり、旧 Node の lower epoch output は client 検証で拒否される。

## 確認と終了

- `GET /v1/dome-hosting/status/{instance_id}` で state、epoch、session、participants、sleep、expiry を確認する。
- participant が 0 の間も assignment は lease expiry/close まで残り、physics だけ sleep する。
- desktop の「Hostingを終了」は owner-signed close を canonical replica に追記してから Node の `/release` を呼ぶ。Node に到達できなくても canonical close と expiry が authority を失効させる。

## 所有Domeの管理・削除（#1020）

停止中でも一覧の「Domeを管理」から再開できる。管理を開いただけでは入室やhost切替をしない。「この端末で稼働を開始して入室」は明示操作で新epochを発行し、authoritative admission後に空間へ移る。稼働済みなら入室だけを行う。

「稼働を終了」はDomeを保持する。「Domeを削除」は確認後にInstanceを失効させ、同じContextで再作成できるようにする。失敗時は同じoperation IDとgenerationで再試行する。再起動後は一覧上部の未完了操作から再試行できる。CN cleanup pendingはlocal削除済みと区別し、旧leaseへ署名したreleaseを既存auth/consent経路で再送する。別Nodeへの切替や同意の迂回は行わない。

履歴・共有Preset/素材のretentionはADR 0036/0040に従う。他peer上のcopyや全assetの即時消去を保証する操作ではない。確認用scenarioは`cargo xtask scenario desktop_smoke_metaverse_dome_delete`。

## Prop とレイアウト保存（操作）

- persistent prop の追加・削除は owner だけが active session に送信できる。session 中の位置・回転は一時状態であり、「現在のレイアウトを保存」を実行するまで manifest は変わらない。
- guest prop は参加者が追加でき、5分の wall-clock TTL または session 終了で消える。layout candidate と durable manifest には含めない。
- 保存時は active host が physics tick 境界で persistent prop だけを抽出して署名し、owner が候補を検証・署名した後に新しい manifest revision をpublishする。同一レイアウトはno-op、変更を伴う保存は30秒に1回まで。
- 保存成功後は同じhost targetでlease epoch/sessionを更新する。新sessionは保存されたtransformから速度0、grab/seatなし、guestなしで開始する。
- late joinまたは欠落検知時はdesktopの「Physics snapshotを再同期」で最大100件のmemory-only ringから再取得する。ringより古いsequenceの場合は最新baselineを返す。
- 別の端末で稼働中のDomeでも、入室中の参加者は同じ操作で取得できる（P2Pの経路。受信の上限16 MiBを超える分は新しい側だけ）。入室していない端末の要求は拒否される。

## 隣接Domeへの遷移

- clientはactive topologyから最大4方向の隣接Domeを解決し、host状態、空きcapacity、参照assetを先読みする。`ready`以外の境界はconnection zone中心線の10 cm手前で閉じる。
- avatarがconnection zoneへ入ると、送信元hostへ`prepare_transition`を送り、grabとseatを解除して以後のinteractionをfenceする。宛先hostにはconnection ID、topology digest、両Instance generation、participantを結び付けた15秒のadmission reservationを要求する。
- 中心線通過時は宛先commitを先に確定し、component座標を保った宛先local transformでavatarを生成する。その後に送信元`complete_transition`を再試行する。中心線前の後退や失敗は宛先reservationと送信元fenceをabortする。
- transition APIはowner-device hostとCommunity Node hostで同じticket/runtime contractを使う。Community Node endpointは既存bearer authenticationとconsent gateの内側にある。宛先が別の端末の所有者の端末で稼働中のときは、入室と同じP2Pの経路でprepare/commit/abortを送る。宛先の所有者の端末はCommunity Node hostと同じくtopology全体を照合せず、本人、access、block（visitorとowner間）、定員を確かめる。
- persistent/guest propは遷移しない。host physicsはprepared/admitted avatarだけにconnection zoneを許可し、propは開口部を含めて送信元半球内へ拘束する。

調査時は、participant raw inputを記録せず、transition ID、connection ID、topology digest、source/target generation、target lease epoch/session、boundary state、denial codeだけを採取する。`DOME_TRANSITION_STALE_TOPOLOGY`はtopology再取得（宛先が所有者の端末のときは、所有者の端末が遷移元のDomeを一覧の行か受信中のheartbeatで知らない場合にも返る。遷移元がCommunity Node hostで、所有者の端末にその行が無い場合など）、`DOME_TRANSITION_CAPACITY_FULL`は退出待ち、`DOME_TRANSITION_INVALID_TICKET`は15秒以内の新規prepareで復旧する。宛先commit後に送信元cleanupだけが失敗した場合、宛先をcurrentとして維持し、送信元completeを再試行する。

## Spatial Contextへの入場

- Clientはactive hostを持つDomeを、own hosted、confirmed last visit、private channel設定entry、Instance ID安定順で試す。優先候補がoffline、満員、access失効、asset不備の場合は次候補または選択一覧へfallbackする。
- Channel entry Domeはchannel ownerだけが同じContextのInstanceへ設定できる。設定変更は既存Connectionを作成、解除、変更しない。
- Owner hostはcurrent access/blockを、Community Node hostは短命access proofを`Join`直前に確認する。Host snapshotにlocal avatarが現れるまでClientはscene、presence、音声を開始しない。
- Hostはmanifest default spawnから固定順でavatar/prop colliderと25 cm安全余白を検査する。安全候補が無ければ`DOME_ENTRY_NO_SAFE_SPAWN`となり、participantとavatar bodyは追加されない。
- 所有者の端末で稼働中のDomeへ別の端末から入るときは、leaseの`OwnerDevice.endpoint_id`へP2Pで接続する（ALPN `/kukuri/dome-session/1`、[ADR-0038](../adr/0038-dome-hosting-lease-session-lifecycle.md)）。所有者の端末はaccessとblockを自分で再評価し、入室していない端末のJoin以外の入力を拒否する。
- 10秒以内に接続できない場合、入室は`DOME_HOST_UNREACHABLE`で失敗し、画面は「所有者の端末に接続できませんでした」と表示する。所有者の端末が起動していること、同じtopic（またはchannel）に参加していること、relayを含む接続設定を確認する。所有者の端末がこの経路に対応しない旧版の場合も同じ表示になる。
- 経路の分類は`kukuri_connectivity`のlog「Dome session connection established」の`path`（`direct_p2p` / `relay_supported_p2p` / `relay_fallback`）で確認する。inputとaccess proofの本文はlogに出ない。

## Manifest/asset cache

- 管理対象はmetaverse manifestとそこから参照されるasset blobだけ。physics snapshot、session state、DB metadata、GPU resource、metaverse以外のblobは容量計算に含めない。
- 既定上限はdesktop 1 GiB、Community Node 10 GiB。content hashで重複排除し、`staging`、`current`、`active_lease`、直近3版の`rollback`参照をpinする。
- pinが一つでもあるblobは削除しない。全参照解除から24時間のgrace後にだけlocal GC対象とする。容量不足時もcurrent/active/stagingを退避せず、新規stagingを失敗させる。
- P2Pで既に取得された他peer上のコピーは強制消去できない。保証範囲は新規配布の停止と各nodeのlocal unpin/GCまで。

## Resource budget

- owner desktopは`KUKURI_METAVERSE_RESOURCE_BUDGET_JSON`、Community Nodeは`COMMUNITY_NODE_METAVERSE_RESOURCE_BUDGET_JSON`へ[ADR-0041](../adr/0041-metaverse-resource-budget.md)の完全なJSON objectを設定する。部分object、不正値、安全上限超過は起動失敗になる。
- `GET /v1/dome-hosting/status/{instance_id}`の`resource_budget`で適用値、`resource_metrics`で拒否総数/code別件数、participant/rigid-body high-water、snapshot bytes/throttleを確認する。metricにplayerやassetの識別子は含まれない。
- `METAVERSE_*_RATE_EXCEEDED`はwindow経過後に再試行できる。`LIMIT_EXCEEDED`または`UNVERIFIED_ASSET`はasset/config/scene構成を修正してから再割当する。上限引上げはADRのhard ceiling内に限定する。
- Clientの`reduced` / `fallback` / `minimal`表示はlocal描画budgetによる段階的劣化で、host session停止を意味しない。current Domeの基本操作を確認し、optional texture/avatar/propを減らすかlocal budgetを安全範囲内で調整する。

## 再起動と障害復旧

Node 再起動時は有効な lease と保存済み manifest bundle を再検証し、新しい session id、manifest initial transform、velocity 0、grab/seatなし、guest propなしで開始する。

`GracePeriod` または split-brain を観測した場合は、秘密情報を採取せず Context/Instance、lease epoch/digest、target host、session、last heartbeat、rejection reason を確認する。owner が同一 host を再開するか、より高い epoch で明示切替する。DB 行や replica record の手動書換えは行わない。

## Offline、draining、Return Home

- Hostは5秒ごとに署名済みheartbeatを発行する。5秒超の欠落では境界が`offline`となり、15秒まで同じsessionの復帰を待つ。期限後は`closed`となり、Clientはready隣接Domeからentry候補の順に安全退避する。
- Grace中は最後のsceneを表示したままinput、presence/audio、新規transitionを停止する。同じlease epoch/sessionが復帰すれば再Joinは不要。
- Participant keepaliveは5秒、host cleanupは30秒。Community Nodeでkeepaliveがaccess deniedになった場合は対象participantだけを退避させる。
- 通常のConnection解除は3秒`draining`となる。新規通過は止まるが、terminal revokeまではcomponent座標を保持し、revoke後も既存participantを分裂だけで退去させない。
- owner間block、Instance失効は即時`blocked/closed`。unblock後にConnectionは自動復元されない。
- Return HomeはHUDの家アイコンから実行する。target admission確認前にsource sceneが消えないこと、候補なしではDome選択へ戻ることを確認する。

診断ではContext/Instance、Connection ID、lease epoch/session、heartbeat age、recovery phase/reason、denial codeだけを採取し、heartbeat/access proof/raw input本文を保存しない。

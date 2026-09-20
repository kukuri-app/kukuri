# Community Node indexer OOM・再起動ループの復旧

- 対応日: 2026-09-20。時刻は特記しない限り JST。
- 対象: GCP `kukuri-cn` / `asia-northeast1-a` / `kukuri-cn-vm`。
- 依頼: indexer healthy=0、last ingest age=1000000000 の調査・対応。
- 変更範囲: 本番 indexer のコンテナ再作成・残存プロセス終了・再起動、readiness / monitor の実行。本記録以外のリポジトリ変更、コミット、イメージ更新は行っていない。

## 原因と修正前の証拠

1. 14:54:28、kernel が global OOM を記録し、旧 indexer コンテナ内の `app`（PID 3720126）を終了した。anon RSS は 2,009,472 KiB。VM は約3.83 GiB、swapなし。同居する ArcadeDB は調査時点で約1.41 GiBを使用していた。indexer のメモリ増加の具体的な原因は未確定。
2. Docker は旧コンテナ `6c35e75de122…` について `cannot delete running task` / `task must be stopped before deletion` を繰り返した。15:09時点で `Restarting (137)`、restart count=23。直近の `OOMKilled=false` だけでは先行する kernel OOM を否定できない。
3. 監視スクリプトは `/v1/status` から `last_ingest_at` を取得できないと `1000000000` を送る。実際に10億秒経過したという意味ではない。Cloud Monitoring でも15:10:41のhealthy=0 / age=1000000000を確認した。
4. 再作成後、旧コンテナ所属の残存 `app`（PID 3869206）が `docs.redb` / `blobs.db` のFLOCKを保持していた。新コンテナはHTTP healthyでもworker=falseで起動待ちだった。`/proc/<pid>/cgroup` と旧コンテナID、実行ファイル、親shimを照合して特定した。

同日午前の [取り込み遅延調査](2026-09-20-cn-recovery-ingest-latency-investigation.md) とは区別する。今回は kernel OOM と containerd / Docker の後片付け失敗を直接観測した。本番revisionは `4b7519460e8baf3c29d7ec0eadc6eb2c727a1065` 系の既存imageであり、ローカルの #1223 を配備した結果ではない。

## 復旧操作

- Composeのindexer imageと稼働imageが同じdigestであることを確認した。
  `ghcr.io/kukuri-app/kukuri-cn-indexer@sha256:066a220a6d2dd88087c28d395e5dcb0665cc51e3de7399bd4dcfd13bf2937160`
- 15:10:42、既存Composeから `up -d --no-deps --pull never --force-recreate cn-indexer`。bind mount `/var/lib/kukuri/cn-indexer`を維持した。
- 15:12:32、旧コンテナへの所属を再確認したPID 3869206だけにSIGTERM。終了とファイルロックの解放を確認した。
- 新indexerが起動待ちを継続していたため、`docker restart -t 10 community-node-cn-indexer-1`。15:13:12に起動完了。
- 15:13:48、`kukuri-monitor.service` を手動実行し成功。
- 15:14:09〜10、`kukuri-readiness.service` を手動実行し成功。

API / relay / Postgres / ArcadeDB / Valkeyの再起動、永続データ削除、閾値緩和、readiness gateの迂回は行っていない。旧containerd状態に起因する問題に対し、Docker daemon全体やVMの再起動は不要だった。

## 復旧確認

- 初回全4scope巡回は15:13:26に終了。`last_pass_duration_ms=13248`、scanned=96、indexed=86、last_error=null。
- healthy、restart count=0。15:13:23のindexer memoryは12.89 MiB。
- readiness: `ready=true fail=0 unknown=0`、truth=86 / projection=86、判定無し索引=0 / 非許可・重大の表出=0 / 失敗→許可=0。
- scan_errors=1、media_fetch_unavailable=2は残る。全投稿・添付の取得成功を保証する結果ではない。
- 公開discovery endpointは401 `AUTH_REQUIRED`。activation gateの解除を確認したが、認証済みclientでの結果表示確認の代替にはしない。
- Cloud Monitoring APIで15:13:48の `indexer_healthy=1` / `indexer_last_ingest_age_seconds=23` を確認した。通知サービスのincident closeやメール配送自体は未確認。
- readiness / monitor timerには次回実行予定あり。

## 継続確認と残課題

- 次の定期全4scope巡回も15:18:38に完了。`last_sync_at=last_ingest_at=1789885118`、`last_pass_duration_ms=12144`、累積scanned=192 / indexed=172、last_error=null。15:18:51時点でhealthy / restart count=0、indexer memory=15.02 MiB。復旧後の追加kernel OOMは15:16:53時点で0件。
- 2巡目で累積scan_errors=2 / media_fetch_unavailable=4。これらの取得・検査失敗は応急復旧では解消していない。
- 15:19:16の自動readinessも `ready=true fail=0 unknown=0` / truth=86 / projection=86、15:19:17にservice成功。手動の一時的なactivationだけでなく、timerによる継続更新を確認した。
- メモリ増加の根本原因と長期的な再発防止は未解決。今回のコンテナ復旧を恒久修正とは扱わない。実装対策ではメモリ増加の再現、indexer / 同居DBの資源予算、OOM後の自動復旧を分けて検証する。
- 本番運用の復旧と文書追加のみのため、Rust / frontend build・testは実行していない。復旧前後の実機状態・kernel / Docker journal・status・readiness・Cloud Monitoringを検証根拠とする。

## #1206との関連評価

ユーザー依頼により [#1206](https://github.com/kukuri-app/kukuri/issues/1206) の本文・コメント（0件）、本番revision / Issue基準commit / HEADの依存、解決済み依存ソースを照合した。結論は「間接的な関連候補だが、今回のOOMとの因果関係は未確認」。Issueの変更や本番設定変更は行っていない。

- #1206は9/18 15:32:54 UTCのクライアントログで `Dropping received relay packet: no available capacity` が489件集中した事象。今回のindexer OOMは9/20 05:54:28 UTCであり、同時刻・同一プロセスの観測ではない。原本添付ログの全件再解析は今回行っていない。
- 本番revision `4b751946`、#1206基準 `361791d3`、HEAD `e08743fb` のCargo.lock上のiroh / iroh-relayは1.0.3で、当該依存の更新差分はない。CN indexerも共有iroh nodeを使用するため、同じ受信経路を通る可能性はある。
- `iroh-1.0.3/src/socket/transports/relay/actor.rs:681` の `handle_relay_msg` が受信datagramを `try_send` し、失敗時に警告する。`relay.rs:54` のキュー容量は512件。consumerは同ファイルの `poll_recv` → `poll_recv_queue` → channel `poll_recv`。`no available capacity` はTokio `TrySendError::Full`。このメッセージはrelayサーバー自体のメモリ不足を意味しない。
- 満杯のdatagramは破棄され、このキューが無制限に保持する経路ではない。そのため「489件の警告→キュー無限増加→約2GiBのRSS」を直接の説明にはできない。キュー以外のtask / buffer / allocatorの保持は別途調査対象。
- 仮説A: drop→QUIC再送・取得再試行→処理滞留や別buffer増加→メモリ圧迫。仮説B: memory / CPU圧迫・runtime consumer停滞→queue満杯とdrop、最終的にOOM。双方とも未検証で、警告だけでは因果の向きを決められない。
- CNは `crates/cn-runtime-support/src/lib.rs:8` のfmt subscriberを使用する。desktop専用のログバッファがCN内で無制限に増える、という説明には根拠がない。ただし大量の同期ログ出力によるruntimeへの負荷は切り分け対象。
- 15:22:28時点、復旧後indexerのDockerログ（06:13 UTC以降）で当該警告は0件、Docker statsのコンテナmemoryは約15 MiB。これは復旧後の観測であり、障害前の不存在を証明しない。旧コンテナ再作成前に採取した末尾ログは短く、全ログを退避していない。Cloud Loggingの障害時間帯を対象とする当該文字列検索も0件だったが、Dockerログ全件が収集されていることを確認していないため否定材料にはしない。

次の原因調査では、同一indexerプロセスについてRSS / cgroup memory、relay drop件数、consumer遅延、接続・取得中task数、ingest進捗を同じ時系列で観測する。consumer停止→再開と受信集中を有限の負荷で再現し、負荷後にメモリと通信が回復するか確認する。#1206にはこの共通transport境界の検証を対応付け、OOM後のDocker/containerd残存・ロック問題は別の復旧境界として扱う。原因未特定のqueue増量はメモリ余力を減らすため採用しない。

## セッション内の継続観測: 索引APIの再停止

ユーザー依頼により15:26から、本番の設定やプロセスを変えずに継続観測を開始した。約50秒間隔でcontainer state、cgroup memory / memory.events、プロセスRSS、host MemAvailable、indexer status、区間内のDockerログ分類、kernel OOM、readiness / monitorの結果を採取。稼働container IDは `9167028374f0…` で一致している。JSONLと退避したDockerログはローカル一時ディレクトリ `kukuri-indexer-observe-20260920-152823` に保持し、本文・識別子を含み得る生ログはリポジトリへ置かない。

### 15:43時点の判定

**OOM・再起動ループの再発はないが、全巡回の長期化で公開索引が再停止した。** 応急復旧直後の2巡成功を継続可用性の保証とは扱わない。

- 15:28:46に次の全巡回を開始。bootstrap peerはactive=5 / applied=5。
- general_jaは15:29:38、devは15:29:48、testは15:31:59に完了。その後generalの取り込み中で、全巡回は未完了。
- 15:30:41の自動readinessは合格、truth=projection=87。15:36:17も成功。
- 前回の全巡回完了は15:23:46（`last_sync_at=1789885426`）。15:38:46を過ぎると900秒の鮮度条件を超える。
- 15:40:06のDB集計で87件、最新indexed_at=15:39:57。したがってワーカー全体が停止した証拠ではなく、巡回中の部分的なDB更新はある。この時点の公開discoveryは401 `AUTH_REQUIRED`。
- **15:41:40にreadinessが `ready=false fail=1 unknown=0`。唯一の不合格は `indexer_ingest_fresh`。truth=projection=87は一致。15:42:23と15:43:39に公開discoveryが404 `INDEX_QUERY_NOT_ACTIVATED`となることを確認した。** ユーザーからの「発見ができない」という申告に対応するサーバー側の失敗である。
- 15:43:39もworker_running / ingest_enabled=true、4scope open、last_error=null、container healthy / restart count=0。これらは取り込み鮮度や公開索引の可用性を保証しない。
- 観測中のcgroup memory.currentは概ね17〜20 MiB、RSSは約46 MiB。Docker statsのmemoryとプロセスRSSは別の指標。kernel OOM / cgroup oom / oom_kill / relay受信dropは0で、この再停止を#1206のdropやOOMの再発とは扱わない。
- 15:26から15:42過ぎまでに退避したログではconnect timeout=167、ephemeral transfer success=78、relay drop=0。成功もある一方で直列の接続待ちが累積している。カウント範囲は先行する初回復旧ログとは別。

この観測依頼では再起動の反復、閾値緩和、activationの強制、image更新は行っていない。以後の巡回完了・自動復帰と観測終了は以下に記録する。

### 全巡回完了と自動復帰

- 15:45:02に全巡回が完了。`last_pass_duration_ms=975250`（16分15.250秒）、累積scanned=386 / indexed=346。先行する短時間巡回と異なり、900秒の鮮度予算を超えた。
- 15:47:19に自動readinessが `ready=true fail=0 unknown=0` / truth=projection=87へ復帰。15:47:27に公開discoveryが401 `AUTH_REQUIRED`となりactivation gateの解除を確認した。readiness上の無効期間は15:41:40〜15:47:19（5分39秒）。認証済みclient画面の復帰時刻は未確認。
- この期間もrestart count=0、kernel OOM / relay drop=0。再起動や手動activationを行わずに回復した。

### #1212の改善適用可能性

ユーザー質問に基づき [#1212](https://github.com/kukuri-app/kukuri/issues/1212) と [実装PR #1223](https://github.com/kukuri-app/kukuri/pull/1223)、ローカル実装を照合した。IssueはComplete / Closed、PRは `e08743fb3ed9271ba8d6a777f7c04bc6efadd283` としてmerge済み。本番image revisionは修正前の `4b751946…` で、今回の観測は修正の本番評価ではない。

- peerの成功実績による順位付け・失敗backoff、remote blob取得全体30秒の上限、scope内投稿の既定4並列は、今回確認した接続待ちの累積と後続投稿の待ちを減らす直接的な対策となる。
- 接続不能なpeerや存在しないblobを取得可能にする修正ではない。readinessのlast_sync_at / last_ingest_at双方900秒という条件は変更されていないため、全巡回が必ず予算内になる保証はない。
- 本番rollout後の巡回時間、失敗/取得不能の割合、複数巡回にわたるactivation継続、並列化後のメモリpeakを比較する必要がある。OOMの根本修正としての効果は別途未確認。

### ユーザー指示による観測終了

15:50台のサンプル取得後、ユーザーの「一旦観測はここまで」の指示でローカル観測プロセスを停止した（exit 1 / Ctrl-C）。追加の本番採取は実施せず、保存済みデータのみ集計した。常設のreadiness / monitor timerは変更していない。

- 手動観測開始15:26:45、連続採取15:28:48〜15:50:53、26サンプル。予定した30分を完走した記録ではない。
- cgroup memory.current: 17.48〜20.00 MiB。採取したmemory.peakの最大は24.33 MiB（コンテナcgroupの生存期間のpeakであり、この採取区間だけのpeakとは限らない）。
- 区間ログのkernel OOM / relay受信dropは0。全サンプルhealthy、restart count=0。
- 長時間巡回とreadiness不合格による公開索引の停止を1回確認。15:47に自動復帰し、最後に採取したreadiness / monitor結果は成功。
- 15:50に次の巡回が始まり、最後の区間ログにも接続・転送timeoutがある。この巡回の完了と、その後の継続可用性は未確認。
- 結論: OOM・再起動ループは観測中再発しなかったが、「問題なし」ではない。#1212未配備の既存取り込み経路で、接続待ちの累積による公開索引の一時停止が再現した。

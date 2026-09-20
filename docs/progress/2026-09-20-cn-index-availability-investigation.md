# api.kukuri.app 索引停止の調査

- 調査日: 2026-09-20 12:00 頃 JST
- 依頼範囲: 原因調査。production の再起動・設定変更・データ変更は行っていない。
- 対象: GCP project `kukuri-cn`、`asia-northeast1-a/kukuri-cn-vm`
- 稼働 cn-indexer revision: `4b7519460e8baf3c29d7ec0eadc6eb2c727a1065`
- ローカル HEAD: `b302f942decd25ae834ab9c295edee516fa96e02`

## 確認できた原因と影響

索引ワーカーの進捗停止により `indexer_ingest_fresh` が不合格になり、readiness activation が有効でなくなっている。公開 API で次を再現した。

```text
GET /healthz -> 200 {"status":"ok"}
GET /v1/node/manifest -> 200（community_index=true）
GET /v1/index/discovery?scope_kind=public_topic&scope_id=kukuri%3Atopic%3Ageneral
-> 404 {"code":"INDEX_QUERY_NOT_ACTIVATED","message":"this community node index activation is not current"}
```

manifest は構成上の対応機能を示しており、実際の activation の有効性を保証しない。索引 query gate は bearer 認証より前に activation を検査するため、未認証の read-only request で停止理由を確認できた（`crates/cn-user-api/src/handlers/indexing.rs`）。

## 時系列（JST）

- 9/18 15:48:05: 現在の cn-indexer container 起動。
- 9/18 20:49:42: 最後の全 scope 巡回成功（`last_sync_at=1789732182`）。
- 9/18 20:53:27: 最後の ingest 完了（`last_ingest_at=1789732407`）。この通知は索引に関係しない key で scanned=0。
- 9/18 20:54:42: 次の巡回で seed peer 更新完了（active=6 / applied=6）。
- 9/18 20:54:44: `general_ja` / `dev` / `test` / `general` の replica open 完了。その後の取り込み完了を確認できない。
- 9/18 21:02:27: readiness はまだ `ready=true fail=0 unknown=0`。
- 9/18 21:08:17: `indexer_ingest_fresh` が不合格。許容鮮度は900秒。
- 9/20 11:55:55: 同じ時刻値のまま `ready=false fail=1 unknown=0`。公開 API でも停止を再現。

## 調査時点の状態

- API / indexer / Postgres / ArcadeDB / Valkey は running / healthy。indexer は restart count=0、OOMKilled=false。
- indexer `/v1/status`: worker_running=true、ingest_enabled=true、opened_scopes=4、last_error=null。
- scanned=1805、indexed=1359、skipped_non_allow=125、scan_errors=0、provider_unavailable=0。前二者は累積処理数であり、現在の索引件数ではない。
- readiness の truth=36 / projection=36。件数整合の証拠であり、全投稿の索引完了やデータ損失がないことまでは保証しない。
- 両 provider の credential probe は合格。relation analysis も新鮮。
- readiness timer は次回実行予定あり。timer 停止による期限切れではない。
- `/var/lib/docker` は76%使用、空き6.2 GiB。観測時の Postgres に長時間 active query や lock wait は見られなかった。
- iroh sync/gossip の timeout / `MultipathNotNegotiated` が記録されている。ただしワーカー停止との因果関係は未確定。

## コードから分かる継続停止の条件

`crates/cn-indexer/src/worker.rs` は全 scope の処理を順次 await し、外側に巡回全体／scope の timeout がない。返らない await が一つあると次の巡回へ進めない。`worker_running` は起動時に true となり、進捗監視ではない。Docker healthcheck の `/healthz` もプロセスの HTTP 応答のみを確認する。このため、取り込みが止まっていても healthy のままであり、自動再起動は起きない。

ログと状態は巡回中の待機継続に整合するが、待機している具体的な呼出しや根本原因は未確定。処理段階別の開始・終了ログや task stack が不足しており、docs query、blob fetch、その他の待機を断定できない。ローカル HEAD と稼働 revision の indexer 差分も確認したが、最新コードへの更新だけで解消する根拠はない。

## 次の対応候補（未実施）

1. 応急復旧: cn-indexer のみを再起動し、last_sync_at / last_ingest_at と処理数の更新、次巡回の完了を確認する。その後 readiness service を実行し activation と公開 API の復帰を確認する。再起動で直ることは未検証。
2. 復旧確認: 認証・同意済み client から既存公開投稿の検索／発見を確認し、少なくとも次回巡回後まで進捗が継続することを確認する。
3. 恒久対策: 段階／scope ごとの進捗観測、停止 await の再現、キャンセル安全性を考慮した期限と再試行、進捗停止時の回復処理を検討する。readiness gate を無効化したり鮮度判定を緩めたりして復旧扱いにしない。

調査には gcloud/IAP 経由の read-only container 状態・journal・ログ・集計 SELECT、公開 HTTP GET、CodeGraph とソース照合を使用した。秘密値・投稿本文は本記録に含めない。実装変更はなく、build/test は実行していない。

## 追記: 応急復旧（ユーザー承認後）

- 2026-09-20 12:15:07 JST、`docker restart community-node-cn-indexer-1` を実行。API / relay / DB の再起動や構成変更は行っていない。
- 12:15:23 に worker 再開、4 scope を open。general_ja、dev、test の取り込みは12:16:29までに完了。
- general の巡回中、索引は36件から44件へ増加。12:41時点で、現在の44件のうち31件の indexed_at が再起動後に更新されていた。
- 本文の ephemeral fetch で複数peer／接続候補を順次試行し、候補ごとの15秒 timeoutが累積している。転送成功とDB更新も確認しており、再起動前の進捗停止とは区別する。
- 添付mediaの取得timeoutも2回、scan_errors=1を観測。該当mediaは非allowとして表出を抑止。readiness実装はscan_errors累積値そのものではなく、判定無し／非allow・critical表出／失敗からallowへの誤変換がないことを検査する。
- 初回全件巡回完了前のため、この時点では応急復旧は未完了。readiness gateや鮮度閾値は変更していない。

### API再開確認（13:16 JST）

- 13:15:56、初回全件巡回が完了。`last_sync_at=last_ingest_at=1789877756`、`last_pass_duration_ms=3633280`（約60.6分）。scanned=80、indexed=60、skipped_non_allow=14、deindexed=6。
- 13:16:38に `sudo systemctl start kukuri-readiness.service` を実行。13:16:40に `ready=true fail=0 unknown=0`、activation書き込み成功。
- truth=64 / projection=64。判定無し索引=0、非許可・重大の表出=0、失敗→許可=0。scan_errors=7 / media_fetch_timeout=14は残るため、全投稿・メディアの取得成功とは扱わない。
- 次回readiness timer予定は13:21:51 JST。
- 公開のsearch/discovery双方で `404 INDEX_QUERY_NOT_ACTIVATED` → `401 AUTH_REQUIRED` を確認。APIのactivation gate解除を確認した。`/healthz` は200。
- 13:16:57の集計でも既存entryのindexed_atは13:16:48へ更新され、初回巡回後のイベント処理による進捗がある。`event_whole_scope_fallbacks=1`、理由は `manifests/media`。

**残る制約:** 認証済みクライアントからの実結果表示、および次の定期全件巡回の完走は未確認。初回巡回が900秒の鮮度閾値を大きく超えたため、同様の長時間巡回が続くと再停止し得る。ここで確認できたのはAPIの一時的な再開であり、継続可用性・根本原因の解消ではない。再起動の反復やreadiness gateの迂回は行っていない。

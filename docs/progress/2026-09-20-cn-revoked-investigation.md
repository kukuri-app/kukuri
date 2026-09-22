# CN activation revoked の調査（2026-09-20）

- 調査対象: GCP kukuri-cn / asia-northeast1-a / kukuri-cn-vm、api.kukuri.app。
- 観測: 2026-09-20 17:30〜17:35 JST。調査のみ。本番設定変更、再起動、readiness手動実行は行っていない。

## 確認結果

- DB `cn_admin.readiness_activations` の17:17:16、17:23:16、17:29:16 JSTの記録はいずれも `revoked=true`, `reason=readiness_failed`。
- 17:29:17のreadinessは `ready=false fail=1 unknown=0`。唯一の不合格は `indexer_ingest_fresh`。
- 最終全scope同期完了は16:30:22 JST（1789889422）、最終scope取り込み完了は16:49:54 JST（1789890594）。両方が900秒以内という条件を満たさない。
- 16:41の判定は合格、16:47:16から17:29まで連続不合格。15:41と16:05にも同じ条件で一時的な不合格があり、その後回復していた。
- worker_running=true、ingest_enabled=true、opened_scopes=4、last_error=null。scanned=897、indexed=788。これらはscope完了時に更新されるため、処理中の実進捗を完全には表さない。
- provider認証、DB migration、投影到達、truth=91 / projection=91、relation analysisは合格。readiness timerには次回予定あり。
- 公開 `/healthz` は `status=ok`。API、indexer、Postgres、Valkey、ArcadeDBはhealthy、relayはrunning。
- indexerは15:13:12 JST起動、restart count=0、観測時RSS約22.27 MiB。Docker領域は76%使用、空き6.2 GiB。今回の観測は同日先行のOOM・再起動ループとは異なる。

## 原因

直接原因は取り込み鮮度判定の失敗によるactivationの自動失効。これによりindex / trust等のactivation gate対象が閉じる。healthz成功はこれらの利用可能性を保証しない。

本番indexer revisionは `4b7519460e8baf3c29d7ec0eadc6eb2c727a1065`、image digestは `sha256:066a220a6d2dd88087c28d395e5dcb0665cc51e3de7399bd4dcfd13bf2937160`。

16:35:22開始の全scope巡回はgeneral_jaが16:39:22、devが16:40:07、testが16:49:54に完了し、以後generalの完了記録がない。17:30以降もbounded scan blobの転送timeout（候補ごと15秒）が続く一方、docs syncの成功は観測できる。

当該revisionはpeer・到達候補・投稿・scopeを順次処理し、remote fetch全体の時間上限がない。全scopeの完了までlast_sync_atが進まず、遅いblob取得が全巡回を長期化する。今回のログは[同日午前の遅延調査](2026-09-20-cn-recovery-ingest-latency-investigation.md)と同じ構造に一致する。個々のpeerが転送を完了できない理由は、相手側の証跡がなく未確定。

## 次の対応

- mainには #1223（e08743fb、取り込み高速化）と #1230（d80d0011、取得の有限化）があるが、本番には未配備。
- 既存production rolloutに従い、対象image・digest・ローカル検証・backupを確認して配備するのが恒久対策候補。配備後は15分を超えて巡回とreadinessの継続成功を確認し、実クライアントの索引利用まで検証する必要がある。
- 閾値緩和やactivationの手動改変は原因の解消にならない。今回、復旧操作は実施していない。

証拠は本番readiness journal、DBの限定SELECT、Docker status/inspect/stats、indexer `/v1/status`・ログ、稼働revisionのソース比較。本文・peer ID・IP・blob hashは本記録に保存していない。

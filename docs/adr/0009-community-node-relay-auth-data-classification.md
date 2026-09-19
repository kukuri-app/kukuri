# ADR 0009: Community-Node Connectivity/Auth Data Classification

## Status
Accepted

## Feature Data Classification
- Feature 名: community-node connectivity/auth
- Durable / Transient: Durable server state + local durable desktop config
- Canonical Source: server `Postgres` + desktop local `community-node.json` + desktop secure token storage
- Replicated?: Server state is not client-replicated; desktop config is local only
- Rebuildable From: server `Postgres` migrations/seed data + desktop local config + secure token storage
- Public Replica / Private Replica / Local Only: public bootstrap metadata, private auth/consent state in `Postgres`, local desktop config/token storage
- Gossip Hint 必要有無: No
- Blob 必要有無: No
- SQLite projection 必要有無: Desktop local only; server does not use SQLite
- 必須 contract:
  - `community_node_auth_verify_rejects_capability_url_mismatch`
  - `community_node_bootstrap_requires_auth_when_rollout_is_required`
  - `community_node_auth_rollout_respects_existing_connection_grace`
  - `desktop_runtime_restores_community_node_config_and_tokens_after_restart`
  - `transport_custom_connectivity_mode_connects_when_community_node_connectivity_urls_are_configured`
- 必須 scenario:
  - `community_node_public_connectivity`

## Decision
- community-node server persistence は Phase6 から `Postgres` に固定する。
- current desktop canonical data plane は `docs + blobs + hints + DHT` のまま維持し、community-node は接続基盤と auth/control plane を担う。
- desktop は multi-node list を保持し、token は keyring/file fallback へ保存する。
- `connectivity_urls` の反映は startup-only とし、変更時は desktop restart を要求する。

## Consequences
- server 側の query/migration/testing は最初から `Postgres` 前提に揃える。
- migration/seed の標準入口は `cn-cli prepare` とし、`cn-user-api` は prepared DB を既定前提に fail-fast 起動する。
- `docs/blobs/gossip/SQLite` の既存責務を community-node 導入で変更してはならない。
- desktop の community-node 設定変更は `iroh` endpoint 再生成が必要な場合に限り `restart_required` を返す。

## セッション維持と接続復旧（#1176）

- Rust側schedulerがUI pollingと独立してnodeごとの登録・rendezvous・token更新を駆動する。観測送信とconnectivity self-healは別laneで、あるlaneの未完了を次tickや別nodeの実行条件にしない。同じlaneを重複実行せず、runtime停止時は実行中futureも破棄する。
- CNの通常HTTP clientは接続5秒・応答本文を含む全体10秒を上限とする。セッション更新の排他はnode別、relay/seedの全体反映は専用の排他区間で最新の検証済み状態から行う。未同意nodeを別nodeの成功で有効化しない。
- remote peer不在だけで正常なlocal docs actorを停止しない。通常のself-healはdiscovery/購読の再適用とする。local docsの読み取りprobeが失敗した場合はstackを再構築するが、probeのtimeoutだけでは破損と判定せず再試行へ戻す。
- 再構築が失敗しても前回stackの参照・peer stateを保持し、次回の再試行を可能にする。再構築のcommit後はseed値が同じでも旧streamを再利用せず、購読・private capabilityを新しいstackへ復元する。identityやdocs/blobsの初期化は復旧手段にしない。
- node生成前の失敗でも、開いたFsStoreのshutdown完了を待ってからエラーを返す。失敗したstoreの非同期Dropだけに次回openの安全性を依存させない。
- relay受信容量とblob取得失敗のUI/キャッシュはそれぞれ#1206・#1207の責務であり、本変更で通信優先度や取得gateを変更しない。

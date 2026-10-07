# ADR 0008: DHT Discovery Data Classification

## Status
Accepted

## Feature Data Classification
- Feature 名: seeded DHT discovery
- Durable / Transient: Transient + local durable config
- Canonical Source: local `discovery.json` config + runtime Mainline DHT records
- Replicated?: Runtime lookup result only
- Rebuildable From: local discovery config + live DHT records
- Public Replica / Private Replica / Local Only: Local only config, public DHT endpoint records
- Gossip Hint 必要有無: No
- Blob 必要有無: No
- SQLite projection 必要有無: No
- 必須 contract:
  - `transport_seeded_dht_can_connect_by_endpoint_id_without_ticket`
  - `seeded_dht_syncs_post_between_apps_without_ticket_import`
  - `restart_restores_seeded_dht_config_and_reconnects`
- 必須 scenario:
  - desktop 2 instance で reciprocal seed を設定し、`post -> reply/thread -> live/game -> restart -> reconnect without reimport` を確認する

## Decision
- DHT discovery v1 は `EndpointId -> EndpointAddr` 解決だけを扱い、topic rendezvous / relay / mDNS は含めない。
- `KUKURI_DISCOVERY_MODE` と `KUKURI_DISCOVERY_SEEDS` を最優先とし、未指定時のみ `db_path.with_extension("discovery.json")` を読む。
- `DiscoveryMode::SeededDht` では `iroh` の `DhtAddressLookup` を shared endpoint に mount し、seed peer は `EndpointAddr::new(id)` で保持する。
- manual `import_peer_ticket` は即時接続用の ephemeral hint として残し、discovery config には永続化しない。

## 改訂（2026-10-07、#1632）
- 本 ADR の DHT は `EndpointId -> EndpointAddr` の住所の解決を扱う。公開 blob の hash から保持端末を探す発見は
  [ADR 0063](0063-public-blob-dht-discovery.md) が扱い、住所の解決と同じ node の DHT を共有する
  （`DhtAddressLookup::builder().dht(..)`）。hash の発見を住所の解決の結果として扱わない。
- 設定「公開コンテンツの発見」がオンで補助 index の一覧があれば、`static_peer` や Community Node の利用中でも DHT を
  組み立てる。オフでは上の Decision のとおり。旧記述の「`SeededDht` でだけ `DhtAddressLookup` を mount する」は
  この条件に置き換わる。

## Consequences
- seed peer が 0 件のとき discovery は idle で、現状どおり local-first / static ticket fallback の振る舞いを維持する。
- peer address の正本は DHT にあり、`docs / blobs / gossip / SQLite` は endpoint address の canonical store にならない。
- env で discovery を与えた場合、desktop UI の seed editor は read-only でなければならない。

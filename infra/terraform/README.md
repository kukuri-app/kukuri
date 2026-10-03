# kukuri community node Terraform (GCP)

Issue #381 の Terraform 実装。GCP 上に community node（`cn-user-api` + `cn-iroh-relay` +
Postgres + Valkey）をデプロイする。詳細手順は
`docs/runbooks/community-node-gcp-terraform.md` を参照する。

## deployment profile

| profile | compute | Postgres | Valkey | blob/media | backup | 位置づけ |
|---|---|---|---|---|---|---|
| `low-cost` | 単一 Compute Engine VM | node-local container（control-plane data のみ） | node-local container（TTL ephemeral、backup 対象外） | local cache（既定無効） | pg_dump → GCS | 個人・小規模 operator の入口。managed DB 不要。 |
| `managed-db` | Compute Engine VM | Cloud SQL | Memorystore | local cache | Cloud SQL 自動 backup/PITR | default / 利用者が増えた public node。 |
| `ha` | Compute Engine VM | Cloud SQL REGIONAL | Memorystore STANDARD_HA | GCS object storage | Cloud SQL 自動 backup/PITR | 明示的な high-cost 構成。third-party 標準ではない。 |

全 profile で `cn-user-api` + `cn-iroh-relay` は Compute Engine 上に置く（Cloud Run は relay の
UDP/QUIC を扱えないため）。profile ごとに変わるのは data/cache/blob/backup 階層。

## データ境界（全 profile 共通）

- 小さく重要な永続データ（auth/consent, admission mode, invite/allowlist/ban, report metadata,
  operator config）は Postgres に置く。
- TTL 付き一時状態（topic rendezvous, presence, short-lived connection hints）は Valkey に置き、
  永続化・backup を前提にしない。
- blob/media 本体は **Postgres に置かない**。local cache / iroh blobs / object storage を使う。
  cache は size / TTL / eviction を変数で制御し、backup 対象外にできる。
- `community_index` / `moderation` / `community_local_trust` は Phase B（計画中・未提供）。
  初期 `low-cost` DB に同居させない。search index / trust graph は rebuildable data として扱い、
  canonical DB に混ぜない。

## deployment profile と cn-operator capability profile は別物

- `low-cost` / `managed-db` / `ha` = **インフラ**のコスト/データ階層の軸（この Terraform）。
- `minimal` / `relay-enabled` / `full-service` = **cn-operator** の開示・manifest 用 capability の軸。
- 両者は独立して選ぶ。operator-config.yaml は `cn-operator init` で別途用意する。

## ディレクトリ

```text
infra/terraform/
  modules/
    gcp-network/            VPC/subnet/static IP/firewall (80,443,7842/udp,3478/udp,IAP SSH)
    gcp-dns/                Cloud DNS A records (任意)
    gcp-vm-compose/         Compute Engine + startup script (compose/Caddy/ACME/backup) + Secret Manager accessor IAM
    gcp-low-cost-backup/    pg_dump 退避先 GCS bucket
    gcp-cloudsql-postgres/  Cloud SQL 拡張点
    gcp-managed-valkey/     Memorystore 拡張点
    gcp-blob-storage/       blob object storage 拡張点
  envs/
    low-cost/               apply 可能（初期スコープ）
    managed-db/             拡張点（validate 対応）
    ha/                     拡張点（validate 対応）
```

## 検証

```bash
terraform fmt -check -recursive infra/terraform

terraform -chdir=infra/terraform/envs/low-cost  init -backend=false && terraform -chdir=infra/terraform/envs/low-cost  validate
terraform -chdir=infra/terraform/envs/managed-db init -backend=false && terraform -chdir=infra/terraform/envs/managed-db validate
terraform -chdir=infra/terraform/envs/ha         init -backend=false && terraform -chdir=infra/terraform/envs/ha         validate
```

apply は `low-cost` のみが初期スコープ。`managed-db` / `ha` は拡張点で、apply 完成は後続。

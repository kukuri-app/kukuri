# Community Node GCP Terraform Deploy

最終更新日: 2026-09-17

`openai-moderation` の非秘密設定は `deploy.moderation`、キーは既存の `deploy.vlm_api_key_secret_id` から注入する。詳細は[専用providerの運用](community-node-openai-moderation.md)を参照。

## 目的

- community node（`cn-user-api` + `cn-iroh-relay` + Postgres + Valkey）を GCP に
  Terraform でデプロイする（Issue #381）。
- `deploy_indexer_stack=true` で index / moderation stack（`cn-indexer` + ArcadeDB +
  relation 定期解析 + moderation secrets）を同じ VM に追加する（Issue #615）。
- 本手順の配備対象は実装済みの `low-cost`。`managed-db` / `ha` は後述の未完成の拡張点であり、配備作業で完成させる対象にしない。
- third-party community node operator が低コストで始められる `low-cost` profile を標準入口にする。

実装は `infra/terraform/`。この runbook は実行手順、設計の根拠は同 `README.md` と
`docs/architecture/p2p-first-community-node-responsibility-boundary.md` を参照する。
既存production環境の日常rollout、意図しないVM置換の回避、容量枯渇からの復旧、実投稿の
live確認は `docs/runbooks/community-node-production-rollout.md` を先に参照する。

特定の公開topicの索引を提供するlow-cost nodeは `index_expected_topics` に期待集合を明示できる。
空が既定で、一般nodeへ既定topicを強制しない。この設定は監視専用で、管理画面でのtopic登録を
代替しない。欠落・空索引警報、本文取得失敗metric、実投稿による検索確認は
[production rolloutの索引可用性手順](community-node-production-rollout.md#522-default-onboarding-node-の索引可用性)に従う。

> 注意: この runbook は法的助言ではない。日本国内で relay を運用する場合の電気通信事業の
> 届出要否や記載内容は、最終的に operator 自身と総合通信局・専門家への確認が必要。region 既定の
> `asia-northeast1`（東京）も法的保証ではなく、単なる既定値。

実行前に対象project・VM・deployment profile・有効capability・image digestと、今回の配備または復旧の終了条件を固定する。任意のindex stackや別profileを一律に有効化・検証しない。

## アーキテクチャ

- 全 profile で `cn-user-api` + `cn-iroh-relay` は **Compute Engine VM** 上で動かす。
  Cloud Run は relay の UDP/QUIC（`7842/udp`）を扱えないため。
- profile ごとに変わるのは data/cache/blob/backup 階層。
- `low-cost` は単一 VM 上で既存 community node スタックを GHCR image から動かし、
  Caddy が API/relay-HTTP の HTTPS を終端、QUIC は VM の `7842/udp` で直接公開する。

GHCR の repository owner は software image の配布元を示すだけで、配備された Node の運営主体を
示さない。Node の operator、domain、連絡先、監査 actor、公開文書は各 operator の
`operator-config.yaml` / tfvars だけから決める。`region`、`machine_type`、deployment profile の
初期値も KingYoSun 運営 Node の特権的な標準ではなく、operator が要件と法域に合わせて変更する
配備用の開始値である。

```text
client ──https://api_domain      ─▶ Caddy(:443) ─▶ cn-user-api(:8080)
client ──https://relay_domain    ─▶ Caddy(:443) ─▶ cn-iroh-relay(:3340)
client ──relay_domain :7842/udp  ─────────────────▶ cn-iroh-relay QUIC(:7842)
VM 内: cn-postgres / cn-valkey は private（公開しない）
deploy_indexer_stack=true 時（#615）:
VM 内: cn-arcadedb(:2480) / cn-indexer(:8630) も private（compose network 内のみ。
       firewall / Caddy へ新規公開 port は追加しない）
VM 外: cn-indexer ─outbound HTTPS─▶ Project Arachnid Shield
       cn-indexer ─承認済み境界（private tunnel または public TLS + API key + source allowlist）─▶ self-host VLM endpoint
```

### TLS / 証明書

- Caddy 内部 auto-TLS は使わない。`cn-iroh-relay` の QUIC が PEM ファイルを直接読むため、
  ACME companion（certbot）が `api_domain` / `relay_domain` の PEM を `/var/lib/kukuri/certs` に
  発行・更新し、Caddy と relay が共有する。
- 初回は startup script が `certbot --standalone` で発行（Caddy が :80 を bind する前）。
- 更新は systemd timer（daily）が webroot mode で renew し、更新時のみ Caddy reload +
  relay restart で新 PEM を反映する。

## データ境界

| データ | 置き場所 | backup |
|---|---|---|
| auth/consent, admission mode, invite/allowlist/ban, report metadata, operator config | Postgres（control-plane data） | low-cost: pg_dump→GCS / managed-db,ha: Cloud SQL |
| topic rendezvous, presence, short-lived connection hints | Valkey（TTL ephemeral） | 対象外 |
| blob/media 本体 | local cache / iroh blobs / object storage（**Postgres に置かない**。恒久保存しない） | 対象外。必要時の再取得は提供元と保持状態に依存 |
| index 真実源（supported set / indexing request / channel secret）、moderation verdict / artifact / risk signal | Postgres（#615。既存 backup にそのまま含まれる） | low-cost: pg_dump→GCS |
| index 投影 + relation graph | ArcadeDB（**rebuildable projection。canonical store ではない**） | 対象外（空からの再構築手順を後述） |
| cn-indexer の iroh state（endpoint 同一性 / docs replica / blob store） | `indexer_data_disk_gb > 0` で専用 PD | 対象外。PDでVM置換を越えて保持する。endpoint秘密鍵や欠損した全履歴をネットワークから復元できるとは扱わない |

## 人手で先に用意するもの

GCP / GitHub の以下は Terraform の前に手動で用意する。`gcloud` 例:

```bash
# 1) project / billing / API
gcloud config set project YOUR_PROJECT
gcloud services enable \
  compute.googleapis.com iam.googleapis.com iamcredentials.googleapis.com \
  secretmanager.googleapis.com storage.googleapis.com \
  dns.googleapis.com serviceusage.googleapis.com cloudresourcemanager.googleapis.com

# 2) Terraform state 用 GCS backend bucket
gsutil mb -l asia-northeast1 gs://YOUR_TF_STATE_BUCKET
gsutil versioning set on gs://YOUR_TF_STATE_BUCKET

# 3) Secret Manager に secret を作成（payload は Terraform に渡さない）
#    JWT secret は 32 byte 以上、placeholder 文字列(change-me 等)を含めない。
printf '%s' "$(openssl rand -hex 32)" | \
  gcloud secrets create kukuri-cn-jwt-secret --data-file=-
printf '%s' "$(openssl rand -hex 24)" | \
  gcloud secrets create kukuri-cn-postgres-password --data-file=-

# 3b) index / moderation stack（#615、deploy_indexer_stack=true のとき）の runtime secrets。
#     値は tfvars / metadata / repo に書かず、Secret Manager だけに置く。
# channel secret key（32 byte 以上、placeholder 不可。cn-user-api と cn-indexer が共有）
printf '%s' "$(openssl rand -hex 32)" | \
  gcloud secrets create kukuri-cn-channel-secret-key --data-file=-
# ArcadeDB root password（8 文字以上）
printf '%s' "$(openssl rand -hex 24)" | \
  gcloud secrets create kukuri-cn-arcadedb-password --data-file=-
# moderation event signing key（secp256k1 秘密鍵 hex。issuer node の鍵として公開鍵が event に載る）
printf '%s' "$(openssl rand -hex 32)" | \
  gcloud secrets create kukuri-cn-safety-signing-key --data-file=-
# 署名鍵の公開鍵 hex(= risk signal の issuer_node_id)を導出し、operator-config.yaml の
# server.node_id に記入する(#706)。秘密鍵は同一シェルの環境変数に留め、標準出力には公開鍵だけが出る。
# 異議申し立ては公開ノード情報の node_id と issuer_node_id が一致する場合だけ受理され、
# 一致しない構成は validate-config と cn-user-api の起動時検査で拒否される。
COMMUNITY_NODE_SAFETY_SIGNING_KEY="$(gcloud secrets versions access latest \
  --secret=kukuri-cn-safety-signing-key --project=YOUR_PROJECT)" \
  cargo run -q -p kukuri-cn-cli -- moderation issuer-node-id
# Project Arachnid Shield credentials（operator 自身が https://projectarachnid.com で取得）
printf '%s' 'YOUR_ARACHNID_USERNAME' | \
  gcloud secrets create kukuri-cn-arachnid-username --data-file=-
printf '%s' 'YOUR_ARACHNID_PASSWORD' | \
  gcloud secrets create kukuri-cn-arachnid-password --data-file=-
# 任意: VLM API key（self-host の無認証 endpoint なら作成不要）
# printf '%s' 'YOUR_VLM_API_KEY' | gcloud secrets create kukuri-cn-vlm-api-key --data-file=-

# PowerShell で secret file を作る場合は BOM/改行が混入しないよう、ASCII + NoNewline にする。
# 例: Set-Content -Encoding ascii -NoNewline secret.txt <hex-string>
# BOM/改行が混じると Postgres DSN / JWT secret として読めず、起動時 migration が失敗する。

# 4) GHCR の public image を用意（cn-user-api / cn-iroh-relay / cn-cli / cn-indexer）
#    public image 前提なので VM 側の認証は不要。
```

GHCR image は `.github/workflows/kukuri-cn-images.yml` が `docker/cn/Dockerfile` を使って 4 binary 分 build/publish する。

| binary | package | GHCR image |
|---|---|---|
| `cn-user-api` | `kukuri-cn-user-api` | `ghcr.io/kukuri-app/kukuri-cn-user-api:<tag>` |
| `cn-iroh-relay` | `kukuri-cn-iroh-relay` | `ghcr.io/kukuri-app/kukuri-cn-iroh-relay:<tag>` |
| `cn-cli` | `kukuri-cn-cli` | `ghcr.io/kukuri-app/kukuri-cn-cli:<tag>` |
| `cn-indexer` | `kukuri-cn-indexer` | `ghcr.io/kukuri-app/kukuri-cn-indexer:<tag>` |

`cn-indexer` image（#614）は production feature 構成（Project Arachnid Shield + OpenAI-compatible VLM。mock は選択不能）で build され、workflow が publish 前に `validate-config` smoke（provider 解決 / slot 制約 / credential env 欠落 / credential 非漏出）を通す。credential / endpoint / 署名鍵は image に含まれないため、デプロイ時に runtime secret（env）として注入する。手元でも次で構成だけを検証できる:

```bash
docker run --rm -e COMMUNITY_NODE_DATABASE_URL=... -e COMMUNITY_NODE_CHANNEL_SECRET_KEY=... \
  -e COMMUNITY_NODE_INDEXER_EXTERNAL_RELAY_URLS=... \
  ghcr.io/kukuri-app/kukuri-cn-indexer:<tag> validate-config
```

`cn-indexer` は low-cost Terraform stack へ `deploy_indexer_stack=true`（#615）で追加する。手順は後述の「index / moderation stack のデプロイ」を参照。

初回 publish は workflow を default branch（通常 `main`）へ merge した後に手動 dispatch する。workflow file がまだ default branch に無い状態では `workflow_dispatch` が見つからないため、PR 中は build-only 検証に留める:

```bash
gh workflow run kukuri-cn-images.yml -f image_tag=latest -f push=true
```

既に workflow が default branch に存在する場合は、必要に応じて `--ref <branch-or-tag>` を付けて特定 ref の image を publish する。

workflow は PR では build のみ、`main` push / `develop` push / `v*` tag push / manual dispatch では GHCR に push する。
`main` push は `latest` と `sha-<12桁>`、`develop` push は `develop` と `sha-<12桁>`、tag push は tag 名と `sha-<12桁>` を publish する。

初回 publish 後、GitHub Packages の各 package visibility が private の場合は、Terraform で使う前に GitHub UI で public に変更する（VM は GHCR 認証なしで pull する前提）。
`terraform.tfvars` には、例えば以下を指定する:

```hcl
cn_user_api_image   = "ghcr.io/kukuri-app/kukuri-cn-user-api:latest"
cn_iroh_relay_image = "ghcr.io/kukuri-app/kukuri-cn-iroh-relay:latest"
cn_cli_image        = "ghcr.io/kukuri-app/kukuri-cn-cli:latest"
cn_indexer_image    = "ghcr.io/kukuri-app/kukuri-cn-indexer:latest"
```

本番 apply では `latest` より digest 固定（例: `ghcr.io/kukuri-app/kukuri-cn-user-api@sha256:...`）を推奨する。
全 kukuri image の digest は次で確認できる（`cn_*_image` 変数は `@sha256:` 参照をそのまま受け付ける）:

```bash
docker buildx imagetools inspect ghcr.io/kukuri-app/kukuri-cn-indexer:latest --format '{{println .Manifest.Digest}}'
```

digestはworkflow logの先頭の `sha256:` から転記せず、workflow成功後のregistry manifestから取得する。
`cn-indexer` jobはpublish対象とは別のpositive smoke imageを先にbuildすることがあり、そのdigestは
GHCRに存在しない。取得した参照はapply前に必ず解決確認する:

```bash
docker manifest inspect \
  ghcr.io/kukuri-app/kukuri-cn-indexer@sha256:<registryで確認したdigest> >/dev/null
```

## low-cost deploy

既存production nodeを更新する場合は、この節を直接実行する前に
`docs/runbooks/community-node-production-rollout.md` のbackup、replacement gate、Docker空き容量、
post-deploy verificationを通す。

```bash
cd infra/terraform/envs/low-cost
cp terraform.tfvars.example terraform.tfvars   # 値を埋める（secret VALUE は書かない）
cp backend.hcl.example backend.hcl             # state bucket/prefix を埋める

terraform init -backend-config=backend.hcl
terraform plan
terraform apply
```

- `manage_cloud_dns=false`（既定）の場合は `terraform output static_ip` の IP に対して
  `api_domain` / `relay_domain` の A レコードを手動で設定する。
- `manage_cloud_dns=true` の場合は既存 Cloud DNS zone（`dns_zone_name`）に A レコードを作成する。
- 単独 operator が GCS backend を使わず始める場合は、`backend.tf` の `backend "gcs" {}` を
  コメントアウトして `terraform init`（local backend）でもよい。

### 確認

```bash
curl -fsS https://<api_domain>/healthz
curl -fsS https://<relay_domain>/ping
terraform output ssh_iap_command   # IAP 経由 SSH
terraform output admin_iap_tunnel_command # IAP 内部 admin UI
```

VM 内のサービスは `/var/lib/kukuri/community-node` の docker compose で動く。SSH は IAP のみ
（`22/tcp` と admin UI の `9090/tcp` は GCP IAP レンジからのみ許可）。admin UI を使う場合は、
別 terminal で output の tunnel command を実行し、`http://localhost:9090` を開く。到達には
`iap.tunnelInstances.accessViaIAP` を含む IAM role が必要で、Caddy / public DNS には載せない。

admin UI は状態、最新 readiness activation、admission mode、supported topics、直近 50 件の通報、
Cloud Logging 導線を表示する。browser write を有効にする operator は、自身の監査 identity を
`admin_actor` へ明示する。汎用既定値は空であり、空なら write endpoint は fail-closed して
read-only になる。`safety_operator_review` も運用方針に合わせて明示し、append-only audit、
差分 preview、CSRF 防御の内側で browser write を扱う。
credentialや配備宣言は引き続き `operator-config.yaml` / Terraform / Secret Manager / `cn-cli` を
SSoTとし、admin UIから変更しない。

### admission / 運用

入会制御（招待 / whitelist / ban）は `cn-cli admission` を VM 上の compose で実行する。
`docs/runbooks/community-node-self-host-vps.md` の admission 節と同じ操作。

```bash
# VM へ IAP SSH 後
cd /var/lib/kukuri/community-node
COMPOSE=/var/lib/toolbox/kukuri/bin/docker-compose
sudo "$COMPOSE" run --rm cn-migrate   # 既に起動時に実行済み（再実行は冪等）

# admission用に、registryで検証したcn-cli digestを直接使う例。
# secretを含むproject .envをcommand出力へ展開しない。
CLI_IMAGE=ghcr.io/<owner>/kukuri-cn-cli@sha256:<verified-digest>
sudo docker run --rm --network community-node_default --env-file .env \
  "$CLI_IMAGE" admission show
```

Container-Optimized OSでは `docker compose` pluginが見つからない場合がある。Terraform構成では
startup scriptが配置する `/var/lib/toolbox/kukuri/bin/docker-compose` を常に使う。

## index / moderation stack のデプロイ（#615）

`deploy_indexer_stack=true` で `cn-indexer` + ArcadeDB + relation 定期解析を同じ VM の compose に
追加する。既定は false（従来の API / relay のみ構成）。

### 前提

- 「人手で先に用意するもの」の 3b) の runtime secrets（channel key / ArcadeDB password /
  signing key / Arachnid credentials / 任意 VLM key）を Secret Manager に作成済みであること。
  secret の accessor 権限は Terraform が VM service account へ自動付与する。
- `machine_type` は既定 `e2-medium`（ArcadeDB(JVM) + cn-indexer の同居前提）。`e2-small` では
  memory が不足する可能性が高い。既存 VM を e2-small から上げる場合、apply が VM を
  stop/start する点に注意。
- 本番では `indexer_data_disk_gb > 0`（専用 PD）を推奨。cn-indexer の iroh endpoint 同一性と
  docs replica が VM 置換に耐える。

### 有効化（operator-config 経由を推奨）

operator-config.yaml の `deploy:` に次を追加し、`generate-tfvars` で tfvars を再生成する
（値は例。secret は **ID のみ**）:

```yaml
safety:
  events:
    emit_signed_moderation_events: true
    signing_key_secret_id: kukuri-cn-safety-signing-key
  providers:
    known_csam:
      provider: project-arachnid-shield
      required: true
    general:
      provider: openai-compatible-vlm
    unknown_csam:
      provider: openai-compatible-vlm
deploy:
  # ...既存の設定...
  deploy_indexer_stack: true
  cn_indexer_image: ghcr.io/kukuri-app/kukuri-cn-indexer:latest   # 本番は digest 固定
  indexer_data_disk_gb: 10
  relation_analyze_interval_minutes: 60   # 1〜90（readiness の関係解析 7200 秒以内を保つ上限。#1101）
  indexer_retention_days: 30              # 必須。再取得可能な保存物の保持期間 T（3 以上。#1221 R5-F）
  indexer_capacity_rows: 1000000          # 必須。容量 B（索引・撤回 marker・関係・verdict・scan cache の行数）
  channel_secret_key_secret_id: kukuri-cn-channel-secret-key
  arcadedb_password_secret_id: kukuri-cn-arcadedb-password
  arachnid_username_secret_id: kukuri-cn-arachnid-username
  arachnid_password_secret_id: kukuri-cn-arachnid-password
  vlm_api_base_url: https://vlm.example.net   # public TLS + source allowlist の例
  vlm_model: inclusionAI/SingGuard-2b
  vlm_response_format: guard
  vlm_api_key_secret_id: kukuri-cn-vlm-api-key
```

`validate-config` が runtime の起動 gate（必須 secret ID / relay / provider credential /
署名鍵）を apply 前に fail-closed で検査する。tfvars を手書きする場合も同名の変数を設定する。

### rollout 順序

apply 1 回で startup script が次の順序を machine 的に守る（compose の depends_on / healthcheck）:

1. `cn-migrate`（migration。冪等）
2. Postgres / ArcadeDB ready（ArcadeDB は空 database `kukuri_index` を初回起動時に冪等作成）
3. `cn-indexer` 起動（ArcadeDB schema を `ensure_schema` で冪等作成。provider / 署名鍵の構成不備は
   fail-closed で起動失敗）
4. relation analyze timer 有効化

ユーザー向け read surface はこの後も **既定 false** のまま:
`index_query_enabled` / `trust_read_enabled` は full-stack E2E（後続 issue）完了までは
有効化しない。無効の間、`GET /v1/index/*` / `/v1/trust/*` / `/v1/relation/*` は 404。

### 確認

```bash
terraform output ssh_iap_command   # IAP SSH
# VM 上で:
cd /var/lib/kukuri/community-node
docker ps                                        # cn-arcadedb / cn-indexer が healthy
docker exec $(docker ps -qf name=cn-indexer) curl -fsS http://127.0.0.1:8630/healthz
docker exec $(docker ps -qf name=cn-indexer) curl -fsS http://127.0.0.1:8630/v1/status  # last ingest / backoff 等
systemctl list-timers kukuri-relation-analyze.timer --all   # NEXT が時刻であること
systemctl status kukuri-relation-analyze.service # relation analyze の last success / failure
journalctl -u kukuri-relation-analyze.service -n 50
```

startup は relation analyze の service / timer を再生成する。boot から 15 分以上経ってからの再実行では、
再生成した timer が起動直後に 1 回実行し、以後は前回実行から `relation_analyze_interval_minutes` ごとに続く（#1099）。`NEXT` が `-` の場合や
手動で解析する場合は `sudo systemctl start kukuri-relation-analyze.service` を使う
（`docker-compose run` だけでは timer の記録が残らない）。

外部からの port scan で 80/443/`relay_quic_port`/udp 以外（2480 / 8630 / 5432 / 6379）が
閉じていることも確認する（firewall には新規 ingress を追加していない）。

### readiness（読み取り面の解禁判定、#616）

読み取り面（索引 query / 信頼 read）は、環境変数（`COMMUNITY_NODE_INDEX_QUERY_ENABLED` /
`COMMUNITY_NODE_TRUST_READ_ENABLED`）が真でも、`cn-cli readiness` の全項目合格記録
（`cn_admin.readiness_activations`）が無ければ公開されない（有効化の関門）。
全項目合格すると記録は自動で書かれる。user-api は各read時に最新activationを再検査するため、
記録の更新・失効に再起動は不要。

GCP構成では `kukuri-readiness.timer` が5分ごとに同じ判定を実行する（activationの既定有効期限は
15分）。`systemctl status kukuri-readiness.timer` / `systemctl status kukuri-readiness.service` /
`journalctl -u kukuri-readiness.service -n 100` で、最終成功・失敗と次回実行を確認する。判定失敗時は
CLIが旧activationをrevokeし、read-time gateがindex / trust surfaceを閉じる。timerは次回再試行する。

手動で判定する場合も service 経由で実行し、timer の記録と次回実行を残す:

```bash
# VM 上で:
sudo systemctl start kukuri-readiness.service
sudo journalctl -u kukuri-readiness.service -n 100 --no-pager
sudo systemctl list-timers kukuri-readiness.timer --all   # NEXT が時刻であること
# 全項目合格 → 有効化記録が書かれ、次のreadから反映される。
```

- startup は readiness の service / timer を再生成する。再生成後の初回は timer 起動の 2 分後に予定され、
  以後は前回実行から 5 分ごとに続く（#1097）。`NEXT` が `-` の場合は上記の service 起動で再開する。
- 疎通確認（Arachnid への合成ハッシュ送信 / VLM への無害な 1 リクエスト）の結果は
  15 分 TTL で保存される。強制再実行（`--force-probe`）は service に引数を渡せないため、
  `cn-readiness` を直接実行した後に上記の service 起動を続ける
  （`docker-compose run` だけで終えない。手順は `community-node-production-rollout.md` §5.2）:

  ```bash
  cd /var/lib/kukuri/community-node
  sudo /var/lib/toolbox/kukuri/bin/docker-compose run --rm cn-readiness \
    readiness --config /etc/kukuri/operator-config.yaml --force-probe
  sudo systemctl start kukuri-readiness.service
  ```
- `relation_analysis_recent` が不合格の場合は relation analyze を 1 回実行する
  （`sudo systemctl start kukuri-relation-analyze.service`。既定の許容は 7200 秒以内の成功記録）。
  `systemctl list-timers kukuri-relation-analyze.timer` の `NEXT` も確認する。
- 判定項目集合が変わる更新を入れた場合、古い記録は無効になり面は自動で閉じる
  （readiness の再実行で再解禁する。user-apiはread時に最新activationを読むため再起動不要）。

### VLM のネットワーク境界（self-host endpoint）

self-host VLM は次のどちらか一方の境界に固定し、runbook・network diagram・実配備を一致させる。

1. **private tunnel**: GCP VM から WireGuard / VPN 等で到達し、`vlm_api_base_url` に tunnel 先の
   private アドレスを指定する。WireGuard peer の設定は
   `docs/runbooks/community-node-self-host-vps.md` を参照する。
2. **public TLS + source allowlist**: HTTPS endpoint、API key、GCP VM の static egress IP の
   source allowlist をすべて必須とする。`vlm_api_key_secret_id` から鍵を注入し、VLM 側では
   allowlist 外と無認証のリクエストを拒否する。現行 `vlm.kukuri.app` はこの境界を採用する。

平文HTTPのpublic公開、API keyだけでsource restrictionが無い構成、検証を無効化したTLSは不可。
- VLM 不達時は cn-indexer が fail-closed（scan 失敗 = hold。allow に落ちない）。
- Project Arachnid Shield への通信は outbound HTTPS のみで、callback / inbound port は無い。

### Cloud Monitoring / log retention

- VM の `kukuri-monitor.timer` が5分ごとに disk使用率、Postgres/ArcadeDB/indexer health、
  last ingest age、backoff、外部 safety provider failure、media fetch unavailable累計、
  relation最終成功時刻を custom metrics へ送る。ピア不在・未複製によるmedia取得不能は
  `media_fetch_unavailable_total` で観測するが、外部provider障害のpaging対象には含めない。
- `relation_last_success_age_seconds` は関係解析の最終成功からの経過秒数。解析の失敗は、次の成功まで
  `1000000000` を送る。systemd の実行記録が無い期間（解析の実行中、boot 後、startup による unit 再生成後、
  timer の停止後）は、`/var/lib/kukuri/community-node/.monitor-relation-analyze` に残した最後の成功、
  boot 後の初回予定（16 分後）、有効な timer の起動のうち、最も新しい時刻から数える。そのため、初回の
  解析前には警報を出さず、timer が止まった場合は最後の成功から閾値（間隔 × 3）を超えた時点で警報を出す
  （#1102）。
- Terraform は各 custom metric descriptor と alert policy を作成する。通知を実配送するには、
  `monitoring_notification_channels` に既存 channel の resource name を設定して apply する。
- 確認: `systemctl status kukuri-monitor.timer`、`systemctl start kukuri-monitor.service`、
  `journalctl -u kukuri-monitor.service -n 50`。
- compose 全serviceは `json-file` の `max-size=20m` / `max-file=5` を共通適用する。
  `docker inspect <container> --format '{{json .HostConfig.LogConfig}}'` で実値を確認する。

### ArcadeDB を空から再構築する

ArcadeDBはprojectionであり、正本はPostgresと取得できるdocsにある。対象scope・objectと必要な表示・検索結果を先に固定し、全履歴が揃うことを復旧の成功条件にしない。

以下のvolume初期化は、projection全体の破損等で再作成を明示的に選んだ場合だけの操作である。通常の欠損・検索不調に適用せず、削除対象volumeと正本の保持を確認する。

```bash
cd /var/lib/kukuri/community-node
/var/lib/toolbox/kukuri/bin/docker-compose stop cn-indexer cn-arcadedb
docker volume rm community-node_cn-arcadedb-data   # volume 名は docker volume ls で確認
/var/lib/toolbox/kukuri/bin/docker-compose up -d cn-arcadedb
/var/lib/toolbox/kukuri/bin/docker-compose up -d cn-indexer
# cn-indexerはschemaを作成する。復旧結果は事前に固定した対象で確認する。
```

現行indexerには起動時・周期的な全件見直しが残る。これは設計原則上の未解消点であり、全件再同期を正当化したり、文書変更だけで有界な復旧を実装済みとしたりしない。対象を絞った復旧が現行入口でできない場合は未完了として記録し、完了範囲を再定義する。

### rollback（API / relay のみ構成へ戻す）

`deploy_indexer_stack=false` にして apply する（operator-config 経由なら
`deploy: deploy_indexer_stack: false` → `generate-tfvars` → apply）。

- cn-indexer / ArcadeDB / relation timer が構成から外れ、公開 surface は従来どおり
  API / relay のみになる（flag が既定 false のため index / trust API はもともと 404）。
- startupは `docker-compose up -d --remove-orphans` とoptional systemd unitのcleanupを行うため、
  旧構成のcn-indexer / ArcadeDB containerやrelation / readiness timerは再起動後に残らない。
- Postgres の永続 data（verdict / moderation artifact / index 真実源）は残る。
- `indexer_data_disk_gb > 0` の PD は Terraform 管理のため、変数を 0 にしない限り残る
  （再有効化時に endpoint 同一性を保てる）。

## backup / restore

- low-cost の backup は systemd timer（`kukuri-backup.timer`）が `pg_dump -Fc` を取り、
  GCS backup bucket（`terraform output backup_bucket`）へアップロードする。
  database 全体 dump のため、#615 で増えた永続 table（moderation verdict / artifact /
  risk signal / index 真実源）と暗号化済み案件データ／legal hold も追加設定なしで含まれる。
- Valkey と blob cache は backup 対象外。ArcadeDB も backup 対象外
  （再作成時も全件復元は保証しない）。raw blob / media は恒久保存しない。

restore 例（VM 上）:

```bash
cd /var/lib/kukuri/community-node
COMPOSE=/var/lib/toolbox/kukuri/bin/docker-compose
# 取得した dump を cn-postgres に流し込む（事前に cn-user-api 停止推奨）
sudo "$COMPOSE" stop cn-user-api
cat cn-postgres.dump | sudo "$COMPOSE" exec -T cn-postgres \
  sh -lc 'pg_restore --clean --if-exists --no-owner -U "$POSTGRES_USER" -d "$POSTGRES_DB"'
# 起動時にoperator-configの保持期限を再計算し、期限切れ（非hold）をlisten前に削除する。
sudo "$COMPOSE" start cn-user-api
```

restore drill は本番DBへ直接上書きせず、隔離した一時DBへ復元して次を確認する。

1. 最新dumpのGCS object generationとSHA-256を記録する。
2. 一時Postgresを起動し `pg_restore --exit-on-error --no-owner` で復元する。
3. `cn-cli prepare` の後、`COMMUNITY_NODE_OPERATOR_CONFIG` を復元対象と同じ設定へ向けて `cn-cli retention sweep` を実行する。
4. 期限切れ非hold行が削除され、有効行と期限切れhold行が保持されること、通常APIが期限切れhold行を返さないことを照合する。
5. 一時DBを削除し、実施日時・dump generation・検証結果を運用記録へ残す。

## secret rotation

- JWT / provider API key / Arachnid credential / safety signing keyは、Secret Managerへ新versionを追加し、
  VMを再起動してstartup scriptに `.env` を再生成させる。再起動後に旧versionを無効化する。
- Postgres passwordは、backup取得 → DB role password変更 → Secret Manager新version追加 → VM再起動を
  連続したmaintenance windowで行い、`cn-migrate`・API health・readinessを確認後に旧versionを無効化する。
- `COMMUNITY_NODE_CHANNEL_SECRET_KEY` は保存済みchannel secretの復号鍵なので、単純な値差替えは禁止。
  再暗号化migrationとrollback可能な二重読取期間を用意した専用変更として扱う。
- `COMMUNITY_NODE_LEGAL_DATA_KEY` も案件の機微区分の復号鍵であり、単純な値差替えは禁止。
  鍵を失うと受付・管理読取・復元がfail-closedになるため、Secret Managerのversion保護と再暗号化手順を先に用意する。
- ArcadeDB passwordはprojection停止・再構築可能性を前提に、password変更とcompose secret更新を同じ
  maintenance windowで行う。失敗時はArcadeDBを空から再構築する。
- rotation後は `docker compose config` やjournalへ値を出力しない。secret名だけを記録し、
  `cn-operator check-disclosures` とruntime/boot logの旧値・新値非含有検査を行う。

## managed-db / ha（拡張点）

`managed-db` は Cloud SQL + Memorystore、`ha` は HA DB/cache + object storage を使う。
初期実装では root は `terraform validate` まで対応する拡張点で、apply 完成は後続。

```bash
terraform -chdir=infra/terraform/envs/managed-db init -backend=false
terraform -chdir=infra/terraform/envs/managed-db validate
terraform -chdir=infra/terraform/envs/ha init -backend=false
terraform -chdir=infra/terraform/envs/ha validate
```

注意点（apply 時）:

- Cloud SQL は private services access（VPC peering）経由の private IP で VM から接続する。
  network module が `enable_private_services_access=true` で peering を作成する。初回 apply は
  `servicenetworking.googleapis.com` の有効化が必要。
- DB password は 2 か所で扱う: Cloud SQL user 作成用に `TF_VAR_database_password`（state には
  sensitive 保持、VM metadata には焼かない）、VM が boot 時に取得する用に同じ値を Secret Manager
  へ登録し `database_password_secret_id` を指定する。VM は起動時にこの secret から password を
  取得し、DSN へ URL-encode して組み立てる。
- VM への DB password 注入は startup script の metadata に平文を残さない（low-cost の JWT/PG
  secret と同じ Secret Manager fetch 方式）。

## local Postgres のデータ永続化（low-cost）

- `postgres_data_disk_gb=0`（既定）では Postgres data は boot disk 上の docker volume に置く。
  startup script の変更などで VM が置換されると boot disk ごとデータが消える可能性がある。
- 本番運用や long-lived node では `postgres_data_disk_gb` を 1 以上にして専用 persistent disk
  （`prevent_destroy`、VM 置換でも残る）に Postgres data を置くことを推奨する。
- 専用 disk 利用時は ext4 の `lost+found` で `initdb` が失敗しないよう、compose が `PGDATA` を
  mount 直下のサブディレクトリ（`/var/lib/postgresql/data/pgdata`）に設定する。docker volume
  利用時（既定）は `lost+found` が無いため従来どおり mount 直下を `PGDATA` とする。
- backup（pg_dump→GCS）は別途有効。disk 永続化と backup は併用する。
- `enable_disk_snapshots=true` の場合、boot disk と `postgres_data_disk_gb>0` で作成した Postgres data disk の両方に snapshot schedule を attach する。

## CI（GitHub Actions）

`.github/workflows/kukuri-terraform.yml`:

- `fmt-validate`: `terraform fmt -check -recursive` + 全 env root の `init -backend=false` +
  `validate`。credentials 不要。
- `low-cost-plan`: Workload Identity Federation で GCP に認証し、`low-cost` の `terraform plan`。
  `vars.GCP_WORKLOAD_IDENTITY_PROVIDER` が未設定、または fork PR の場合は skip する（PR 側の Terraform/workflow へ GCP OIDC を渡さない）。

### CI 用 GCP / GitHub セットアップ（人手）

リポジトリ移転時はproviderの `attribute-condition` とCI service accountの
`roles/iam.workloadIdentityUser` の `attribute.repository/<owner>/<repo>` を同じ現repositoryへ更新する。
GitHub APIのrepository IDで同一repositoryの移転を確認し、旧名の許可は残さない。

```bash
# Workload Identity Federation（GitHub OIDC）
gcloud iam workload-identity-pools create github --location=global
gcloud iam workload-identity-pools providers create-oidc github \
  --location=global --workload-identity-pool=github \
  --issuer-uri="https://token.actions.githubusercontent.com" \
  --attribute-mapping="google.subject=assertion.sub,attribute.repository=assertion.repository,attribute.repository_owner=assertion.repository_owner" \
  --attribute-condition="assertion.repository=='kukuri-app/kukuri'"

# provider が既に作成済みの場合は create-oidc の代わりに update-oidc を使う:
# gcloud iam workload-identity-pools providers update-oidc github \
#   --location=global --workload-identity-pool=github \
#   --attribute-mapping="google.subject=assertion.sub,attribute.repository=assertion.repository,attribute.repository_owner=assertion.repository_owner" \
#   --attribute-condition="assertion.repository=='kukuri-app/kukuri'"

# CI 用 service account（plan に必要な read/metadata 権限を付与）
gcloud iam service-accounts create kukuri-tf-ci
# 例: roles/compute.viewer, roles/storage.admin(state), roles/secretmanager.viewer,
#     roles/iam.serviceAccountViewer など。plan で参照する範囲に合わせる。
```

GitHub repository variables（Settings → Secrets and variables → Actions → Variables）に登録:

- `GCP_WORKLOAD_IDENTITY_PROVIDER`: `projects/900604885452/locations/global/workloadIdentityPools/github/providers/github` のような provider resource name（短い `github` だけでは不可）
- `GCP_SERVICE_ACCOUNT`: `kukuri-tf-ci@kukuri-cn.iam.gserviceaccount.com` のような service account email（短い `kukuri-tf-ci` だけでは不可）
- `GCP_PROJECT_ID`, `GCP_REGION`, `GCP_ZONE`
- `CN_API_DOMAIN`, `CN_RELAY_DOMAIN`, `CN_ACME_EMAIL`
- `CN_USER_API_IMAGE`, `CN_IROH_RELAY_IMAGE`, `CN_CLI_IMAGE`
- `CN_JWT_SECRET_ID`, `CN_POSTGRES_PASSWORD_SECRET_ID`
- `TF_BACKEND_BUCKET`, `TF_BACKEND_PREFIX`
- 任意: `CN_MANAGE_CLOUD_DNS`, `CN_DNS_ZONE_NAME`

apply 済み環境では、個別変数の代わりに **生成済み terraform.tfvars 全体**を
`CN_LOW_COST_TFVARS_B64` に載せる（設定されていれば個別 TF_VAR より優先される）。
個別変数の mirror では実 state と乖離し、plan が `prevent_destroy` 資源（Postgres data PD）の
破棄計画になって失敗するため。`generate-tfvars` で tfvars を更新したら毎回これも更新する:

```bash
grep -v '^operator_config_path' infra/terraform/envs/low-cost/terraform.tfvars \
  | base64 -w0 | gh variable set CN_LOW_COST_TFVARS_B64
```

`operator_config_path` 行を除くのは、CI checkout には operator-config.yaml（gitignore 済み）が
存在せず `file()` が失敗するため。tfvars は secret ID のみで secret 値を含まない。

> CI は `plan` まで。`apply` は実行しない。

## deployment profile と cn-operator capability profile

- `low-cost` / `managed-db` / `ha` は **インフラ**のコスト/データ階層の軸（この Terraform）。
- `minimal` / `relay-enabled` / `full-service` は **cn-operator** の開示・manifest 用 capability の軸
  （`docs/runbooks/community-node-operator-docs.md`）。
- 両者は独立した軸だが、`operator-config.yaml` を単一の入力元にして両方を生成できる（#380）。
  operator-config の `deploy:` セクションがコスト軸、`profile` / `features` が capability 軸。

## operator-config から一気通貫でセットアップ（#380）

`cn-operator` を起点に docs / manifest と terraform env 設定の両方を生成し、デプロイ時に
operator-config.yaml を VM へ配置して public manifest endpoint を有効化できる。

```bash
cd infra/terraform/envs/low-cost

# operator-config.yaml を用意（deploy: セクションを含める）
cn-operator init --out operator-config.yaml
# operator-config.yaml を編集（server / features / deploy を埋める）
cn-operator validate-config --config operator-config.yaml

# 同じ config から terraform.tfvars を生成
cn-operator generate-tfvars --config operator-config.yaml --out terraform.tfvars

cp backend.hcl.example backend.hcl   # state bucket/prefix を埋める
terraform init -backend-config=backend.hcl
terraform plan
terraform apply
```

- `terraform.tfvars` の末尾コメントに従い `operator_config_path = "operator-config.yaml"`
  を有効化すると、Terraform が env dir からの相対 path を読み込み、起動時に
  operator-config.yaml が VM の `/etc/kukuri/operator-config.yaml` へ配置され、
  `cn-user-api` が `COMMUNITY_NODE_OPERATOR_CONFIG` で読み込む。
- これにより `GET /.well-known/kukuri/community-node.json` / `GET /v1/node/manifest` が応答し、
  `report_endpoint` capability を有効化した node では `POST /v1/report` が受理される。
- manifest に記載される `GET /terms` / `GET /privacy` / `GET /external-transmission` /
  `GET /moderation-policy` / `GET /abuse-policy` と、公開用の `GET /data-retention` も、
  同じ operator config から生成した Markdown を応答する。デプロイ後は manifest の各 URL が
  200 であり、`Content-Type` が
  `text/markdown; charset=utf-8` であることを確認する。
- `operator_config_path` が空（既定）なら manifest endpoint は `404` のまま（従来挙動）。
- blob cache の on/off は operator-config の `features.blob_cache` が真実源で、tfvars の
  `blob_cache_enabled` はそこから導出される。`features.blob_cache: false` なのに
  `deploy.blob_cache_size_gb > 0` を指定すると `validate-config` / `generate-tfvars` が失敗する。
- tfvars 生成は `low-cost` のみ対応。`managed-db` / `ha` は拡張点。

## 関連

- `infra/terraform/README.md`
- `docs/runbooks/community-node-self-host-vps.md`（VPS edge の手動 self-host）
- `docs/runbooks/community-node-operator-docs.md`（cn-operator の文書生成）
- `docs/architecture/p2p-first-community-node-responsibility-boundary.md`

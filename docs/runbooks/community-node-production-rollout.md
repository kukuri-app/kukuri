# Community Node Production Rollout / Live Verification

最終更新日: 2026-09-24

専用 `openai-moderation` を使用する配備は、[動画・OpenAI Moderationの運用](community-node-openai-moderation.md) の設定、tmpfs、合成readiness probe、構成世代更新も適用する。

## 目的

この runbook は、GCP `low-cost` Community Node の production image 更新から Terraform、
startup、readiness、実クライアント投稿の追跡、公開境界の復旧までを一続きで実行するための
**KingYoSun が運営する default onboarding Node 専用**の日常運用手順である。ここに記載する
domain、actor、image digest は汎用既定値ではない。Terraform の初期構築と各変数の説明は
`docs/runbooks/community-node-gcp-terraform.md`、クライアント配布は
`docs/runbooks/release.md` を参照する。

この手順で特に防ぐ事故は次の4つ。

- workflow log に出た smoke build の digest を production digest と誤認する
- `metadata_startup_script` の差分だけで既存 VM を意図せず置換する
- image pull 中に Docker 領域が枯渇し、更新途中で startup が止まる
- health/readiness だけで完了とし、実投稿の media fetch・allow-only index・非残留を見落とす

## 0. 変数と記録先

対象revision・node・capability、今回確認するtopic/object、観測期間・待機期限と成功条件を先に固定する。標準rolloutは更新した構成の起動・readiness・必要な公開面と実投稿の確認で終了する。障害復旧・secret rotation・過去migrationの個別確認は該当するものだけ選び、全分岐を毎回実行しない。期限を超えて判定できなければ未完了とし、対象や試行を増やし続けない。

作業開始前に値を固定し、同じ値を作業記録へ残す。secret 値は記録しない。

```bash
export KUKURI_GCP_PROJECT="YOUR_PROJECT"
export KUKURI_GCP_ZONE="asia-northeast1-a"
export KUKURI_VM="kukuri-cn-vm"
export KUKURI_REPO="kukuri-app/kukuri"
export KUKURI_MAIN_SHA="<40-character-main-sha>"
export KUKURI_SHA_TAG="sha-$(printf '%s' "$KUKURI_MAIN_SHA" | cut -c1-12)"
```

PowerShell:

```powershell
$env:KUKURI_GCP_PROJECT = 'YOUR_PROJECT'
$env:KUKURI_GCP_ZONE = 'asia-northeast1-a'
$env:KUKURI_VM = 'kukuri-cn-vm'
$env:KUKURI_REPO = 'kukuri-app/kukuri'
$env:KUKURI_MAIN_SHA = '<40-character-main-sha>'
$env:KUKURI_SHA_TAG = 'sha-' + $env:KUKURI_MAIN_SHA.Substring(0, 12)
```

記録するもの:

- main SHA、PR、Fast CI run、Community Node Images run
- 4 image の **registryで解決したdigest**
- apply前backup object、generation、size
- Terraform plan の add/change/destroy と replacement の有無
- startup script の desired / metadata SHA-256
- container、timer、readiness、truth/projection、scan/media metrics
- 障害・復旧操作、削除したもの、rollback先digest

## 1. publish と digest の確定

1. PRの全check成功を確認してから対象変更をmainへ反映する。
2. main の `Kukuri Fast` と `Kukuri Community Node Images` を最後まで確認する。
3. image job 成功後、GHCR に存在する `sha-<12桁>` tag から digest を取得する。
4. digest参照が実際にmanifestとして解決できることを検査する。

```bash
gh pr checks <pr-number> --repo "$KUKURI_REPO" --watch
gh pr merge <pr-number> --repo "$KUKURI_REPO" --squash --delete-branch
```

repositoryにrequired checkが設定されていない場合、`gh pr merge --auto` はcheck待ちにならず即時merge
されることがある。CI成功後にmergeする運用ではauto-merge登録に依存せず、`gh pr checks --watch` の
成功を明示確認してからmergeする。merge後に起動するmain workflowも別に完走確認する。

```bash
for image in kukuri-cn-user-api kukuri-cn-iroh-relay kukuri-cn-cli kukuri-cn-indexer; do
  ref="ghcr.io/kukuri-app/${image}:${KUKURI_SHA_TAG}"
  digest="$(docker buildx imagetools inspect "$ref" --format '{{println .Manifest.Digest}}')"
  test -n "$digest"
  docker manifest inspect "ghcr.io/kukuri-app/${image}@${digest}" >/dev/null
  printf '%s@%s\n' "ghcr.io/kukuri-app/${image}" "$digest"
done
```

workflow log の最初の `sha256:` をコピーしてはならない。`cn-indexer` job は publish対象とは別に
positive smoke用imageを先にbuildすることがあり、そのdigestはGHCRにpushされない。真実源は
job成功後のregistry manifestである。VMからも `docker manifest inspect <image>@<digest>` が通ることを
確認すれば、公開範囲・認証・digestの取り違えをapply前に検出できる。

取得した4参照を `operator-config.yaml` と
`infra/terraform/envs/low-cost/terraform.tfvars` の両方へ反映し、差異が無いことを確認する。

## 2. apply前 gate

### 2.1 検証とbackup

`operator-config.yaml` の `server.node_id` が、Secret Manager の署名鍵から導出した発行元識別子
(`cn-cli moderation issuer-node-id`。導出手順は `community-node-gcp-terraform.md`)と一致することを
先に確認する(#706)。不一致のままだと cn-user-api は起動を拒否し、異議申し立ても受理されない。
署名鍵を差し替えた場合は node_id も更新し、生成文書を再生成する。

```bash
cargo run -q -p kukuri-cn-operator -- validate-config --config operator-config.yaml
cargo xtask cn-check
cargo xtask cn-test
cargo xtask cn-e2e

terraform -chdir=infra/terraform fmt -check -recursive
terraform -chdir=infra/terraform/envs/low-cost init
terraform -chdir=infra/terraform/envs/low-cost validate
terraform -chdir=infra/terraform/envs/low-cost plan -out=tfplan
```

本番DBのbackupを先に取得し、GCS objectのgenerationとsizeを記録する。既存の
`kukuri-backup.service` を使う場合:

```bash
gcloud compute ssh "$KUKURI_VM" \
  --project "$KUKURI_GCP_PROJECT" --zone "$KUKURI_GCP_ZONE" \
  --tunnel-through-iap --command 'sudo systemctl start kukuri-backup.service'
gcloud storage ls -l "gs://<backup-bucket>/postgres/**"
```

### 2.2 replacement gate

`terraform plan` に `module.vm.google_compute_instance.vm` のreplacementがあれば、そのままapplyしない。
次をすべて満たす場合だけ後述のstartup metadata in-place同期を使える。

- replacement理由が `metadata_startup_script` のみ
- machine type、boot disk、network、service account、attached PDに同時変更が無い
- Postgres / indexerのPDと最新backupを確認済み
- DNS / ACME / 証明書を保持する必要があり、計画停止でVM置換するよりin-placeが安全

他のreplace理由が混じる場合はmaintenance windowを取り、通常のVM置換として扱う。

## 3. startup metadata の in-place 同期（限定的なbreak-glass手順）

`metadata_startup_script` だけの置換を避ける場合、review済みplanからdesired scriptを抽出し、
Compute Engine metadataの `startup-script` だけを更新する。作業用fileはsecret値を含まない設計だが、
operator configや内部URLを含み得るので一時fileとして扱う。

`jq` を使う例:

```bash
rollout_tmp="$(mktemp -d)"
trap 'rm -f -- "$rollout_tmp/startup.sh"; rmdir -- "$rollout_tmp"' EXIT

terraform -chdir=infra/terraform/envs/low-cost show -json tfplan \
  | jq -j '.resource_changes[]
      | select(.address == "module.vm.google_compute_instance.vm")
      | .change.after.metadata_startup_script' \
  >"$rollout_tmp/startup.sh"
test -s "$rollout_tmp/startup.sh"
sha256sum "$rollout_tmp/startup.sh"

gcloud compute instances add-metadata "$KUKURI_VM" \
  --project "$KUKURI_GCP_PROJECT" --zone "$KUKURI_GCP_ZONE" \
  --metadata-from-file "startup-script=$rollout_tmp/startup.sh"
```

PowerShell 7でplanから抽出する場合は、BOMを付けずLFを維持する。

```powershell
$plan = terraform -chdir=infra/terraform/envs/low-cost show -json tfplan |
  ConvertFrom-Json -Depth 100
$script = $plan.resource_changes |
  Where-Object address -eq 'module.vm.google_compute_instance.vm' |
  ForEach-Object { $_.change.after.metadata_startup_script }
if ([string]::IsNullOrWhiteSpace($script)) { throw 'desired startup script not found' }
$tmp = Join-Path ([IO.Path]::GetTempPath()) 'kukuri-startup.sh'
[IO.File]::WriteAllText($tmp, $script, [Text.UTF8Encoding]::new($false))
Get-FileHash -Algorithm SHA256 $tmp
gcloud compute instances add-metadata $env:KUKURI_VM `
  --project $env:KUKURI_GCP_PROJECT --zone $env:KUKURI_GCP_ZONE `
  --metadata-from-file "startup-script=$tmp"
```

metadata server側のbytesと一致することを確認する。

```bash
gcloud compute ssh "$KUKURI_VM" \
  --project "$KUKURI_GCP_PROJECT" --zone "$KUKURI_GCP_ZONE" \
  --tunnel-through-iap --command \
  "curl -fsS -H 'Metadata-Flavor: Google' \
   http://metadata.google.internal/computeMetadata/v1/instance/attributes/startup-script \
   | sha256sum"
```

古い `tfplan` はreplacementを含むため絶対にapplyしない。metadata同期後にplanを作り直し、VMの
replacementが消えたことと、残るadd/change/destroyを再reviewしてから新しいplanだけをapplyする。

```bash
terraform -chdir=infra/terraform/envs/low-cost plan -out=tfplan-safe
terraform -chdir=infra/terraform/envs/low-cost apply tfplan-safe
terraform -chdir=infra/terraform/envs/low-cost plan   # 最終的に No changes
```

`add-metadata` は既存metadataを保持して指定keyを更新する。Compute APIの `setMetadata` を直接使う
場合は、必ず現在のfingerprintと全metadata itemを読み、他keyを欠落させない。

## 4. startup と容量枯渇の復旧

Container-Optimized OSではCompose v2 plugin discoveryを前提にしない。必ずTerraformが配置した
standalone binaryを使う。

```bash
cd /var/lib/kukuri/community-node
COMPOSE=/var/lib/toolbox/kukuri/bin/docker-compose
sudo "$COMPOSE" ps
```

startup前に容量を確認する。

```bash
df -h /var/lib/docker
sudo docker system df
sudo docker image ls --filter dangling=true
```

新旧4 imageを同時に保持できる空きが無ければ、先にmaintenance判断を行う。startupは次で再実行する。

```bash
sudo systemctl restart google-startup-scripts.service
sudo journalctl -u google-startup-scripts.service -f
```

成功条件はjournalの `[kukuri-startup] bootstrap complete` と、対象containerのrunning/healthy。

`no space left on device` で止まった場合、既存containerがhealthyなら慌ててvolumeを消さない。
次の順で復旧する。

1. `docker ps -a` で稼働・停止containerを記録する。
2. `docker system df` と `df -h /var/lib/docker` を記録する。
3. dangling imageだけを確認する。
4. `sudo docker image prune -f` を実行する。
5. 回収量と削除対象がregistryから再pull可能であることを記録する。
6. startup serviceを再実行する。

rollout中に `docker system prune`、`docker image prune -a`、`docker volume prune` は使わない。
特にPostgres / ArcadeDB / indexer dataのvolume・PDを容量回収対象にしてはならない。

## 5. post-deploy verification

### 5.1 image / container / timer

```bash
cd /var/lib/kukuri/community-node
COMPOSE=/var/lib/toolbox/kukuri/bin/docker-compose
sudo "$COMPOSE" ps
for container in community-node-cn-user-api-1 community-node-cn-iroh-relay-1 \
  community-node-cn-indexer-1; do
  sudo docker inspect "$container" \
    --format '{{.Name}}|{{.Config.Image}}|{{index .Config.Labels "org.opencontainers.image.revision"}}'
done

for unit in kukuri-readiness.timer kukuri-monitor.timer \
  kukuri-backup.timer kukuri-relation-analyze.timer; do
  sudo systemctl is-enabled "$unit"
  sudo systemctl is-active "$unit"
done
```

### 5.2 readiness

readinessは手動でも `kukuri-readiness.service` 経由で実行する。`docker-compose run` だけで実行すると
serviceの起動記録が残らず、timerの5分間隔の再判定と実行結果の追跡から外れる。

```bash
sudo systemctl start kukuri-readiness.service
sudo journalctl -u kukuri-readiness.service -n 100 --no-pager
sudo systemctl list-timers kukuri-readiness.timer --all
```

`list-timers` の `NEXT` が時刻になっていることを確認する。`-` の場合はtimerが次回実行を持たず、
activationの有効期限（既定15分）後にindex / trustが閉じる。startup再実行直後はtimer起動の2分後に
初回が予定される（#1097）。それ以外で `-` なら `sudo systemctl start kukuri-readiness.service` を
実行し、`NEXT` が決まったことを記録する。

provider設定・キー・decoderの変更後など、保存済みprobeを使わずに再判定する場合（`--force-probe`）は、
serviceに引数を渡せないため次の2段階で行う。force-probeの実行が合格すると新しいprobe結果が保存され、
続くserviceの実行はそれを再利用する（15分以内）。force-probeが不合格なら旧activationはrevokeされ、
続くserviceも不合格のまま閉じる。

```bash
cd /var/lib/kukuri/community-node
sudo /var/lib/toolbox/kukuri/bin/docker-compose run --rm cn-readiness \
  readiness --config /etc/kukuri/operator-config.yaml --force-probe
sudo systemctl start kukuri-readiness.service
sudo journalctl -u kukuri-readiness.service -n 100 --no-pager
sudo systemctl list-timers kukuri-readiness.timer --all
```

最低条件:

- `ready=true fail=0 unknown=0`
- provider credentialが全slotでpass
- `permanent_blob_storage_disabled` がpass
- worker running、supported public scopes opened、sync / ingest fresh
- `post_scheduler` の processing / retry_wait / oldest_pending_at が実進捗と整合し、同じ値のまま停止していない
- 固定した無害な検証投稿が期待どおり処理され、失敗をallowへ変えるfallbackが0。過去の累積エラーや対象外peerの取得不能だけを今回の失敗としない
- 固定した対象objectのPostgres truthとArcadeDB projectionが一致。全履歴の走査・再構築を待たない
- relation analysis recent

readinessの成功だけではlive media確認の代わりにならない。

### 5.2.1 IAP admin UI

Terraform apply 後、local workstation で次を実行する。

```bash
cd infra/terraform/envs/low-cost
terraform output -raw admin_iap_tunnel_command
# 表示された command を別 terminal で実行し、http://localhost:9090 を開く
```

画面で user API、admission mode、最新 readiness、supported topics、直近の通報、operator action auditを照合する。
この listener は Caddy / public DNS に接続せず、firewall は Google IAP TCP forwarding range のみを
許可する。

browser writeは `admin_actor` が非空のdeploymentだけで有効になる。この default Node の
非公開実設定では次の共有運用identityを明示する。IAP TCP forwardingはHTTP identity headerを注入しないため、
formや任意headerではなくdeployment値だけをaudit actorとして信用する。

```hcl
admin_actor = "ops@kukuri.app"
```

画面から適用できるのは、runtime Postgresがcanonical sourceの次の操作だけである。

- admission mode変更
- public supported topic追加・除去
- report status変更（received / reviewing / actioned / dismissed）
- 異議申し立て中のリスク判定の認容・棄却・検知情報調整・訂正版再発行

すべて `Preview` でactor・target・impactを確認し、次画面の `Confirm and apply` で確定する。apply時に
入力と現在値を再検証し、state変更と `cn_admin.operator_actions` audit rowを同じtransactionでcommitする。
成功画面のaudit IDを運用記録へ残す。戻る操作またはvalidation errorではstateもauditも変更されない。

`cn_admin.operator_actions` はDB triggerでUPDATE / DELETEを拒否する。auditにはprovider credential、
report details、reporter contact、private channel secretを含めない。actor未設定時はread-only表示になり、
write endpointは503でfail-closedする。

異議申し立て審査の変更操作には、既定で有効な `COMMUNITY_NODE_SAFETY_OPERATOR_REVIEW=true` も必要である。無効時は
申し立て内容と対象のリスク判定を参照できるが、操作欄は参照専用になる。同じリスク判定へ複数の
申し立てが届いても、一覧では一つの審査対象にまとめて表示する。

- 認容: `Disputed` から `Cleared` へ移し、関連通報を `actioned` にする。次回の信頼評価から寄与を除外する。
- 棄却: `Disputed` から `None` へ戻し、関連通報を `dismissed` にする。信頼評価への寄与は維持する。
- 検知情報調整: 分類、深刻度、確信度、失効時刻を変更する。異議申し立ては審査中のままにする。
- 訂正版再発行: 旧判定を認容（寄与なし）として終結させたまま残し、公開範囲を含む訂正版を新規発行する。関連通報は処理済みになる（#710）。

確認画面を開いた後で判定または関連通報が変化した場合、適用は拒否される。画面を読み直し、現在値を
確認してから再度適用する。適用失敗時は、対象のリスク判定、関連通報、`cn_admin.operator_actions` を
照合する。操作記録が無ければ同じ取引内の変更も確定していない。操作記録があるのに状態が一致しない
場合は追加操作を行わず、Postgres のバックアップを確保して障害として調査する。

次はbrowserから変更しない。reviewed `operator-config.yaml` / Terraform / Secret Manager /
`cn-cli readiness`の既存workflowを使う。

- provider / LLM / Project Arachnid endpoint・credential
- capability / authority scope / image revision
- private channel secret、invite code、allowlist、ban

汎用 Compose では `http://127.0.0.1:19090` がadmin UIで、actor の既定値は空（read-only）である。
この default Node だけは `COMMUNITY_NODE_ADMIN_ACTOR=ops@kukuri.app` を実設定から注入し、
operator reviewも有効にする。
loopback以外へbindする場合は、先に同等の認証・firewall境界を用意する。

異議申し立て審査の変更操作は、`COMMUNITY_NODE_ADMIN_ACTOR` に加えて運用者設定
`safety.moderation.operator_review`（terraform 変数 `safety_operator_review` →
`COMMUNITY_NODE_SAFETY_OPERATOR_REVIEW`）の有効化が必要（標準配備は既定有効。#709）。
無効化・再有効化手順は `docs/runbooks/openai-compatible-vlm.md` の「appeal / operator レビュー運用」を参照。

### 5.2.2 default onboarding node の索引可用性

この節は本runbookのdefault onboarding nodeだけに適用する。利用者側の既定購読とノードの
`cn_index.supported_topics` は別の状態であり、image更新・readiness成功では同期されない。
`general` / `test` / `dev` の公開索引を提供するこのnodeでは、管理画面の対応トピックに
`kukuri:topic:general` / `kukuri:topic:test` / `kukuri:topic:dev` があることを確認する。
欠落していれば、5.2.1のpreview→applyで追加し、監査IDを残す。既存topicを削除しない。

非公開の `infra/terraform/envs/low-cost/terraform.tfvars` に期待集合を明示する。
これは監視条件であり、DBを自動変更しない。他のnodeへ強制する既定値ではない。

```hcl
index_expected_topics = ["kukuri:topic:general", "kukuri:topic:test", "kukuri:topic:dev"]
```

GCP plan CIを有効にしている場合は、`CN_LOW_COST_TFVARS_B64`の期待集合も同じ値へ同期する。
この更新では既存の他設定を保持する。ローカルtfvarsだけの更新ではCIが警報削除を計画し得る。

`kukuri-monitor.timer` が5分ごとに次のCloud Monitoring metricを送る。

| metric（`custom.googleapis.com/kukuri/community_node/` 配下） | 意味・扱い |
| --- | --- |
| `index_expected_topics_present` | 期待する公開topicがすべてDBにあれば1。欠落・DB取得失敗なら0。0が5分続くと既存通知先に警報 |
| `index_expected_topics_entries` | 期待する公開topic群の索引行数。DB取得失敗は-1。0以下が5分続くと警報 |
| `body_fetch_failures_recent` | 直近10分の本文取得失敗log件数。log取得失敗は-1（不明）。peer依存のため単独の通知警報は作らず、provider障害と区別して調査 |

期待集合が空のnodeには、先頭2つのmetricの時系列・警報は作らない。索引stack無効時も警報を作らない。
監視はread-only SELECTと状態/logの参照だけを行う。metricへ本文・鍵・ピア識別子を含めない。
本文失敗の計数はindexerの `failed to resolve post body; not indexing the post (fail-closed)` logを
使うため、このlogを変更する際は `infra/terraform/modules/gcp-vm-compose/scripts/index-health.sh`
とfixtureを同時に更新する。

監視script更新時は、Terraformで生成したstartupの変更が監視部分だけであることを確認する。
3節のin-place metadata同期でVM置換を避け、生成されたmonitor scriptも既存の
`/var/lib/kukuri/community-node/monitor.sh`へowner root・mode 700で反映する。
反映前に同fileをbackupし、`bash -n`を通す。readinessやcontainerのrestartは不要。
`sudo systemctl start kukuri-monitor.service`、終了コード、Monitoring上の実値、警報の有効化と
既存通知先の保持を確認する。metadataとruntime fileを一致させ、次回startupで旧監視に戻さない。

監視の回帰確認（Linux/WSL内のPython・bashを使用。Windows PythonからWSLのbashを直接呼ばない）:

```bash
python3 -m unittest discover -s infra/terraform/tests
terraform -chdir=infra/terraform fmt -check -recursive
terraform -chdir=infra/terraform/envs/low-cost validate
terraform -chdir=infra/terraform/envs/low-cost test
```

### 5.3 public surface

```bash
curl -fsS "https://<api-domain>/healthz"
curl -fsS "https://<relay-domain>/ping"
curl -fsS "https://<api-domain>/.well-known/kukuri/community-node.json"
curl -fsS "https://<api-domain>/v1/node/manifest"
```

manifestが示すterms / privacy / external-transmission / moderation-policy / abuse-policy /
data-retentionもHTTP 200と `text/markdown; charset=utf-8` を確認する。manifest の `node_id` が
`cn-cli moderation issuer-node-id` の出力(署名鍵の公開鍵 hex)と一致することも確認する
(異議申し立ての発行元照合に使われる。#706)。index / trust surfaceは、
有効時に未認証401または入力不備400となり、構成未完了を示す404へ戻っていないことを確認する。

### 5.4 monitoring通知とlog / secret監査

notification channelを追加・変更したrolloutでは、Terraformが全Community Node alert policyへ同じ
channelを付けたことをAPIで照合する。Email channelはCloud Monitoring側の `enabled=true` だけで
完了とせず、受信側でOPENED / CLOSEDの両方を確認する。

配送試験は本番policyを書き換えず、`[TEST]` prefixの一時policyを作る。既存custom metricへ短いtest値を
送り、一時policyでは即時評価、本番policyの継続条件（既定5分）より短く終了させる。OPENED受信後に
正常値を送り、CLOSED受信を確認して一時policyを削除する。最後に次を記録する。

- channel resource name、type、enabled
- 本番policy総数とchannel添付数
- test signal開始 / 復旧時刻
- OPENED / CLOSED受信
- 残存 `[TEST]` policy 0件、notification error 0件

logのsecret非含有監査では、secret値を `grep "$SECRET" ...` のようにcommand lineへ載せてはならない。
値は権限0700の一時directoryへ取得し、root-only scriptのprocess内で読み、出力はsecret IDごとの
match countだけにする。対象期間を固定したjournal、startup log、対象container logを調べ、`matches=0`を記録する。過去全ログへ範囲を拡張しない。

誤ってsecret値をargvやjournalへ出した場合は、そこで監査を止める。対象secretをrotateし、該当する
journal archive / container logを保持方針に従ってrotate・vacuumした後、新旧両方の値で0件を再確認する。
秘密値そのものをincident記録へ転記しない。

### 5.5 `BlobText` 本文の再投影と検索確認

本文取得処理を変更した `cn-indexer` のrolloutでは、ArcadeDBのvolumeやentryを手動削除しない。
現行workerには起動時・周期的な全件見直しが残るが、これは設計原則上の未解消点である。全件処理の完了を運用上の成功条件にせず、対象objectと確認期限を固定して次を実施する。

1. `cn-indexer` の `/v1/status` と対象期間のlogで、固定した投稿の処理が進み、対象scopeが停止していないことを確認する。期限内に到達しなければ取得不能・待機・処理失敗を区別して記録する。
2. read-onlyのArcadeDB照会で、対象objectの `text` が空文字でなく、期待する本文を含むことを確認する。
3. topic内検索とsupported set横断検索でASCII語と日本語語をそれぞれ検索し、期待object IDが返ることを確認する。
4. 同じscopeの発見一覧にも同じobject IDが存在し、検索だけが欠落する不整合が無いことを確認する。
5. 本文拒否の挙動を変更する場合は、受入条件に含めた取得不能・不正本文等について、空本文entryや未検証本文を投影しないことを関連testで確認する。rolloutのたびに未依頼の異常系を追加しない。

`BlobText` のraw bytesは `BlobService::fetch_blob_ephemeral` で取得し、検証とscanの間だけ保持する。
Postgresは本文を持たず、ArcadeDBには検証・allow判定済みの検索用textだけを投影する。本文blobの非残留は
6.2と同じlocal miss、ephemeral transfer、再度のlocal missの組で確認する。

default onboarding nodeのrollout完了・障害復旧では、本文処理の差分有無にかかわらず、
既定3topicそれぞれの実投稿についてこの検索確認を行う。既存の無害な公開投稿を使える場合は
新規投稿は不要。新規投稿には検証であると分かる本文を使い、topic・object ID・時刻を記録する。
投稿元を接続・購読状態に保ち、日本語/ASCII検索、topic内検索、横断の発見・おすすめで同じ
object IDが返ることを確認する。CLIでは `set_topic_gossip_enabled` を `enabled=true` で呼び、
`get_sync_status` の `subscribed_topics` に対象があることを確認する。`create_post`の成功だけは
購読開始の証拠にならない。

readinessの `truth=projection=0` はデータ整合性の結果であり、検索可能性の成功証拠ではない。
本文取得失敗も `skipped_non_allow` に含まれるため、有害判定と同一視しない。本文が再取得不能な
場合は索引を抑止する既存境界を維持し、供給元の接続・保持状態と最新logを照合する。
今回の対象投稿について実結果と監視値を確認する。過去の成功を今回の対象の結果に読み替えず、全件再巡回や追加の再起動をその確認手段として強制しない。

#1212以降、投稿処理は`COMMUNITY_NODE_INDEXER_MAX_CONCURRENT_POSTS`（既定4）で上限付き並列化される。
復旧時は`/v1/status`の`post_scheduler`と`last_pass_duration_ms`を記録する。並列度を上げる前にVMの
memory、safety providerの同時実行制限、peer側負荷を確認し、一つずつ増やす。取得不能を解消するために
readinessの鮮度やfail-closed判定を緩めない。remote blob取得は1件30秒のattempt budgetでretry_waitへ
移り、次のreconcileで再試行される。retry_waitの増加とindexed時刻の前進を区別して確認する。

### 5.6 verdict再利用とrisk signal集約の確認（#1050）

`cn-indexer`は内容とscan構成が不変のsubjectについて保存済みverdictを再利用し、risk signalを鍵ごとに集約する。この経路を変更するrolloutで、事前に選んだ既存投稿と新規投稿だけを確認する。

1. [重複集約migration](../../crates/cn-core/migrations/202609150002_risk_signal_dedupe.sql)を初めて導入する場合は、backup取得後・`cn-migrate`前に移行対象の活性重複鍵と通報から参照される行を確認する。同一鍵に参照行が2件以上ある場合、migrationは2件目以降を削除せず失効させるため、該当鍵が0件であることを確認する。0件でなければ適用を止め、既存通報への影響と実行可否を先に確定する。適用後はmigration記録、対象鍵の結果と `uq_cn_safety_risk_signals_active_key` の存在を確認する。この移行対象の事前確認を、適用済みの通常rolloutで毎回実行する全DB照合へ広げない。
2. 対象の既存投稿を処理した際の `scans_reused` / `scans_fresh` と対象risk signalを照合し、同じ内容・構成で不要な再scanや追加行が発生しないことを確認する。
3. 無害な新規投稿1件の到着から `indexed_at` までを、固定した待機期限で確認する。他投稿の再scanや全件passの終了を待たない。
4. 対象通知の前後で `scanned` / `last_event_ingest_duration_ms` と対象期間のlogを確認する。既知IDは対象別、対象不明keyは現在索引窓として処理され、今回選んだ投稿の結果へ到達したかを区別する。全prefix読取り0・100 ID上限の判定は変更PRのcontractで行い、rollout時のstatus値だけから全履歴非走査を推定しない。
5. peer更新を変更した場合は、対象peerの登録後の本文/media取得を同じ投稿で確認する。対象batchの失敗時はその原因を記録し、全peer・全scopeの強制再適用で成功扱いにしない。

旧workerのopen/購読は最大32物理replica、新公開readerの受付は最大32scopeであり、`opened_scopes`は
全support件数ではなくその時点の旧workerの予約枠（途中失敗でopen状態が不明な枠を含む）を表す。
定常の失効scope照合は最大32件の永続cursorで進む。手動全scope取込は#1221 R5-Eで撤去したが、旧namespace同期は残るため、
この有限な運用確認の成功を、保持量・失敗回数への依存を含む#1221全体の解消と読み替えない。

### 5.7 content advisory 付き索引と trust 不変の確認（#1054）

nsfw / objectionable の suspected は `allow` + content advisory で索引され（ADR 0028 §8）、trust の
評価値には寄与しない。`policy_version` が `2026-09-public-node-v3` になり scan 構成 fingerprint が
変わる構成では、現行実装が過去投稿を再scanし得る。これは当時の移行挙動であり、全件backfillを現在の設計目標やrolloutの終了条件にはしない。確認対象の投稿を先に固定する。

1. 該当migrationの適用記録と `cn_safety.scan_verdicts.advisory_labels` 列の存在を確認する。既適用の環境で全既存行が初期値であることを再要求しない。

2. 対象処理の完了後、nsfw 相当の benign 投稿（過去に `exclude` だったもの、または検証用の投稿）の
   verdict が `action = allow` / `policy_version = 2026-09-public-node-v3` で、`advisory_labels` に
   `category` / `label`（`adult` または `sensitive`）/ `signal_id` を持つこと。対応する
   `cn_index.index_entries` 行があること。

```bash
OBJECT_ID="<64-hex-post-id>"
case "$OBJECT_ID" in (*[!0-9a-f]*|'') echo 'invalid object id' >&2; exit 1;; esac
test "${#OBJECT_ID}" -eq 64 || exit 1
PG_CONTAINER="$(sudo docker ps -qf name=cn-postgres)"
sudo docker exec "$PG_CONTAINER" sh -lc \
  "psql -U \"\$POSTGRES_USER\" -d \"\$POSTGRES_DB\" -At -F '|' \
   -c \"SELECT v.subject_id, v.action, v.policy_version, v.advisory_labels
       FROM cn_safety.scan_verdicts v
       WHERE v.subject_kind = 'post' AND v.subject_id = '$OBJECT_ID';\""
```

3. 認証・同意済み client から `GET /v1/index/search?scope_kind=public_topic&scope_id=<topic>&q=<語>`
   を呼び、該当 entry の `content_advisories` が上の `advisory_labels` と一致し、`content_labels` が
   応答に無いことを確認する（第 2 のラベル源であって署名済みラベルではない）。
4. 著者の `GET /v1/trust/users/{pubkey}` で、nsfw / objectionable の basis 行が `contribution = 0` /
   `raw_contribution = 0` で並び、`relative` / `trust` が反映前の値から動いていないこと。
   `GET /v1/trust/pull/{pubkey}` の basis にこれらが出ないこと。
5. `cn_safety.risk_signals` で対象投稿の signal が 1 件（`severity = low`、`basis = classifier_score`）
   であること。固定した対象投稿の再処理でも同じ鍵の行が増えないこと（§5.6）。
6. `general_action` を `hold` / `exclude` へ厳格化した node では、同じ投稿が索引に入らないこと
   （既定の `label` 運用では確認不要）。

## 6. 実クライアントのbenign media確認

実在の違法mediaや疑わしいmediaを検証に使わない。権利上問題のない小さな画像をpublic topicへ投稿し、
JST時刻、topic、post object ID、media hashを記録する。

### 6.1 index truth

VM上でobject IDを64桁hexに限定してから照会する。

```bash
OBJECT_ID="<64-hex-post-id>"
case "$OBJECT_ID" in (*[!0-9a-f]*|'') echo 'invalid object id' >&2; exit 1;; esac
test "${#OBJECT_ID}" -eq 64

PG_CONTAINER="$(sudo docker ps -qf name=cn-postgres)"
sudo docker exec "$PG_CONTAINER" sh -lc \
  "psql -U \"\$POSTGRES_USER\" -d \"\$POSTGRES_DB\" -At -F '|' \
   -c \"SELECT scope_kind, scope_id, object_id, author_pubkey, verdict_action, critical, indexed_at
       FROM cn_index.index_entries WHERE object_id = '$OBJECT_ID';\""
```

benign投稿の成功条件は対象rowが `public_topic`、期待topic、`allow`、`critical=false` であること。

### 6.2 ephemeral fetch / 非残留

```bash
MEDIA_HASH="<64-hex-media-hash>"
case "$MEDIA_HASH" in (*[!0-9a-f]*|'') echo 'invalid media hash' >&2; exit 1;; esac
test "${#MEDIA_HASH}" -eq 64

sudo docker logs --since 30m community-node-cn-indexer-1 2>&1 \
  | grep -F "$MEDIA_HASH"
```

次の組を確認する。

1. `fetch local miss, trying remote peers`
2. `ephemeral fetch remote transfer completed`
3. 別provider処理または後続scanでも同じhashが再び `fetch local miss` になる

3はremote bytesがlocal blob storeへ追加されていない実機証跡になる。あわせてreadinessの
`permanent_blob_storage_disabled` と `media_fetch(success/unavailable/timeout/oversize)` を記録する。
一度のtransfer successだけで恒久非残留と判定しない。

### 6.3 非表出時の切り分け

- `configured_peer_count=0` / active peerなし: docs participantのpeer情報がmedia BlobServiceへ
  伝播しているか、running imageのrevision、peer refresh logを確認する。
- remote transfer成功後にquarantine: providerのlabel / scoreとrouter結果を確認する。
  clean classifier結果は `Completed` かつlabel/scoreなしであり、critical capabilityを持つことだけを
  検知根拠にしてはならない。
- 過去のscanで保存されたblob verdict rowは、再deploy後の現在のpost surfacingを単独では表さない。
  DBを手動修正せず、対象postの現在のindex truth、最新worker metrics、対象hashの最新logを組み合わせる。
- provider unavailable / timeout / oversize / scan error時は非表出が正しい。allowへ手動fallbackしない。

## 7. 検証用DNSを残す場合の運用境界

届出・公開判断前に検証用DNSを残す場合は、不特定の新規参加を許可したまま完了しない。
DNSを閉じない場合はadmissionを `invite` へ切り替える。既存active subscriberは継続利用できる。

```bash
cd /var/lib/kukuri/community-node
CLI_IMAGE="ghcr.io/<owner>/kukuri-cn-cli@sha256:<verified-digest>"

sudo docker run --rm --network community-node_default --env-file .env \
  "$CLI_IMAGE" admission show
sudo docker run --rm --network community-node_default --env-file .env \
  "$CLI_IMAGE" admission set-mode --mode invite
sudo docker run --rm --network community-node_default --env-file .env \
  "$CLI_IMAGE" admission show
```

期待値は `admission mode: invite`。`.env` の値や `docker compose config` を作業logへ出力しない。

## 8. rollback判断

次のいずれかなら、新imageでの調査を続ける前にprevious digestへ戻す。

- migration後にAPI / indexerがhealthyへ戻らない
- required providerが継続的に失敗しreadinessが閉じたまま
- truth/projection不一致が再投影待ち時間を超えて続く
- 検証対象のbenign contentが誤って除外される、または拒否対象が表出するregression
- startup再実行後も容量・証明書・networkの障害が解消しない

rollbackでもtagではなく、直前に記録した4つのdigestを使う。apply前backupを保持し、DB schemaを
戻す必要がある変更では専用のmigration rollback手順が無い限りDBを上書きしない。復旧後に
readiness、public surface、実投稿を再検証する。

## 9. 完了記録テンプレート

```text
main / PR:
Fast CI / image workflow:
digests (user-api / relay / CLI / indexer):
backup object / generation / size:
Terraform plan/apply/final plan:
startup desired/server SHA-256:
container / timer:
readiness:
truth / projection / relation:
live post ID / media hash / posted_at:
media local-miss -> ephemeral-success -> later local-miss:
admission / DNS boundary:
incident / recovery / deleted resources / recoverability:
rollback digests:
```

## 関連

- `docs/runbooks/community-node-gcp-terraform.md`
- `docs/runbooks/community-node-operator-docs.md`
- `docs/runbooks/openai-compatible-vlm.md`
- `docs/runbooks/project-arachnid-shield.md`
- `docs/runbooks/release.md`

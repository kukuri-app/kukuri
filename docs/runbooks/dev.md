# Development Runbook

Issueの起票からCloseまでのstage gate、固定surface inventory、状態遷移、独立監査は [Issue lifecycle runbook](./issue-lifecycle.md) に従う。本書は各validation commandの実行方法を規定する。

実行する検証の選定・重複path・重い検証の中断は [REFACTORING.md](../../REFACTORING.md) の「path別検証マトリクス」「検証の選定・中断」に従う。下の一覧は全変更で全commandを実行する指定ではない。

## 初回セットアップ
```bash
npx pnpm@10.16.1 install --dir apps/desktop
cargo xtask doctor
```

- `apps/desktop` の frontend toolchain は Vite 8 に合わせて Node `^20.19.0 || >=22.12.0` を前提にする

## 日常コマンド
```bash
cargo xtask check
cargo xtask test
cargo xtask desktop-ui-check
cargo xtask cn-check
cargo xtask cn-test
cargo xtask e2e-smoke
cargo xtask release-check v0.1.0-preview.1
cargo xtask scenario community_node_public_connectivity
cargo xtask scenario community_node_multi_device_connectivity
cargo xtask scenario desktop_smoke_bookmark_workflow
cargo xtask scenario desktop_smoke_game_room_persist
cargo xtask scenario desktop_smoke_metaverse_dome_persist
cargo xtask scenario desktop_smoke_metaverse_dome_connections
cargo xtask scenario desktop_smoke_metaverse_dome_entry
cargo xtask scenario desktop_smoke_metaverse_dome_transition
cargo xtask scenario desktop_smoke_live_session_persist
cargo xtask scenario pairwise_dm_offline_text_image_video_delivery_and_local_delete
cargo xtask scenario private_channel_invite_connectivity
cargo xtask rust-check
cargo xtask rust-test
cargo xtask app-api-slow-test
cargo xtask tauri-check
cargo xtask tauri-test
cargo xtask desktop-lint
cargo xtask desktop-test
cargo xtask desktop-storybook
cargo xtask desktop-browser-test
cargo xtask desktop-visual-test
```

`cargo xtask check` は non-CN Rust check、`apps/desktop/src-tauri` の Tauri backend compile、frontend lint/typecheck をまとめた日常 fast path。

`cargo xtask test` は non-CN Rust test と `apps/desktop` の Vitest をまとめた日常 regression path。

`cargo xtask desktop-ui-check` は `apps/desktop` の `lint`, `typecheck`, `test`, `storybook:build`, `test:e2e:browser`, `test:e2e:visual` をまとめて流す browser-aware frontend gate。`test:e2e:visual`(視覚回帰)は Windows などの非 CI 環境では `ignoreSnapshots` により比較が skip され、到達操作の smoke としてのみ流れる（下記「視覚回帰」を参照）。

- `cargo xtask rust-test` は `cargo-nextest` を優先して non-CN package を流し、`kukuri-harness` は serial 実行、doctest は `cargo test --doc` で補完する。local で `cargo-nextest` が無い場合だけ `cargo test` に fallback する。
- `cargo xtask tauri-check` は `CARGO_TARGET_DIR=target/desktop-tauri-check` を使って `apps/desktop/src-tauri` を warm cache 向けに compile する。
- `cargo xtask tauri-test` は `apps/desktop/src-tauri` の lib 単体 test を実行する（#1234）。この crate は root workspace の `exclude` に入っており、`cargo xtask rust-test` / `cargo xtask test` の対象にならない。target は `tauri-check` と同じ `target/desktop-tauri-check`。`-- <filter>` 以降は test binary へ渡す（例: `cargo xtask tauri-test -- tracing::tests`）。
  - CI では `Kukuri Linux Package` の `linux-appimage` job だけが `cargo xtask-lite tauri-test --package-build` で実行する。`--package-build` は `desktop-package` と同じ release profile / target で build し、package の成果物を再利用する。`Kukuri Fast` は `tauri-check`（compile のみ）で、lib test を実行しない。
  - Windows の test exe は Common Controls v6 の manifest を持たず、そのままでは `STATUS_ENTRYPOINT_NOT_FOUND`（`TaskDialogIndirect`）で起動に失敗する。`tauri-test` は Windows で test exe の隣に外部 manifest（`<exe>.manifest`）を書いてから実行する。Windows は manifest の解決結果を exe の path と更新時刻で cache するため、exe の更新時刻も更新する（manifest なしで一度起動した exe は、manifest を置くだけでは失敗し続ける）。`cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` を直接実行すると同じ失敗になるので、Windows では `tauri-test` を使う。製品 binary の manifest と `build.rs` は変えていない。
  - `cfg(windows)` の test（`commands/os_notification_windows.rs` など）は CI で実行されない。該当 file を変えたときは Windows ローカルで `cargo xtask tauri-test` を実行する。
- `cargo xtask desktop-lint` / `desktop-test` / `desktop-storybook` / `desktop-browser-test` / `desktop-visual-test` は targeted rerun 用。workflow とローカル rerun のどちらでも同じ entrypoint を使う。
- `cargo xtask cn-check` / `cargo xtask cn-test` は `cn-*` server slice の compile/test 用。
- `cargo xtask-lite <command>` は xtask を `harness` feature なしで build して実行する alias（`.cargo/config.toml`）。`e2e-smoke` / `scenario` 以外の command は `cargo xtask` と同じ動作で、xtask 自体の build が軽い。CI の harness を使わない job はこちらを使う（#1120）。
- `cargo xtask ci-prune-target` は `target/` と `apps/desktop/src-tauri/target/` から workspace crate の build 成果物だけを削除し、依存 crate の成果物は残す。CI の Cache Volume の容量対策として各 job の最後に実行する（#1120）。ローカルで実行すると workspace crate が次回再 compile される。
- `Kukuri Flake Probe`（`.github/workflows/kukuri-flake-probe.yml`、手動起動のみ）は、同じ suite を繰り返し実行して失敗率を測る。lane（`rust` / `vitest` / `playwright` / `all`）と 1 shard あたりの回数を指定し、2 shard を並行させる（既定は 2 × 10 = 20 回）。test の並列度を上げる変更の前後で使う（#1121）。実体は `scripts/ci/flake_probe.sh` で、失敗しても最後まで回し、失敗回数と各回の所要秒を step summary に出す。CI 本体とは別の Cache Volume tag（`kukuri-probe-*`）を使う。
- `cargo xtask cn-test` は `docker-compose.community-node.yml` の `cn-postgres` を自動起動し、`KUKURI_CN_RUN_INTEGRATION_TESTS=1` を付けて contract/integration test を流す。
- `cargo xtask scenario community_node_public_connectivity` も `cn-postgres` を自動起動し、in-process の `cn-user-api` / `cn-iroh-relay` を立てて 2 desktop scenario を流す。
- `cargo xtask scenario community_node_multi_device_connectivity` は same-author 2 desktop の endpoint-bound bootstrap で `post -> reply/thread -> reconnect` を確認する。
- `cargo xtask scenario desktop_smoke_metaverse_dome_persist`は固定Domeの作成、owner customization、規格外値の拒否、restart後のdocs + blob復元を確認する。
- `cargo xtask scenario desktop_smoke_metaverse_dome_move`はpublic topicのowner Domeをprivate channelへ移し、同一Preset customization、owner slot重複拒否、source非表示、restart後のtarget復元を確認する。move失敗時は同じmove idでretryする。target staging前の失敗では旧Domeが残り、完了後は旧Contextへ戻らない。
- `cargo xtask scenario desktop_smoke_metaverse_dome_connections`は共有Contextの3 ownerで2本のConnectionを成立させ、cycle拒否、restart後のtopology復元、revoke後のcomponent再分割を確認する。proposalが拒否された場合はslot占有、component merge / cycle、Instance generationを確認する。queue上限はoutbound 32、同一peer slot 4、receiver slot 32、local create 8件 / 10分。signed blockはConnectionを`owners_blocked`で失効し、unblockだけでは再接続しない。
- `cargo xtask scenario desktop_device_backup_restore`は使用中の1アカウントを1暗号化ファイルへ保存し、新しいapp data環境でのpreview／prepare／install／commit、core restore transactionの`AwaitingConsent`→`Activated`遷移、誤passphrase時の無変更、端末固有iroh endpoint secretの除外を確認する。Tauriの再同意画面、IPC gate、runtime／network副作用はTauri側のtestで別に確認する。
- `cargo xtask scenario desktop_smoke_metaverse_dome_entry`はowner-hosted Domeへのauthoritative entry、manifest default spawn、Dome境界に収まらないcolliderの決定的なsafe-spawn退避を確認する。Prop/avatar重複と全候補占有はhost unit testで固定する。入場拒否の調査ではraw inputやaccess proof本文を採取せず、Instance、lease epoch/session、denial codeだけを確認する。
- `cargo xtask scenario desktop_smoke_metaverse_dome_transition`は異なるownerの隣接Domeをowner deviceでhostし、送信元prepare、宛先reservation/commit、送信元completeの順でavatar-only handoffを行い、同時在室が残らないことを確認する。
- Metaverseは実験機能のため、`world_version = 1` / `2`の既存roomは再作成する。

### 検証を選ぶ入口

日常の製品変更は `cargo xtask check` + `cargo xtask test` を起点とし、変更pathと影響に応じた必須項目は [検証マトリクス](../../REFACTORING.md#path別検証マトリクス) で選ぶ。文書の誤字など区分Aは同節の対象確認を使う。UIの証跡は [ADR 0014](../adr/0014-uiux-dev-flow.md)、視覚baselineの更新方法は次節を参照する。

## ローカル先行検証

CI は課金対象の計算資源で動く。実装しながら CI へ push して確かめる進め方は費用が大きいので行わない。実装中の確認はローカルで済ませ、CI は PR を作った後の最終確認だけに使う。

- 変更 path に対応する validation は [検証マトリクス](../../REFACTORING.md#path別検証マトリクス) で選び、ローカルで実行してから commit する。
- workflow を変えるときは `actionlint <対象 file>` を実行する。runner label を増やす場合は `.github/actionlint.yaml` にも追加する。
- `docker/cn/**` や image の build 手順を変えるときは、ローカルの Docker で `docker buildx bake --file docker/cn/docker-bake.hcl --allow "fs.write=<出力先>"` を実行し、smoke まで通してから PR にする。
- CI 専用の設定（runner profile、Cache Volume、同時実行枠）を変えるときは、変更前後の計測値と根拠を Issue に記録する。
- ローカルで再現できない項目（実 runner の版差、Cache Volume の当たり外れ、registry への push）は、PR の run か merge 後の run で確認する。その項目を PR 本文の「検証」に明記する。
- 反復して失敗率を測るときは `Kukuri Flake Probe` を使い、通常の CI を繰り返し起動しない。

PR 作成後は `gh pr checks <番号>` で全 check の完了を待ち、pending が 0 件になってから merge する（`kukuri-fast.yml` 以外にも `xtask/**` や `.cargo/**` の変更で起動する workflow がある）。

## リファクタリング監査の発火要否（#873）

`cargo xtask refactoring-audit-check` は、監査済みbaseline以後にratchetの新規path登録または許容上限増加があるかを読み取り専用で判定する。判定結果のtrue／falseはともにexit 0で、破損baseline・commit不足・非祖先・git取得失敗はnonzero。既存 `oversized-files` のCIゲートとは別のコマンドである。

Python 3.13とschema validatorを準備する（Linuxでは必要に応じて `python` を `python3` に置き換え、venvを使用する）。判定自身は依存をinstallせず、GitHub APIも呼ばない。

```bash
python -m pip install -r scripts/refactoring-audit/requirements.txt
cargo xtask refactoring-audit-check
cargo xtask refactoring-audit-check --format json --now 2026-09-09T00:00:00Z
cargo xtask refactoring-audit-check --force-audit --reason "release安定化前の責務確認"
```

Python実行ファイルは `KUKURI_AUDIT_PYTHON` で指定できる。未指定時はWindowsで `python`、他OSで `python3`。`--repo` はfixture等のGit repository、`--current` は評価するcommit（既定HEAD）、`--now` はtimezone付き評価日時（既定UTC現在時刻）。同じ入力ではJSONとMarkdownは同一になる。入力は引数で渡し、shellのコードとして展開しない。

baselineは評価commit内の `xtask/refactoring-audit-baseline.json` から読み、その `baseline_commit` と評価commitの確定blobを比較する。未commit／未追跡の変更は含めない。schemaとcommit祖先関係、保存した集合／digest、ratchet上限、計測scope付きmetricを検証する。baselineを検証するために `audit_start_commit` までの履歴も必要となる。shallow cloneで履歴不足ならfetchして再実行し、比較起点を動かして回避しない。

発火は `ratchet_new_paths > 0 OR ratchet_increased_caps > 0` のみ。縮小・削除は発火せず、実測行数と許容上限を区別する。commit数・変更path数・hotspot・経過日は観測専用で自動閾値を持たない。変更path数は比較起点以後の履歴で一度でも変更された手書きpathの集合、hotspotはpathごとの変更commit数（mergeはfirst parentとの差分、rename追跡なし、binaryはpath変更として数える）。手書きpathの対象はbaselineのextension／除外設定に従う。

週次実行は `Kukuri Refactoring Audit`、毎週月曜00:00 UTC（日本時間09:00）。手動実行はmainを選び、必要なら `force_audit=true` と空白だけではない `reason` を指定する。forceでもbaseline異常を迂回しない。release／milestone／変更摩擦／互換経路のsunsetなど、人間判断の項目はsummaryと監査Issueに残す。

workflowは以下の軽量経路を使い、harnessと製品crateをbuildしない。標準xtaskではharness featureが既定有効で、既存scenario等の挙動は維持する。

```bash
cargo run --locked --quiet -p xtask --no-default-features -- refactoring-audit-check
```

false時はsummaryだけで終わり、GitHub Issueへのwriteは0件。true時だけ専用jobがOpen Issueの固定marker `kukuri-refactoring-audit:v1` を全ページ探索し、一件を作成または最新の機械管理領域だけ更新する。人間が書いたscope／AC／判断本文は更新対象にしない。全schedule／manual／rerunは共通concurrency groupで直列化するため、Issue write scriptを別経路で同時実行しない。

検索／API失敗は新規作成で回避しない。応答喪失時はActionsログとOpen Issueを確認してworkflowを再実行し、既存markerへ収束させる。同じmarkerのOpen Issueが複数、または管理領域が欠落／重複した場合はwriteせず失敗する。担当者が履歴・scopeを確認して正しい一件と管理領域を確定してから再実行する。既存IssueのClose／削除／scope拡張をworkflowに任せない。

baseline更新は監査完了後の独立review付きPRだけで行う。比較起点・監査日・採用signal・根拠・集合／metric・完了証拠を再計測して更新し、schema／意味検証の成功とtracking IssueのCompleteを確認する。baseline公開merge SHAと製品比較起点は分けて記録し、定期checkの成功だけでbaselineを前進させない。初期入力は [#872完了記録](../progress/2026-09-08-872-refactoring-campaign-phase-4.md)、実装と証跡は [#873作業記録](../progress/2026-09-08-873-refactoring-audit-trigger.md)。

変更時のtargeted validation（週次runでは実行しない）:

```bash
python -m unittest discover -s scripts/refactoring-audit -p 'test_*.py'
node --test scripts/refactoring-audit/upsert.test.mjs
cargo test --locked -p xtask --no-default-features
cargo clippy --locked -p xtask --all-targets --no-default-features -- -D warnings
actionlint .github/workflows/kukuri-refactoring-audit.yml .github/workflows/kukuri-refactoring-audit-test.yml
```

`KUKURI_AUDIT_XTASK` にbuild済みxtask実行ファイルの絶対pathを指定すると、PythonのCLI fixture testは実xtask経由で終了コードと出力を検証する。`Kukuri Refactoring Audit Tests` はこの経路と実repository baselineも検証する。基準値・workflowだけの変更でもこの専用test workflowが動く。

## 視覚回帰 (visual regression)

WP-H8（CSS 改名・整理）の安全網として、主要 14 サーフェスを Playwright `toHaveScreenshot` で撮って baseline と比較する（`apps/desktop/tests/playwright/visual.spec.ts`）。

- **baseline は Linux / Chromium 固定**。`apps/desktop/tests/playwright/__screenshots__/visual.spec.ts/*.png` に commit されている。フォントは非同梱（システムフォント依存）のため Windows 開発機と CI の pixel 一致は構造的に不可能。
- 日本語・中国語を欠落glyphで比較しないよう、比較jobとbaseline生成jobの両方へ `fonts-noto-cjk` をinstallする。ローカルLinuxで再生成する場合も同じfontを用意し、「□」になった画像を正しいbaselineとして採用しない。
- **ローカル（非 CI）は比較 skip**。`playwright.config.ts` の `ignoreSnapshots: !process.env.CI` により、Windows 等では `cargo xtask desktop-visual-test` / `desktop-ui-check` は到達操作の smoke としてのみ流れ、比較は行わない（従来どおり green）。
- **CI（`linux-desktop-browser`）が比較を強制**。CSS 変更で見た目が変わると視覚 step が赤くなる。
- 決定性のための固定: `timezoneId: 'UTC'`（絶対時刻表示が TZ 依存）、`animations: 'disabled'`、`maxDiffPixelRatio: 0.01`（AA 微差を吸収）。

### 視覚 step が赤くなったら
1. Actions の失敗ジョブから artifact `kukuri-desktop-visual-diff` をダウンロードし、`*-actual.png` / `*-diff.png` を baseline と目視比較する。
2. **意図しない差分（退行）** → CSS を修正して push。
3. **意図した差分** → baseline を更新（下記）。CSS 変更 PR に baseline 更新を同梱し、diff 画像を PR 本文に貼る。

### baseline の再生成（意図的な見た目変更・deps 更新時）
- **必ず Linux/Chromium で生成する。Windows ローカルでの `--update-snapshots` は禁止**（フォント差で CI と不一致になり、生成した baseline が即割れる）。
- 手順: GitHub Actions の **「Kukuri Visual Baseline」ワークフロー（`workflow_dispatch`）** を対象ブランチで実行 → 生成された artifact `kukuri-desktop-visual-baseline` をダウンロード → `apps/desktop/tests/playwright/__screenshots__/visual.spec.ts/` に上書き展開して commit。
  - CLI 例: `gh workflow run kukuri-visual-baseline.yml --ref <branch>` → 完了後 `gh run download <run-id> -n kukuri-desktop-visual-baseline -D <tmp>`。
  - artifact をダウンロードできない環境（egress 制限のある remote session 等）では、dispatch 時に input `commit_to_branch=true` を指定すると workflow が対象ブランチへ baseline を commit / push する。`GITHUB_TOKEN` による push は他の workflow を起動しないため、その後に別の commit を push して CI を流す。
- optional（ローカルで Linux baseline を再生成したい場合）: Playwright 公式 Docker イメージ `mcr.microsoft.com/playwright:v1.62.1-jammy`（`pnpm-lock.yaml` の `@playwright/test` バージョンと一致させる）内で `pnpm test:e2e:visual --update-snapshots` を実行する。
- `@playwright/test`（同梱 Chromium）更新や runner イメージ更新でフォント/AA が変わると baseline が一斉に割れることがある。その場合は deps 更新 PR に baseline 再生成を同梱する。baseline 生成（`kukuri-visual-baseline.yml`）と比較（`kukuri-fast.yml` の `linux-desktop-browser`）は同じ Namespace runner profile（`namespace-profile-kukuri-4v`、#1073 / #1117）で動かし、profile を変えるときは両方を同じ PR で変える。
- baseline の置き場は `apps/desktop/tests/playwright/__screenshots__/`（`.gitignore` 済みの `test-results/` とは別。混同しない）。

## community-node compose
```bash
docker compose --env-file .env.community-node -f docker-compose.community-node.yml run --rm cn-migrate
docker compose --env-file .env.community-node -f docker-compose.community-node.yml up --build cn-user-api cn-iroh-relay
```

- host port の既定値は `18080` (`cn-user-api`), `13340` (`cn-iroh-relay`), `15432` (`cn-postgres`), `16379` (`cn-valkey`)
- host 側 bind の既定値は loopback (`127.0.0.1`) なので、LAN/WireGuard 越しに公開する場合は `CN_*_HOST_BIND_IP` を上書きする
- compose 内の service 名は `cn-postgres`, `cn-migrate`, `cn-user-api`, `cn-iroh-relay`
- public URL を変える場合は `CN_BASE_URL`, `CN_PUBLIC_BASE_URL`, `COMMUNITY_NODE_CONNECTIVITY_URLS` を上書きする
- `cn-user-api` は `COMMUNITY_NODE_DATABASE_INIT_MODE=require_ready` で起動するので、`cn-migrate` または `cn-cli prepare` を先に流さないと fail-fast する

## community-node env 標準形
- `.env.community-node.example` をコピーして `.env.community-node` を作り、compose では `--env-file .env.community-node` を使う
- secret は `CN_POSTGRES_PASSWORD` と `COMMUNITY_NODE_JWT_SECRET` の 2 つを最低限上書きする
- `COMMUNITY_NODE_JWT_SECRET` は起動時に検証される。32 byte 未満、または `change-me` を含む placeholder のままだと `cn-user-api` は fail-fast する
- `COMMUNITY_NODE_JWT_SECRET` を rotate すると既存 bearer token は即時無効化される
- `cn-user-api` の HTTP レート制限は `COMMUNITY_NODE_RATE_LIMIT_ENABLED`(既定 example では true)、`..._PER_SECOND` / `..._BURST` / `..._TRUST_FORWARDED_FOR` で調整する。Caddy などの信頼できる reverse proxy 配下でのみ `..._TRUST_FORWARDED_FOR=true` にしてクライアント単位で制限する（直公開時は X-Forwarded-For 詐称を避けるため false）
- `COMMUNITY_NODE_DATABASE_URL` は compose 内では `cn-postgres` 向けに組み立てる。外部 Postgres を使う場合だけ個別に差し替える

## community-node 公開 manual smoke
公開 URL を current community-node 構成で出す場合は、`WireGuard + Caddy` の VPS edge を用いる。詳細な self-host 手順は `docs/runbooks/community-node-self-host-vps.md` を参照する。

VPS 側:

- operator が所有する `api.example.com` と `iroh-relay.example.com`（実 domain へ置換）を VPS の public IP へ向ける
- Caddy は `api.example.com -> http://192.0.2.2:18080`, `iroh-relay.example.com -> http://192.0.2.2:13340` を reverse proxy する
- nftables は `7842/udp` を Home 側 `192.0.2.2:7842` へ WireGuard 経由で forward する

Home 側の `.env.community-node` には最低限この値を入れる。

```dotenv
CN_BASE_URL=https://api.example.com
CN_PUBLIC_BASE_URL=https://api.example.com
COMMUNITY_NODE_CONNECTIVITY_URLS=https://iroh-relay.example.com

CN_USER_API_HOST_BIND_IP=192.0.2.2
CN_IROH_RELAY_HTTP_HOST_BIND_IP=192.0.2.2
CN_IROH_RELAY_QUIC_BIND_ADDR=0.0.0.0:7842
CN_IROH_RELAY_QUIC_HOST_BIND_IP=192.0.2.2
CN_IROH_RELAY_QUIC_PORT=7842
CN_IROH_RELAY_TLS_CERT_PATH=/certs/default.crt
CN_IROH_RELAY_TLS_KEY_PATH=/certs/default.key
CN_IROH_RELAY_CERTS_HOST_PATH=./docker/cn/certs
```

- `api.example.com`（実 domain）は `cn-user-api` を向ける
- `iroh-relay.example.com`（実 domain）は `cn-iroh-relay` の HTTP/TCP と QUIC/UDP を向ける
- desktop は `connectivity_urls` を server から受け取るので、websocket relay 前提は使わない
- `docker/cn/certs/` には relay の実 domain 用の公開証明書と秘密鍵を `default.crt` / `default.key` として置く
- Postgres と Valkey は Home 側 private bind のままにし、VPS や public internet へ公開しない

起動:

```bash
docker compose --env-file .env.community-node -f docker-compose.community-node.yml run --rm cn-migrate
docker compose --env-file .env.community-node -f docker-compose.community-node.yml up -d --build cn-user-api cn-iroh-relay
```

公開確認:

```bash
curl -fsS https://api.example.com/healthz
curl -fsS https://iroh-relay.example.com/ping
```

期待値:

- `connectivity_urls` は operator が設定した relay URL（例: `https://iroh-relay.example.com`）
- desktop client 側は `Save Nodes -> Authenticate -> Accept` の順で進め、その session のまま relay-assisted path を使える
- 公開 community-node path では `Peer Ticket` import は不要
- `Authenticate` 直後の `connectivity urls: pending consent acceptance` は正常で、`Accept` 後に resolved される
- `Accept` 後に `restart required: no` のまま relay-assisted sync へ移るのが正常で、`yes` が出たら regression とみなす
- discovery diagnostics では `Community Bootstrap Peers` が community-node 由来、`Configured Seed IDs` が local seed 設定、`Manual Ticket Peers` が手動 import を表す
- Linux 実機の公開 manual smoke では `Save Nodes -> Authenticate -> Accept -> post -> reply/thread -> blob sync` まで restart なしで成功を確認済み
- relay-only public path でも `Sync Status` / `Tracked Topics` diagnostics は relay-assisted docs/blob peer を含めて `connected` と `peer_count` を出す

### cn-iroh-relay backpressure guardrail
- `iroh-relay 1.0.0` の per-client send queue depth は upstream で `512` 固定。`cn-iroh-relay` から queue depth は変更できない。
- `iroh_relay::server::client: failed to handle send packet frame: failed to forward packet: Full` が 3 client 同時起動などで増える場合、まず client 側の topic warmup throttling / coalescing が入った current build で確認する。
- noisy client の ingress を運用上絞る必要がある場合だけ、`.env.community-node` に `COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_BYTES_PER_SECOND` と任意の `COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_MAX_BURST_BYTES` を設定する。未指定時は従来どおり unlimited。

## cn-cli と cn-operator の役割

- `cn-cli` は稼働中 community node の Postgres / ArcadeDB へ接続し、migration、auth rollout、通報、入会制御、supported set、indexing request、relation 解析を運用する。node の状態を読み書きするため、対象環境の `COMMUNITY_NODE_DATABASE_URL` が必要になる。
- `cn-operator` は `operator-config.yaml` を入力に、設定検証、開示文書、manifest、Terraform 変数、safety readiness を生成・検証する。通常の DB 運用 command は持たず、稼働中 node の状態を直接変更しない。
- 「現在の node 状態を操作する」場合は `cn-cli`、「deploy 前の宣言・生成・readiness を扱う」場合は `cn-operator` を使う。両者は相互代替ではない。

`cn-operator` の詳細は [`community-node-operator-docs.md`](community-node-operator-docs.md) を参照する。

## community-node deploy 順序
```bash
cargo run -p kukuri-cn-cli -- --database-url "$COMMUNITY_NODE_DATABASE_URL" prepare
cargo run -p kukuri-cn-cli -- --database-url "$COMMUNITY_NODE_DATABASE_URL" set-auth-rollout --mode off
cargo run -p kukuri-cn-user-api
cargo run -p kukuri-cn-iroh-relay
# index/moderation/trust/relation を使う構成では、provider・署名鍵・ArcadeDB・relay を設定して起動
cargo run -p kukuri-cn-indexer
# 全 readiness が Pass のときだけ、現在の config/deployment revision に結び付いた activation を記録
cargo run -p kukuri-cn-cli -- readiness --profile public-node --config operator-config.yaml
```

1. migration/seed は `cn-cli prepare` だけで行う
2. `cn-user-api` は prepared DB を前提に起動する
3. index/moderation stack は `cn-indexer` と relation timer を起動し、`cn-cli readiness` の全項目合格後にのみ read surface を解禁する
4. rollout 変更は deploy 後に `cn-cli set-auth-rollout` で行う
5. `COMMUNITY_NODE_DATABASE_INIT_MODE=prepare` は local bring-up と test 用に限定し、常用しない

compose を使う場合:
```bash
docker compose --env-file .env.community-node -f docker-compose.community-node.yml run --rm cn-migrate
# production provider + signing keyを設定した構成のpositive startup smoke
docker compose --env-file .env.community-node -f docker-compose.community-node.yml run --rm cn-indexer validate-config
docker compose --env-file .env.community-node -f docker-compose.community-node.yml up -d
```

## community-node backup / restore
backup:
```bash
docker compose --env-file .env.community-node -f docker-compose.community-node.yml exec -T cn-postgres \
  sh -lc 'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc' > cn-postgres.dump
```

restore:
```bash
cat cn-postgres.dump | docker compose --env-file .env.community-node -f docker-compose.community-node.yml exec -T cn-postgres \
  sh -lc 'dropdb --if-exists -U "$POSTGRES_USER" "$POSTGRES_DB" && createdb -U "$POSTGRES_USER" "$POSTGRES_DB" && pg_restore --clean --if-exists --no-owner -U "$POSTGRES_USER" -d "$POSTGRES_DB"'
```

- backup は schema + data をまとめて保持する `pg_dump -Fc` を標準にする
- restore 前に `cn-user-api` を停止して、Postgres への新規接続を止める
- restore 後に追加 migration がある場合だけ `cn-cli prepare` を流す
- `cn-postgres-data` volume を直接コピーして backup 代わりにしない

## community-node 検証
```bash
cargo xtask cn-test
cargo xtask scenario community_node_public_connectivity
cargo xtask scenario community_node_multi_device_connectivity
```

- `cn-test` は `/v1/auth/challenge`, `/v1/auth/verify`, `/v1/consents/status`, `/v1/consents`, `/v1/bootstrap/nodes` の contract を確認する。
- `community_node_public_connectivity` scenario は `config -> auth -> consent -> post -> reply/thread -> live -> game -> reconnect` を 1 community-node stack + 2 desktops で確認する。
- `community_node_multi_device_connectivity` scenario は same-author 2 desktop の `auth -> consent -> post -> reply/thread -> reconnect` を確認する。
- crate test を直接叩く場合は `KUKURI_CN_RUN_INTEGRATION_TESTS=1` と `COMMUNITY_NODE_DATABASE_URL` を明示する。
- 公開 community-node の手動確認では UI の peer source と peer count を見つつ、timeline / thread / attachment preview / blob media payload fetch の成否まで確認する。

## frontend だけ確認する場合
```bash
cd apps/desktop
npx pnpm@10.16.1 dev
npx pnpm@10.16.1 test
npx pnpm@10.16.1 storybook:build
npx pnpm@10.16.1 test:e2e:browser
npx pnpm@10.16.1 tauri:dev
```

- `pnpm tauri dev` / `pnpm tauri:dev` は loopback の空き port を自動選択し、5173 が使用中なら次の空き port へ退避する。
- desktop shell UI の primary route は hash-based (`#/timeline`, `#/channels`, `#/live`, `#/game`, `#/messages`, `#/profile`, `#/notifications`) に固定されている。settings / context deep-link も hash search param で復元する。
- desktop の Tauri backend は `mainline::rpc::socket`, `noq_proto::connection`, `iroh::socket::remote_map::remote_state`, `iroh_docs::engine::live`, `iroh_gossip::net` を既定で `error` へ落としている。community-node connectivity assist / DHT / docs sync の内部 warning を調べたいときだけ `RUST_LOG=warn,mainline::rpc::socket=warn,noq_proto::connection=warn,iroh::socket::remote_map::remote_state=warn,iroh_docs::engine::live=warn,iroh_gossip::net=warn` を明示する。

## Windows 前提
- Windows prerequisites は Tauri 公式手順を使う: <https://v2.tauri.app/start/prerequisites/#windows>
- 初回 Windows cut の対象は `x86_64-pc-windows-msvc` のみ
- installer build は current-user NSIS + WebView2 download bootstrapper を前提にする

## Windows packaging
```powershell
cargo xtask desktop-package
```

- Windows hostではNSISを生成する。Linux x86_64 hostではAppImageとDebを同一buildで生成する（次節）。
- 生成物は `apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/` に出る
- `cargo xtask desktop-package` は `src-tauri/tauri.windows.conf.json` を使った Windows bundle config を前提にする
- release workflow / updater manifest / draft release の手順は `docs/runbooks/release.md` を参照する

## Linux AppImage／Deb生成

Debは`bundle/deb/`へ生成され、同じ公開鍵で`.deb.sig`を検証する。`scripts/release/deb_package.py`がmetadata／ELF／desktop entry／icon／依存／noticeを実payloadから検査する。package CIは使い捨てrunnerで依存解決・install／reinstall／remove・データsentinel保持を確認する。GUI更新・権限承認は[Deb手順](linux-deb.md)と[#905作業記録](../progress/2026-09-07-issue-905-linux-deb-updater.md)を参照する。

Ubuntu 22.04のx86_64 hostで `cargo xtask desktop-package` を実行する。Linux設定は `apps/desktop/src-tauri/tauri.linux.conf.json`、署名はTauri updater署名のみとする。鍵なしのLinux生成は拒否し、生成後も設定公開鍵との署名一致を検査する。

生成・実機準備・検証用署名の区別は [Linux AppImage手順](linux-appimage-smoke.md)、確認済み範囲は [#889作業記録](../progress/2026-09-05-issue-889-linux-appimage.md) に記載する。生成成功だけでは実機動作・更新成功・公開済みとは扱わない。Release全体のLinux統合は#890が担当する。

#889のScope revision v5では、成功済みの代表実機証跡と変更影響に絞った自動検証を採用する。全OS連携の追加手動確認やComputer Useは必須にせず、自動検証では判定できない具体的な問題だけを最小限の手動補完へ回す。検証方法を変えても署名・identity・データ保護、必須CI・独立監査は維持する。

## remote-sync 用の環境変数
```bash
export KUKURI_BIND_ADDR=0.0.0.0:0
export KUKURI_ADVERTISE_HOST=<LANで到達可能なIPまたはホスト名>
export KUKURI_ADVERTISE_PORT=<必要なら固定port>
export KUKURI_INSTANCE=<同一マシンで複数起動する場合の識別子>
export KUKURI_DISABLE_KEYRING=1
export KUKURI_DISCOVERY_MODE=<static_peer|seeded_dht>
export KUKURI_DISCOVERY_SEEDS=<node_id または node_id@host:port をカンマ区切り>
```

- `KUKURI_ADVERTISE_HOST` を設定すると `Your Ticket` はその host を使う。
- `KUKURI_INSTANCE` を設定すると app data dir が分離される。
- `KUKURI_APP_DATA_DIR` を設定すると app data dir を丸ごと上書きできる。
- 既定の app data dir は build の種別で分かれる（#1105）。配布版（release build）は OS の app data dir（Windows は `%APPDATA%\app.kukuri.desktop`）、`tauri:dev` や `cargo build` の開発ビルド（debug build）はその兄弟の `app.kukuri.desktop.dev` を使う。開発ビルドは配布版の同意記録・アカウント・DB・OS 通知設定を読み書きしない。`KUKURI_INSTANCE` はこの build 別の dir の下に作られ、`KUKURI_APP_DATA_DIR` は build の種別に関係なく指定した dir をそのまま使う。
- 開発ビルドの `KUKURI_APP_DATA_DIR` に配布版の dir を指定しない。同意判定は build の種別を見ないため、開発中の版への同意が配布版の同意として扱われる（記録の `build_profile` は `development` になる）。
- #1105 より前の開発ビルドが配布版の dir に残した同意記録やアカウントは移動・削除しない。記録には `build_profile` が無く、配布版の記録と区別できない。開発ビルドの `.dev` dir は初回起動時に空の状態から始まる。
- WebView の保存領域（theme / 言語などの localStorage）は identifier 単位のため、配布版と開発ビルドで共有される。
- `KUKURI_DISABLE_KEYRING=1` を設定すると OS keyring を使わず、app data dir 内の `*.identity-key` fallback file を使う。
- `KUKURI_DISCOVERY_MODE` / `KUKURI_DISCOVERY_SEEDS` を設定すると discovery panel は read-only になり、env が local file より優先される。

## Linux client daemon

配布archiveとforeground起動・状態/schema取得の例は[Linux CLI手順](linux-cli.md)を参照する。Release CIのarch smokeはその同じcommandを一時profileで実行し、別の人手walkthroughを重複させない。

開発buildでは、CLI専用profileを選んで同意を記録した後、foregroundの常駐プロセスを起動する。

```bash
cargo run -p kukuri-cli -- --profile dev consent status
cargo run -p kukuri-cli -- --profile dev consent accept --accept-documents --age-confirmed --language ja
cargo run -p kukuri-cli -- --profile dev daemon run
```

- profileは`$XDG_DATA_HOME/kukuri/cli/profiles/<profile>`、未設定時は`$HOME/.local/share/kukuri/cli/profiles/<profile>`へ置く。`--profile`と`KUKURI_INSTANCE`が異なる場合、またはprofile selectorと`KUKURI_APP_DATA_DIR`を併用した場合は起動しない。
- 同一profileを別processが所有している場合は`profile_in_use`で終了する。local socketはcanonicalなprofile directoryのdigestから導出した`$XDG_RUNTIME_DIR/kukuri/profile-<digest>.sock`を使い、異なるprofile directory間で名前空間を共有しない。`XDG_RUNTIME_DIR`が無い環境では起動しない。
- `daemon start`、`daemon stop`、`daemon status`は`packaging/linux/systemd/kukuri@.service`をuser unitとして配置した環境で`systemctl --user`を操作する。これらはnamed profile専用で、`KUKURI_APP_DATA_DIR`を指定した実行では使用できない。
- Secret Serviceを利用できない画面なし環境では、新規profileの起動前に`KUKURI_DISABLE_KEYRING=1`を設定する。identityはprofile内の0600 fileへ保存され、以後も同じ設定で起動する。既にkeyringへ保存済みのidentityへこの設定を後付けした場合は、別identityを生成せず型付きstartup errorで停止する。
- 同意が未成立の場合、常駐プロセスはlocal socketだけを開き、account identity、network runtime、scheduler、observerを開始しない。`consent accept`はprofile leaseを取得するため、常駐プロセスを停止してから実行する。

起動中のdaemonには `call get_app_consent_status` で現在の文書versionを確認し、`call accept_app_consents --input -` へ `documents`（`slug`／`version`）、`language`、`age_attested: true` をJSONで明示入力できる。復元後もこの経路で再同意する。

```bash
cargo run -p kukuri-cli -- --profile dev call protocol.commands
cargo run -p kukuri-cli -- --profile dev call protocol.schema
cargo run -p kukuri-cli -- --profile dev call get_desktop_startup_status
cargo run -p kukuri-cli -- --profile dev call list_timeline --input -
cargo test -p kukuri-cli --test command_parity
cargo test -p kukuri-cli --test process_e2e
```

- `call` の通常入力はJSON object。`protocol.commands` の既定ページは100件で、`next_cursor` を次の入力の `cursor` に渡す。対象操作のschema、変更種別、秘密情報の入出力要否を登録簿で確認する。
- `--input -` はstdinのJSON、`--input <path>` はowner-only fileを読む。DM本文は通常JSONで返す。画像・動画・Dome assetは入力のfile参照とhashで固定し、通常JSONへbase64本文を入れない。
- 鍵export用passphrase、backup passphrase、招待tokenは `--secret-input <path>` 等の専用入力を使う。鍵・招待情報の出力先は `--secret-output <新規path>` 等で事前宣言する。既存fileは上書きしない。改行を含め、秘密入力のbytesをそのまま扱うため、passphrase fileへ不要な改行を追加しない。
- 鍵importはsecret frameに `export`／`passphrase` のJSONを渡す。通常payloadは任意の `label` だけ。招待exportは既存仕様で鍵世代を更新するため、変更操作として実行される。
- timeout・切断時にCLIは要求を再送しない。変更の成否が不明な場合は返されたIDや現在の状態を確認する。別入力をCLIが統合することはなく、既存domainの一意性規則は維持する。
- `process_e2e` はLinux限定で、実daemon・実CLI・一時profileを使う。Community NodeのHTTP応答はテスト用mockであり、実node接続のscenarioとは区別する。

PowerShell 例:
```powershell
$env:KUKURI_BIND_ADDR="0.0.0.0:0"
$env:KUKURI_ADVERTISE_HOST="<LANで到達可能なIPまたはホスト名>"
$env:KUKURI_INSTANCE="desktop-a"
```

## Linux / Windows 共通の回帰用手動確認
1. 各端末で `KUKURI_BIND_ADDR=0.0.0.0:0` と `KUKURI_ADVERTISE_HOST` を設定する。
2. 同一マシンで複数起動する場合は `KUKURI_INSTANCE` も別値にする。
3. `npx pnpm@10.16.1 tauri:dev` を起動する。
4. 両方の `Your Ticket` を相互に `Peer Ticket` へ貼って import する。
5. 片方で post し、もう片方の timeline に反映されることを確認する。
6. 片方を再起動しても timeline が維持されることを確認する。
7. どちらかの client を終了し、相手側が polling で `connected: no, peers: 0` に戻ることを確認する。
8. timeline または thread pane の `Reply` ボタンから返信し、相手側の thread に反映されることを確認する。
9. `Add Topic` で 2 つ以上の topic を登録し、切り替えながら各 timeline が維持されることを確認する。
10. peer 接続中に複数 topic へ post し、相手側で各 topic の timeline に反映されることを確認する。
11. tracked topic 一覧の各 topic について `joined / peers / expected / missing / last_received_at / status_detail` が妥当な値になることを確認する。
12. 共通購読 topic を片側で解除し、その topic 行だけ `joined: false / peers: 0` になることを確認する。
13. invalid な `Peer Ticket` を import したときに global `Last Error` が更新されることを確認する。
14. client 再起動後に新規 post を作成し、restart 前後で author identity が変わらないことを確認する。
15. live session を `create -> join -> end` し、viewer count と ended state が相手側に反映されることを確認する。
16. game room を `create -> update score/status` し、相手側に score card が反映されることを確認する。

## Social graph manual verification

`friend of friend` まで見る場合は 3 author 構成を使う。`mutual` と restart 復元だけなら 2 desktop でもよい。

事前に流す自動テスト:

```bash
cargo test -p kukuri-store store_profile_upsert_latest_wins -- --nocapture
cargo test -p kukuri-store author_relationship_projection_rebuild_roundtrip -- --nocapture
cargo test -p kukuri-app-api social_graph_derives_friend_of_friend_and_clears_after_unfollow -- --nocapture
cargo test -p kukuri-desktop-runtime friend_only_channel_restore_keeps_archived_epoch_history -- --nocapture
cd apps/desktop && npx pnpm@10.16.1 test
```

操作手順:

1. 2-3 desktop を起動する。最小 lane は static-peer ticket import で、`friend of friend` まで見るなら A/B/C の 3 author を使う。
2. 同じ public topic を開き、author detail が開ける状態まで接続させる。
3. A で profile を更新し、A の表示名や profile 情報が B 側へ hydrate されることを確認する。
4. A -> B と B -> A で follow し、両端末の author detail に `mutual` が出ることを確認する。
5. `friend of friend` を見る場合は B -> C だけを follow し、A から見た C に `friend of friend` が出ることを確認する。
6. A で B を unfollow するか、`friend of friend` lane では A と B の link を外し、対応する relationship 表示が消えることを確認する。
7. 両端末を再起動し、profile 表示、follow 状態、必要なら `mutual` / `friend of friend` の表示が復元されることを確認する。

## Private channel manual verification

最小 lane は static-peer ticket import。manual verification は `invite_only`, `friend_only`, `friend_plus` の audience ごとに流す。

事前に流す自動テスト:

```bash
cargo test -p kukuri-docs-sync private_replica_requires_registered_capability -- --nocapture
cargo test -p kukuri-app-api private_channel_invite_scopes_posts_and_replies -- --nocapture
cargo test -p kukuri-app-api friend_only_grant_requires_mutual_and_rotate_requires_fresh_grant -- --nocapture
cargo test -p kukuri-app-api friend_plus_share_freeze_rotate_and_new_epoch_visibility -- --nocapture
cargo test -p kukuri-desktop-runtime private_channel_invite_restores_after_restart_without_reimport -- --nocapture
cargo test -p kukuri-desktop-runtime friend_only_channel_restore_keeps_archived_epoch_history -- --nocapture
cargo test -p kukuri-desktop-runtime friend_plus_channel_restore_redeems_rotation_after_restart -- --nocapture
cargo test -p kukuri-harness private_channel_invite_connectivity -- --nocapture
cargo test -p kukuri-harness friend_only_rotate_requires_fresh_grant -- --nocapture
cargo test -p kukuri-harness friend_plus_share_freeze_rotate_connectivity -- --nocapture
cd apps/desktop && npx pnpm@10.16.1 test
```

### `invite_only` lane

必ず `Create Channel -> Create Invite -> Join via Invite` の順で行う。

操作手順:

1. 2 desktop を起動する。最小構成は `Peer Ticket` の相互 import。可能なら 3 台目の未招待端末 C も起動する。
2. 両端末で同じ topic を開き、public timeline が同期することを確認する。
3. 端末 A で `Create Channel` を押し、label を入力して private channel を明示的に作成する。
4. 端末 A で `View Scope` と `Compose Target` が新しい channel に切り替わったことを確認する。
5. 端末 A で `Create Invite` を押し、invite token が表示されることを確認する。
6. 端末 B で `Join via Invite` に token を貼り付けて import し、topic が tracked state に入り、対象 channel が選択されることを確認する。
7. 端末 A でその private channel に post し、端末 B の `All joined` または当該 channel view にだけ表示され、`Public` には出ないことを確認する。
8. 端末 B でその private post に reply し、thread が同じ private channel 内でのみ見えることを確認する。
9. 端末 A でその private channel 上に live session を作成し、端末 B が `join -> leave -> end` を追従できることを確認する。
10. 端末 A でその private channel 上に game room を作成し、score / status 更新が端末 B に反映されることを確認する。
11. 両端末を再起動し、invite 再入力なしで joined private channel が復元され、private post / thread / live / game が再表示されることを確認する。
12. 端末 B でも `Create Invite` が可能で、fresh invite を再発行できることを確認する。
13. 3 台目の未招待端末 C を使う場合は、同じ topic の `Public` / `All joined` から private channel content が見えないことを確認する。

### `friend_only` lane

1. 最低 3 desktop を起動する。A を owner、B を joiner、D を fresh grant 確認用に使う。非 mutual reject を見るなら C も用意する。
2. A と B、および A と D で相互 follow を成立させる。C を使う場合は A と mutual にしない。
3. A で `Audience: Friends` を選んで `Create Channel` し、`Create Grant` で token を発行する。
4. B で `Join Grant` を行い、private channel が選択され、`Policy: Friends` が表示されることを確認する。
5. A で private post を作り、B にだけ表示され、`Public` に漏れないことを確認する。
6. A または B の follow を外して mutual を崩し、owner 側に `rotation required` が出ることを確認する。
7. rotate 前に発行した古い grant を保持したまま、A で `Rotate` を実行する。
8. 古い grant での join が失敗し、新しく発行した grant では join できることを確認する。
9. rotate 後に join した newcomer は old epoch の content を読めず、新 epoch の content だけ読めることを確認する。

### `friend_plus` lane

1. 4 desktop を起動する。A を owner、B/C を既存 participant、D を stale share / fresh share 確認用に使う。
2. A-B、B-C、B-D の pairwise mutual を成立させる。
3. A で `Audience: Friends+` を選んで `Create Channel` し、A -> B share、B -> C share の順で join させる。
4. C 側に `joined via <B>` が表示され、A/B/C 間で private post が同期し、`Public` に漏れないことを確認する。
5. B -> D の share を発行したまま未 import にしておき、A で `Freeze` を実行する。
6. freeze 後も既存 participant は write を継続できる一方、D の stale share import は失敗することを確認する。
7. A で `Rotate` を実行し、B/C が restart なしまたは restart 後復元で新 epoch へ移行することを確認する。
8. old share は rotate 後も失敗し、B が発行した fresh share では D が join できることを確認する。
9. rotate 後に join した D は old epoch content を読めず、新 epoch content だけ読めることを確認する。

自動テスト対応:

- `private_replica_requires_registered_capability`: capability 未登録では private replica を開けない
- `private_channel_invite_scopes_posts_and_replies`: invite import 後の private post / reply 継承を確認する
- `friend_only_grant_requires_mutual_and_rotate_requires_fresh_grant`: mutual 条件、rotation required、fresh grant 必須を確認する
- `friend_plus_share_freeze_rotate_and_new_epoch_visibility`: mutual-chain share、freeze、rotate、new epoch visibility を確認する
- `private_channel_invite_restores_after_restart_without_reimport`: desktop-runtime で restart 後の capability 復元を確認する
- `friend_only_channel_restore_keeps_archived_epoch_history`: friend-only の archived/current epoch 復元を確認する
- `friend_plus_channel_restore_redeems_rotation_after_restart`: friend-plus の encrypted rotation grant redeem を確認する
- `private_channel_invite_connectivity`: 3 client static-peer scenario で invite-only の private post / reply / live / game / restart を通す
- `friend_only_rotate_requires_fresh_grant`: static-peer runtime で friend-only rotate と stale grant block を通す
- `friend_plus_share_freeze_rotate_connectivity`: 4 client static-peer runtime で share chain / freeze / rotate / stale share block を通す
- `apps/desktop/src/App.test.tsx`: `Audience`, `Create Grant`, `Create Share`, `Freeze`, `Rotate`, invite/grant/share 導線の表示を確認する

## Seeded DHT 手動確認
1. 2 instance とも `KUKURI_DISCOVERY_MODE=seeded_dht` を使うか、desktop の discovery panel で seed を保存できる状態にする。
2. 両方を起動し、`Local Endpoint ID` を相互に `Seed Peers` へ登録する。`node_id` だけで通ることを確認する。
3. `Save Seeds` 後に `Stored Seed IDs` と `Connected / Discovered` が埋まることを確認する。
4. `Peer Ticket` import を使わずに `post -> reply/thread -> live/game` が相互に伝播することを確認する。
5. 片側を再起動し、seed 再入力なしで再接続と timeline backfill が成立することを確認する。
6. `Seed Peers` に invalid な entry を入れて保存し、apply 全体が失敗して既存 seed が保持されることを確認する。

- `seeded_dht` は `direct_only` 前提なので、port または advertise address を変えた場合は新しい到達先が peer 間で到達可能であることを確認する。
- `node_id@host:port` は addr_hint 付き接続を含む。DHT 自体の確認は `node_id` のみで行う。

## Windows native smoke
### 無操作時のCN維持・接続復旧を調べる（#1176）

- CNのnode別session、観測送信、self-healは独立laneで実行される。応答待ちのCNがある場合は、`kukuri_connectivity`の失敗段階・retryと、正常CNのheartbeat/rendezvous期限を分けて確認する。共通HTTP clientの上限は接続5秒・本文込み10秒。上限を長くするだけで回復したと判定しない。
- `kukuri_connectivity=info`は既定filterに含まれる。scheduler開始/停止、session ready/失敗、local docs actor不通、stack再構築の世代/commitを記録する。明示的な`RUST_LOG`を使う場合は必要に応じこのtargetを追加する。token・鍵・本文をログへ追加しない。
- `sending to iroh_docs actor failed`はCNのHTTP失敗とは別層。正規のstack切替中か、再構築失敗後かを世代とcommit記録で確認する。remote peerがofflineなだけならlocal actorは維持される。local probeのtimeoutはactor破損の証拠にしない。
- dedicated profileで表示/非表示のみ/画面ロック/suspendを区別し、開始・格納・復帰時刻、版・OS/WebView、CN期限、peer数、実投稿/返信/blobの到達を記録する。既定bufferは2000件/1MiBで過去が落ちるので、調査中は明示的に標準出力を保存する。製品が常時ログファイルを書き出す機能ではない。
- 回帰testは先に原因を特定し、channel制御・一回のfuture poll・仮想時間で確認する。CIへ長時間の放置・実時間sleepを追加しない。`cargo test --locked -p kukuri-desktop-runtime idle_`、`cargo test --locked -p kukuri-desktop-runtime stalled_`が本件の最小確認。実peerの接続確認には既存のconnectivity scenarioを使う。

### 一般的なWindows動作確認

1. native Windows host で `cargo xtask doctor`、`cargo xtask check`、`cargo xtask test` を通す。
2. `cd apps/desktop && npx pnpm@10.16.1 tauri:dev` を起動し、`post -> restart -> persist` と author pubkey 不変を確認する。
3. `KUKURI_DISABLE_KEYRING` を外した状態でも author pubkey が維持されることを確認する。
4. `KUKURI_INSTANCE` を分けた 2 instance で static-peer ticket import、`reply/thread`、live/game の伝播を確認する。
5. 片側終了時に相手側が polling で `connected: no, peers: 0` に戻ることを確認する。
6. 複数 topic の維持、topic 単位の unsubscribe、invalid ticket import 時の `Last Error` 更新を確認する。
7. 可能なら別 host 間でも `KUKURI_ADVERTISE_HOST` を使った static-peer 接続を確認する。
8. `cargo xtask desktop-package` で NSIS installer を build し、install 後に packaged app が通常の app data dir を使って起動することを確認する。

実機確認済み:
- Linux 実機 2 台で固定 port / 相互 ticket import による static-peer 接続が成立
- Linux 実機 2 台で `post -> reply -> thread` と複数 topic の双方向伝播が成立
- Linux 実機 2 台で topic 単位の unsubscribe と peer diagnostics 表示が期待どおりに機能
- Linux 実機で `片側だけ購読 -> 0`, `後から参加 -> 1`, `再び片側だけ -> 0` が topic peer diagnostics に反映され続けることを確認
- Linux 実機で global の `Connection Detail / Last Error` と topic ごとの `status_detail / error:` 表示が期待どおりに機能
- Linux 実機で client 再起動後も author pubkey が変わらず、author identity が維持されることを確認
- Linux 実機 2 台で `seeded_dht` + 相互 `node_id` seed 設定だけで、ticket import なしの接続、再接続、投稿伝播が成立
- Linux 実機 2 台で `seeded_dht` + 相互 `node_id` seed 設定だけで、`reply/thread` と live/game の伝播、および restart 後 reconnect without reimport が成立
- Linux 実機 2 台で片側 port 変更後も、新 port が到達可能なら seed 再入力なしで再接続、投稿伝播、reply が成立
- Linux 実機 2 台で `seeded_dht` の invalid seed 保存が reject され、既存 seed が維持されることを確認
- Linux 実機 2 台で relay-only community-node (`https://api.kukuri.app`, `https://iroh-relay.kukuri.app`) に対し、ticket import なしの peer 間接続、`post -> reply/thread -> live -> game` 伝播が成立
- Windows 実機で `cargo xtask doctor` / `cargo xtask check` / `cargo xtask test` が成功
- Windows 実機で `tauri:dev` の `post -> restart -> persist` と author pubkey 不変を確認
- Windows 実機で Credential Manager を使う keyring 有効状態でも author identity が維持されることを確認
- Windows 実機で `KUKURI_INSTANCE` を分けた 2 instance による static-peer ticket import、`post -> reply -> thread`、live/game 伝播が成功
- Windows 実機で片側終了後に相手側が `connected: no, peers: 0` に戻ることを確認
- Windows 実機で複数 topic 維持、topic 単位 unsubscribe、invalid ticket import 時の `Last Error` 更新が期待どおりに機能
- Windows 実機で別 host 間の static-peer 接続と投稿伝播が成功
- Windows 実機で `cargo xtask desktop-package` による NSIS installer build、install、packaged app 起動が成功
- Linux-first MVP の Phase4 desktop 縦スライスは完了

## Phase5 Cutover Check
1. `cargo xtask doctor` を通す。
2. `cargo xtask check` を通す。
3. `cargo xtask test` を通す。
4. `cargo xtask e2e-smoke` を通す。
5. `cargo xtask check` に含まれる Tauri backend compile が通ることを確認する。
6. `sqlite_deletion_does_not_lose_shared_state` と `restart_restores_from_docs_blobs_without_sqlite_seed` が green であることを確認する。
7. `missing_gossip_but_docs_sync_recovers_post` と `gossip_loss_does_not_lose_durable_post` が green であることを確認する。
8. `compat_event_gossip` が current code から除去されていることを確認する。

現在の HEAD では上記 1-8 を local で確認済みで、Phase5 cutover は完了。

補足:
- desktop shell は約 2 秒ごとに timeline / sync status / local ticket を再取得する。
- `Refresh` は強制再取得用で、通常の確認では押さなくても反映される想定。

## 現在の注意点
- `kukuri-transport` の `transport_static_peer_can_connect_endpoint` は required。
- `kukuri-transport` の `transport_two_process_roundtrip_static_peer` は required に戻した。
- deterministic な required lane は `FakeTransport` と `kukuri-harness` が担う。
- Tauri wrapper の単体 compile は `cargo xtask check` に含めて確認する。
- `cargo xtask desktop-package` はWindows hostでcurrent-user NSIS installer、Linux x86_64 hostで署名付きAppImageとDebを生成する。Linuxの前提と検査は [AppImage手順](linux-appimage-smoke.md)／[Deb手順](linux-deb.md) を参照する。

補足:
- GitHub branch protection の required check 名は repo 外設定なので、`Next Fast/Nightly` から `Kukuri Fast/Nightly` への手動更新が必要。
## community-node topic rendezvous
- community-node の local/dev/CI runtime は Postgres に加えて `cn-valkey` (Valkey/Redis-compatible KV) を必須とする。
- `cn-user-api` は `COMMUNITY_NODE_RENDEZVOUS_REDIS_URL` で KV に接続し、`/v1/rendezvous/topics/heartbeat` の TTL 付き ephemeral topic presence を管理する。
- `cn-iroh-relay` は topic state を持たず、純粋な iroh relay として維持する。
- 通信優先度は `Direct P2P -> Relay Supported P2P -> Relay Fallback`。relay URL があるだけでは fallback ではなく、topic rendezvous による接続補助は `Relay Supported P2P` として扱う。
- `Relay Fallback` は Direct P2P と Relay Supported P2P が成立せず、gossip/docs/blob など実データが relay 経由になる場合だけを指す。

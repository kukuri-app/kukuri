# Development Runbook

[AGENTS.md](../../AGENTS.md)の作業原則・設計原則に従い、実行前に今回の目的・対象・期待結果・判定方法を固定する。起票から終了までの手順は[Issue lifecycle runbook](./issue-lifecycle.md)、本書は選んだ操作と検証commandの実行方法を規定する。

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
cargo xtask android-check
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
- `cargo xtask tauri-check` は `apps/desktop/src-tauri` を、その workspace の既定の target（`apps/desktop/src-tauri/target`）へ compile する。root の `target` と分けることで、CI の rust-cache が両方の依存を保存できる（#1413）。
- `cargo xtask tauri-test` は `apps/desktop/src-tauri` の lib 単体 test を実行する（#1234）。この crate は root workspace の `exclude` に入っており、`cargo xtask rust-test` / `cargo xtask test` の対象にならない。target は `tauri-check` と同じ `apps/desktop/src-tauri/target`。`-- <filter>` 以降は test binary へ渡す（例: `cargo xtask tauri-test -- tracing::tests`）。
  - CI では `Kukuri Linux Package` の `linux-appimage` job だけが `cargo xtask-lite tauri-test --package-build` で実行する。`--package-build` は `desktop-package` と同じ release profile / target で build し、package の成果物を再利用する。`Kukuri Fast` は `tauri-check`（compile のみ）で、lib test を実行しない。
  - Windows の test exe は Common Controls v6 の manifest を持たず、そのままでは `STATUS_ENTRYPOINT_NOT_FOUND`（`TaskDialogIndirect`）で起動に失敗する。`tauri-test` は Windows で test exe の隣に外部 manifest（`<exe>.manifest`）を書いてから実行する。Windows は manifest の解決結果を exe の path と更新時刻で cache するため、exe の更新時刻も更新する（manifest なしで一度起動した exe は、manifest を置くだけでは失敗し続ける）。`cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` を直接実行すると同じ失敗になるので、Windows では `tauri-test` を使う。製品 binary の manifest と `build.rs` は変えていない。
  - `cfg(windows)` の test（`commands/os_notification_windows.rs` など）は CI で実行されない。該当 file を変えたときは Windows ローカルで `cargo xtask tauri-test` を実行する。
- `cargo xtask desktop-lint` / `desktop-test` / `desktop-storybook` / `desktop-browser-test` / `desktop-visual-test` は targeted rerun 用。workflow とローカル rerun のどちらでも同じ entrypoint を使う。
- `cargo xtask cn-check` / `cargo xtask cn-test` は `cn-*` server slice の compile/test 用。
- `cargo xtask-lite <command>` は xtask を `harness` feature なしで build して実行する alias（`.cargo/config.toml`）。`e2e-smoke` / `scenario` 以外の command は `cargo xtask` と同じ動作で、xtask 自体の build が軽い。CI の harness を使わない job はこちらを使う（#1120）。
- `Kukuri Fast` / `Kukuri Nightly` / `Kukuri Flake Probe` は GitHub-hosted の標準 runner（`ubuntu-24.04` / `windows-2022`）で動き、Rust の依存を `Swatinem/rust-cache` で GitHub Actions cache に置く（#1413）。key は job id・toolchain・`Cargo.lock` / `Cargo.toml`・`CARGO_*` 等の環境で分かれ、同じ job id の Fast と Nightly は同じ cache を使う。workspace crate の成果物は保存前に除かれる。`Kukuri Fast` は PR の run では保存せず、main push などの run が保存した cache を復元するだけにする。PR の検証の run を持つ `Kukuri Linux Package` / `Kukuri Windows Store Package` も同じで、main push の run が cache を保存する。署名付きの配布物を build する run（release、Linux Package の distribution、Store の dispatch）は cache を使わない（`docs/runbooks/release.md`）。cache の総量は repository の設定で 100GB（保持 7 日）。
- `Kukuri Flake Probe`（`.github/workflows/kukuri-flake-probe.yml`、手動起動のみ）は、同じ suite を繰り返し実行して失敗率を測る。lane（`rust` / `vitest` / `playwright` / `all`）と 1 shard あたりの回数を指定し、2 shard を並行させる（既定は 2 × 10 = 20 回）。test の並列度を上げる変更の前後で使う（#1121）。実体は `scripts/ci/flake_probe.sh` で、失敗しても最後まで回し、失敗回数と各回の所要秒を step summary に出す。rust-cache の key は job id（`rust` / `vitest` / `playwright`）で分かれ、CI 本体の cache を上書きしない。
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

ローカルでは受入条件と変更影響に対応するtest・型検査等を[検証マトリクス](../../REFACTORING.md#path別検証マトリクス)から選び、全体確認はPR CIで行う。文書の誤字など区分Aは同節の対象確認を使う。UIの証跡は [ADR 0014](../adr/0014-uiux-dev-flow.md)、視覚baselineの更新方法は次節を参照する。

## ローカル先行検証

接続・同期・取得を変更する際は、AGENTS.mdの設計原則に従い、総履歴の走査・完全取得・一括再同期を前提にしない。現在の経路を[現行inventory](../architecture/network-work-inventory.md)と照合し、関連する[共通ownerの設計案](../adr/0055-demand-owned-network-work.md)を参照する。既存の比例経路は未解消の事実として扱い、設計案を移行済みの証拠にしない。

受入条件に含めた所有・停止、権限・保存、cursorの公平性、公開schemaのうち変更に必要な検証だけを選ぶ。全項目の機械的な再監査や、未依頼の失敗条件の追加を行わない。同種の不具合へ個別のguardやtaskを追加する前に、共通の責務へ統合・旧処理を削除した最終形を比較する。

Store migrationを追加・変更したPRでは、up/downの対と`crates/store/src/tests/migrations.rs`の
世代数、`migrations_roundtrip.rs`の`EXPECTED_VERSIONS`、
`fixtures/schema/store_schema_full.txt`を同じ差分で確認する。意図したschema変更ならgoldenを
再生成して差分をreviewし、対応する世代/roundtrip/goldenの局所testと変更indexのquery planを
PR前に実行する。世代数だけの同期でgoldenをCI任せにしない。

irohのmapped address回収を変更・更新するときは
`python tools/check_iroh_resource_contract.py`（Python 3.13）で固定SHAの回収contractを確認する。
Cargoの共有source cacheを変更せず、`target/upstream-contracts`内の一時checkoutへtestだけを追加する。
`--revision <40桁SHA>`で変更前を再現できる。PR/nightlyのRust jobでも同じcontractを実行する。

ローカルの関連検証で実装を確認し、全体の確認が必要になった段階でPRを作成してCIを使う。同じ未変更範囲の全suiteをローカルとCIで反復しない。CI固有の失敗は原因と必要な再実行範囲を確定してから修正する。

- 関連validationとその結果、CI・別環境で補う必要な範囲を記録する。未実行の検証は成功と区別する。
- workflow を変えるときは `actionlint <対象 file>` を実行する。self-hosted の runner label を使う場合は `.github/actionlint.yaml` に登録する（#1413 以降、CI は GitHub-hosted の runner だけで動く）。
- `docker/cn/**` やimageのbuild手順を変えるときは、ローカルで変更した設定・script・対象imageの関連検証を行い、全imageのbuild/smokeはPR CIで確認する。全体のlocal再現が必要な場合のcommandは `docker buildx bake --file docker/cn/docker-bake.hcl --allow "fs.write=<出力先>"`。
  - `OUT_DIR=<出力先>` を設定し、`--set '*.platform=linux/amd64'` を付けて比較対象を固定する。出力先は build context の外に置く。
  - bake 後に `python scripts/ci/cn_image_check.py <出力先>`（Linux は `python3`）を実行する。Python 3.11 以上、Docker、PATH 上の Bash が必要。Windows は Git Bash の `bin` を PATH の先頭へ加える。
  - 4 本番 OCI の全 layer・圧縮サイズ・entrypoint を検査し、同じ config/layer を Docker に読み込んで起動確認する。PostgreSQL/Valkey は専用 internal network と一時 container を使い、終了時にその資源だけを除去する。既存 indexer の正負 smoke は `scripts/ci/cn_indexer_smoke.sh` を共用する。結果は出力先の `image-check-results.json` に残る。
- CI 専用の設定（runner、cache、同時実行枠）を変えるときは、変更前後の計測値と根拠を Issue に記録する。
- ローカルで再現できない項目（実 runner の版差、cache の当たり外れ、registry への push）は、PR の run か merge 後の run で確認する。その項目を PR 本文の「検証」に明記する。
- 反復して失敗率を測るときは `Kukuri Flake Probe` を使い、通常の CI を繰り返し起動しない。

PR作成後は `gh pr checks <番号>` で実際に発生したcheckの成功を確認してからmergeする。文書のみ等でパス条件に該当せずcheckが0件の場合は、その事実を確認して完了できる。CIを発火させるためだけの変更や手動の全suite実行は不要。

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
- `@playwright/test`（同梱 Chromium）更新や runner イメージ更新でフォント/AA が変わると baseline が一斉に割れることがある。その場合は deps 更新 PR に baseline 再生成を同梱する。baseline 生成（`kukuri-visual-baseline.yml`）と比較（`kukuri-fast.yml` の `linux-desktop-browser`）は同じ runner image（`ubuntu-24.04`、#1413）で動かし、image を変えるときは両方を同じ PR で変える。
- baseline の置き場は `apps/desktop/tests/playwright/__screenshots__/`（`.gitignore` 済みの `test-results/` とは別。混同しない）。

## community-node compose
```bash
docker compose --env-file .env.community-node -f docker-compose.community-node.yml run --rm cn-migrate
docker compose --env-file .env.community-node -f docker-compose.community-node.yml up --build cn-user-api cn-iroh-relay cn-stun
```

- host port の既定値は `18080` (`cn-user-api`), `13340` (`cn-iroh-relay`), `15432` (`cn-postgres`), `16379` (`cn-valkey`)。`cn-stun` の `3478/udp` は、クライアントが relay の host の 3478 番へ送るので変えない
- host 側 bind の既定値は loopback (`127.0.0.1`) なので、LAN/WireGuard 越しに公開する場合は `CN_*_HOST_BIND_IP` を上書きする
- `cn-stun`（STUN、binding だけ。ADR 0057 §6）は client の送信元の address を見て返すので、送信元を書き換えずに受ける IP で公開する。送信元を書き換えて UDP を転送する VPS edge 構成では起動しない（[VPS edge の手順](community-node-self-host-vps.md)）。応答は[本番反映の runbook](community-node-production-rollout.md) の「5.3 public surface」の確認 script を、公開した host へ実行して確かめる
- compose 内の service 名は `cn-postgres`, `cn-migrate`, `cn-user-api`, `cn-iroh-relay`, `cn-stun`。公開 blob の発見の補助 index の `cn-addr-index` は profile `addr-index` を指定したときだけ起動する（下記）
- public URL を変える場合は `CN_BASE_URL`, `CN_PUBLIC_BASE_URL`, `COMMUNITY_NODE_CONNECTIVITY_URLS` を上書きする
- `cn-user-api` は `COMMUNITY_NODE_DATABASE_INIT_MODE=require_ready` で起動するので、`cn-migrate` または `cn-cli prepare` を先に流さないと fail-fast する

### 公開 blob の発見の補助 index（cn-addr-index、#1632、ADR 0063 §7）
- kukuri が運用する server（署名つきの一覧の最大 2 台）だけで動かす。起動は `docker compose --env-file .env.community-node -f docker-compose.community-node.yml --profile addr-index up -d cn-addr-index`、停止は同じ指定の `stop cn-addr-index`。record は memory だけに持ち、止めると消える（端末は約 30 分ごとに登録し直す）
- `60125/udp`（DHT と共有）を `CN_ADDR_INDEX_HOST_BIND_IP` で公開する。record を送信元の address で引くので、cn-stun と同じく送信元を書き換えずに受ける IP で公開する（VPS edge 構成では提供しない）
- 上限は `CN_ADDR_INDEX_MAX_ENTRIES`（保持する record、既定 2,000,000）・`CN_ADDR_INDEX_MAX_ENTRIES_PER_IP`（送信元 IP ごとの record、16）・`CN_ADDR_INDEX_REQUESTS_PER_IP_PER_SEC`（送信元 IP ごとの毎秒の要求（登録と照会）、50）。空なら既定
- 一覧の公開: 一覧の鍵（64 桁の 16 進）を持つ host で、fork の `iroh-index-list` を常駐させる（`cargo install --git https://github.com/KingYoSun/iroh-content-discovery --rev f6e2864a1d2228a19aa1cb88239ba87eb0068d2e iroh-mainline-endpoint-discovery --features cli --bin iroh-index-list --locked` で入れ、`IROH_INDEX_LIST_SECRET=<鍵> iroh-index-list --server <IP>:60125 [--server <IP>:60125]`）。10 分ごとに一覧を更新し、止めると一覧は DHT 上の期限で消える。server を変えるときは同じ鍵で新しい一覧を公開する
- 一覧の公開鍵を `kukuri_transport::KUKURI_PUBLIC_BLOB_INDEX`（`PublicBlobIndex::ListKey`）へ入れた build で、client の発見と、operator config の `features.public_blob_search: true`（既定は無効。有効にすると生成文書に開示が載り、利用者はその node で再同意する）にした `cn-user-api` の検索（`POST /v1/blob-providers/search`）が働く。capability が無効な node は検索に 404 を返し、bootstrap の自 node の `resolved_urls.public_blob_search` は false。鍵の無い build で capability を有効にすると `cn-user-api` は起動しない。鍵の発行・server の配置・kukuri の node の有効化は本番反映の Issue で行う
- GCP low-cost の kukuri 運営 node（#1657）では、`public_blob_index_list_secret_id` に Secret Manager の一覧の鍵 ID を指定すると、60125/udp の firewall、host network の `cn-addr-index`、同じ relay image 内の固定 fork の `iroh-index-list` を配置する。publisher は VM の static IP の 1 台の一覧を更新する。鍵は startup 時に取得して root 専用 `.env` へ置き、publisher だけへ渡す。汎用 node は空のままにし、検索の capability と index の運営を混同しない。生成 tfvars にこの運営専用設定は含まれないので、`*.auto.tfvars` に鍵 ID だけを置く。
- `cn-user-api` の既定の log の絞り込みは `iroh_mainline_address_lookup=warn` を含む（住所の解決が info で出す、見つけた端末の endpoint ID・到達情報を log に残さない。開示どおり）。`RUST_LOG` で絞り込みを置き換えるときも、この指定を含める
- Web は、検索を提供する利用中の node へ、既知の相手で取れない公開 blob の hash を送る（ADR 0063 §8）。実ブラウザの経路は `cargo xtask web-e2e public-blob`（fixture の Community Node は Testnet の DHT と同じ process の補助 index で検索を提供する）

### 公開 blob の発見の preview での確認と観測（#1632、ADR 0063 §3・§8）
- 前提: 一覧の鍵の入った build、補助 index の server と一覧の公開、検索を有効にした Community Node（bootstrap の自 node の `resolved_urls.public_blob_search` が true）。確かめる利用者は、その node の文書に同意している。本番での実施と記録は #1657 で行う
- 組み立て: 公開投稿の画像を、取得する端末の既知の相手（同じ topic の参加者のほか、学習した相手・seed・import・最近の相手・hash ごとの取得元）が持たず、別の端末（保持端末）だけが持つ状態にする。保持端末は「公開コンテンツの発見」がオンのデスクトップ版にする（Web は告知しない）。保持端末は、その画像を表示して cache に持つか、同じ画像を自分の公開投稿に添え、告知と補助 index への登録の間隔（約 10 分・約 30 分）を待つ（告知を終えたことを示す log は無い。登録は `RUST_LOG` に `iroh_mainline_endpoint_discovery=debug` を足すと出る `published index record` で分かる）。投稿者など既知の相手は、画像を取得させる前に止める（実ブラウザの E2E の `public-blob` と同じ組み立て）。取得する側は、デスクトップ版（DHT で探す）とブラウザ版（Community Node の検索）
- 観測（試した件数・条件とともに作業記録へ残す）
  - 取得成功率と待ち時間: 投稿を画面に入れてから画像が出るまでの時間と、出なかった件数。1 要求は 30 秒まで、続きは既存の再試行（5・30・120 秒）
  - 候補の消費数: デスクトップ版は開発者モードのアプリ内ログの取得の行（既定の絞り込みには出ないので、`RUST_LOG=warn,kukuri_desktop_tauri_lib=info,kukuri_app_api=info,kukuri_connectivity=info,kukuri_iroh_node=info` で起動する）。`fetch local miss, trying remote peers` の `selected_peer_count` が既知の候補の数。端末ごとの行は失敗と延期のときだけ出るので、消費数は、その行に出た端末の数に、取得できたなら 1 を足して数える。ブラウザ版は端末ごとの試行を出さないので、開発者ツールの network の `POST /v1/blob-providers/search` の応答の `candidates` の数（返した数）と `partial` を記録する
  - 検索と告知の bytes: ブラウザ版は検索の要求・応答の大きさ（開発者ツール）。デスクトップ版の DHT・補助 index の UDP は、process ごとの量に iroh の QUIC・relay・Community Node の HTTP も含まれるので、OS の資源の監視で相手の address（DHT の参加者・補助 index の server）ごとの量を見る（表示は速度なので、おおよその量になる）
  - 実際の転送経路: 取得の間の受信量を画像の大きさと比べる。ブラウザ版の経路は relay の WebSocket と WebRTC の DataChannel の 2 つだけなので、`chrome://webrtc-internals` の DataChannel の受信 bytes を見る（画像の大きさに届けば WebRTC、届かずに画像が出たなら relay。開発者ツールでは WebSocket の受信量の合計を読めない）。デスクトップ版は、OS の資源の監視で、relay の host との通信量と、保持端末の address との UDP の量を比べる。本番の relay は metrics を公開していない。診断の active path は topic の接続相手の経路で、blob の取得の接続を表さないので、参考にとどめる

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

## Web（wasm32）の build と browser 試験
ブラウザでも動く共用 crate（ADR 0056 §2・§3）の wasm32 の clippy と、headless の Chromium での browser 試験は CI の `linux-web-transport` が行う。
browser 試験は native の相手（example）を起動し、その URL を試験の build に渡す（`scripts/ci/browser_peer_test.sh <package> <example> [cargo の引数]`）。

- Linux: `clang`・`llvm`（`llvm-ar`）、`rustup target add wasm32-unknown-unknown`、`Cargo.lock` の wasm-bindgen と同じ版の `wasm-bindgen-cli`、chromedriver が要る。
  `CC_wasm32_unknown_unknown=clang AR_wasm32_unknown_unknown=llvm-ar CHROMEDRIVER=<chromedriver の path>` を付けて実行する。
- Windows: host に clang が無い（secp256k1 の C の build が通らない）ので、`docker/wasm-dev/Dockerfile` の image を使う。Git Bash では次のとおり。

```bash
docker build -t kukuri-wasm-dev docker/wasm-dev
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$(cygpath -w "$PWD"):/src" -v kukuri-wasm-cargo:/usr/local/cargo/registry -v kukuri-wasm-git:/usr/local/cargo/git -v kukuri-wasm-target:/target -e CARGO_TARGET_DIR=/target -w /src kukuri-wasm-dev bash scripts/ci/browser_peer_test.sh kukuri-iroh-node web_peer
```

- 共用 crate の wasm32 の clippy は `cargo clippy --target wasm32-unknown-unknown -p kukuri-core -p kukuri-store -p kukuri-transport -p kukuri-iroh-node -p kukuri-docs-sync -p kukuri-blob-service -p kukuri-webrtc-transport -p kukuri-app-api -p kukuri-metaverse-host -p kukuri-desktop-runtime -p kukuri-web-runtime -- -D warnings`（image では `rustup component add clippy` を先に行う）。
  共用 crate では tokio・std の時刻と task を直接使わず `n0_future`・`web_time` を使う。直接使うと wasm32 の clippy が `disallowed_methods` で止める。
- W9 の transport の browser 試験は `scripts/ci/browser_peer_test.sh kukuri-webrtc-transport webrtc_peer --features test-signaling`。
- W2・W3 の IndexedDB の保存（blob と docs の record）と native との送受信の browser 試験は `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer`。
  W4 AC-2 の鍵・設定・projection・peer の接続候補の保存（reload、失敗の区別、EndpointId の保持、store の parity の scenario）も同じ試験に含む（`src/account_tests.rs`）。

### Web の build と実ブラウザの主要導線の試験（ADR 0060 §1・§4、#1220 W8）
- Web の build: `apps/desktop` の Vite を `VITE_KUKURI_TARGET=web` で動かす（出力は `apps/desktop/dist-web`）。入力の web-runtime は `wasm-bindgen --target web --split-linked-modules` の出力を `apps/desktop/web-runtime-pkg`（別の場所なら `KUKURI_WEB_RUNTIME_PKG`）に置く。Community Node の初期設定は native の配布の設定（`src-tauri/distribution/community-nodes.json`）で、開発・試験では `VITE_KUKURI_COMMUNITY_NODE_BASE_URL` で替える。
- 主要導線の試験は `cargo xtask-lite web-e2e [<scenario>...]`。wasm と Web の build、harness の `web_e2e_fixture`（in-process の Community Node と relay、native の相手、`dist-web` の配信。Postgres と valkey は `cn-test` と同じ compose）、WebdriverIO の driver（`apps/desktop/tests/web-e2e/main-flow.mjs`。ブラウザは `KUKURI_WEB_E2E_BROWSER` の `chrome`（既定）・`firefox`・`safari`・`android`。driver は `CHROMEDRIVER`・`GECKODRIVER`、無ければ自動で入れる。Safari は macOS の safaridriver）を順に動かす。Linux で wasm の build の要件（上）と Docker が要る。`--no-compose` は compose を起動せず、`CN_POSTGRES_PORT`・`CN_VALKEY_PORT` の既存の Postgres・valkey（user `cn`、password `cn_password`、DB `cn`。DB を作れる権限が要る）を使う。
  - ページより先に動く試験の script（`tests/web-e2e/page-init.js`）は、fixture が配信の `index.html` に同じ origin の script として足す（driver の機能に依らず、どのブラウザ・tab でも動く。配信の artifact は変えない）。fallback の端は、driver が置く cookie `kukuri-e2e-without-ice` で示す。
  - scenario（一覧は `node tests/web-e2e/main-flow.mjs --list`）は、それぞれ新しい fixture と新しい client で始め、他の scenario の状態に依存しない。scenario を省くと全部を順に回し、失敗した scenario をまとめて示す（#1559）。
  - CI は、build の job（`linux-web-e2e-build`。`--build-only`）が `dist-web` と fixture を 1 回だけ作る。scenario ごとの job（`linux-web-e2e (<scenario>)`）は、その成果物を受け取って `--no-build <scenario>` で並列に回す。scenario の一覧は、build の job が `--list` で matrix に渡す。
  - Safari の `lifecycle` の反復は `gh workflow run kukuri-flake-probe.yml --ref <branch> -f lane=web-safari -f repeats=3`。同じ build を使い、各回は新しい fixture/client で実行する。失敗しても全回を試し、1 回でも失敗すれば job は失敗する。通常の夜間・手動実行は各 scenario 1 回のまま。
  - この 2 つの job は再利用の workflow `.github/workflows/kukuri-web-e2e.yml` にある。Fast は Chrome で呼び（merge の条件）、同じ workflow を夜間と手動（workflow_dispatch の `browser`）で Firefox で回す（merge の条件にしない。失敗は Issue にする。#1220 AC-5a）。Safari も夜間と手動で、macOS 15 arm64 の runner で回す（`macos-web-e2e-build` が fixture を macOS で作り、`macos-web-e2e (<scenario>)` が Web の build を linux の build の job から受け取る。Postgres・valkey は Homebrew のもので `--no-compose`。Chrome は `KUKURI_WEB_E2E_CHROME_VERSION=stable` で、自動更新されない Chrome for Testing を driver と組で使う）。safaridriver は同時に 1 つの session しか開けないので、各 scenario の最初の client だけが Safari で、2 台目からは同じ runner の Chrome にする（#1220 AC-5b）。Android も夜間と手動で、ubuntu-24.04 の emulator（電話の幅）の Chrome で回す（`android-web-e2e (<scenario>)`。`scripts/ci/android_web_e2e.sh` が emulator の Chrome に合う chromedriver を取り、driver が adb reverse で fixture へ届かせる）。各 scenario の最初の client だけが Android で、2 台目からは同じ runner の Chrome にする（#1220 AC-5c）。driver で作れない段は、PASS の行に `unconfirmed:` として示す。
- Windows では wasm の build（上の image）と Web の build を行ってから、手元の Postgres・valkey を `COMMUNITY_NODE_DATABASE_URL`・`COMMUNITY_NODE_RENDEZVOUS_REDIS_URL` で渡して `web_e2e_fixture` を起動し（`KUKURI_WEB_E2E_DIST` に `dist-web`）、`node tests/web-e2e/main-flow.mjs <scenario>` を `apps/desktop` で動かす。scenario ごとに fixture を起動し直す。Web の build は fixture の user-api の URL（既定 `http://127.0.0.1:4181`）を `VITE_KUKURI_COMMUNITY_NODE_BASE_URL` に渡して作る。
  - fixture の Community Node は、rendezvous の key に fixture ごとの DB の名前を使う。同じ valkey で動く別の fixture（並行する作業の試験）と topic の在席情報を共有しないので、他の試験の投稿が混ざらない（#1559）。

### 上流 iroh-blobs の版を上げるとき（ADR 0058 §5、#1215 W2 AC-4）
iroh-blobs は fork しない。Web は上流の `MemStore` と、blob の提供・取得の既存の protocol（`iroh_blobs::ALPN` の ephemeral の取得と、`/kukuri/remote-blob/1` の fallback）だけを使い、blob の保存は保存 trait（`ContentCacheStore`）の IndexedDB の実装が持つ。版を上げる PR では、上流の変更点を読んでから次を順に行い、どれかが通らない版は採らない。

1. root `Cargo.toml` の `iroh-blobs` の版を上げ、`cargo update -p iroh-blobs` で lockfile を更新する。workspace では `default-features = false`、native だけの feature（`fs-store`・`rpc` 等）は `crates/iroh-node/Cargo.toml` の native の節にある。
2. native の互換: `cargo test -p kukuri-iroh-node -p kukuri-blob-service -p kukuri-docs-sync`（remote の取得・`/kukuri/remote-blob/1`・`FsStore` の読み直し・表示用の状態確認が remote から取らない契約・docs の entry の取得）。取得 gate そのものの試験（`cargo test -p kukuri-app-api tests::media`）は PR の CI で確かめる。iroh-docs（下）も iroh-blobs に依存するので、型が合わなければ iroh-docs の patch rev も同じ PR で上げる。
3. WASM の build: 上の共用 crate の wasm32 の clippy。
4. browser↔native の roundtrip: `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer`（保存 → reload → native との送受信、relay と WebRTC の経路、`MemStore` に blob-service の内容が残らないこと、取得 gate の前提）。

### 上流 iroh-docs の patch rev を上げるとき（ADR 0058 §7、#1216 W3 AC-4）
iroh-docs は fork せず、root `Cargo.toml` の `[patch.crates-io]` で上流の rev を固定する。native は redb の永続 store、Web は `Store::memory()` を使い、Web の本人の record は保存 trait（`ContentCacheStore`）の IndexedDB の実装が持つ。Web の memory store は、上流の GC と iroh-docs の保護（`ProtectCallbackHandler`）で閉じた replica の内容を消す。上流の変更点（特に store の形式、`drop_doc`、`ProtectCallbackHandler`、`get_many` の並び）を読んでから次を順に行い、どれかが通らない rev は採らない。型の compile が通ることだけを Web の保存の成功としない。

1. root `Cargo.toml` の iroh-docs の patch rev を上げ、`cargo update -p iroh-docs` で lockfile を更新する。
2. native の保存互換: `cargo test -p kukuri-docs-sync -p kukuri-iroh-node`（既存の redb の store を開き直す試験、有界な reader、本人の record の保護と保持分の併合）と `cargo test -p kukuri-desktop-runtime protected_migration`（旧 store からの移行）。
3. WASM の build: 上の共用 crate の wasm32 の clippy。
4. browser での動作: `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer`（本人の record の保護、reload の後の復元、native との読み合い、閉じた replica の memory からの回収、失敗の負例）。

## Windows 前提
- Windows prerequisites は Tauri 公式手順を使う: <https://v2.tauri.app/start/prerequisites/#windows>
- 初回 Windows cut の対象は `x86_64-pc-windows-msvc` のみ
- installer build は current-user NSIS + WebView2 download bootstrapper を前提にする
- Windows の bin は `apps/desktop/src-tauri/build.rs` で main thread の stack reserve を 8 MiB にする（exe 既定の 1 MiB では、Tauri が main thread で値渡しする command の future が約 33 KiB を超えると stack overflow で終了する。#1526）。command の future の上限は `crates/desktop-runtime/tests/command_future_sizes.rs` で固定する

## Android 前提（#1193・#1194）
- 前提は Tauri 公式手順を使う: <https://v2.tauri.app/start/prerequisites/#android>。JDK 17 以上（Android Studio 付属の JBR 21 で確認）、Android SDK の platform 36、NDK r29（`NDK_HOME` か `ANDROID_NDK_HOME`）、Rust の target `aarch64-linux-android`（配布）と `x86_64-linux-android`（emulator）を入れる。
- 対象は #1193 の D5・D7 に従う。applicationId は `app.kukuri.android`（`apps/desktop/src-tauri/tauri.android.conf.json`。desktop の identifier と保存先は変えない）、minSdk 29、compileSdk・targetSdk 36、配布の ABI は arm64-v8a だけで、x86_64 は emulator での検証に使う。
- Android project は `apps/desktop/src-tauri/gen/android` に置き、git で追跡する。Tauri CLI 2.12.0 の `tauri android init` の出力から、CLI の呼び方（PATH の node で `apps/desktop/scripts/tauri-cli.mjs` を呼ぶ）、SDK の版（雛形は 37）、Android TV の宣言を直し、未使用の雛形（layout・色・night の theme）と既定のアイコンを除いた。ランチャーアイコンは `src-tauri/icons/android`（`docs/ASSET_MANIFEST.json` で管理）を Gradle の res として直接読む。`gen/android` の build の生成物は同 directory の `.gitignore` が除く。
- desktop だけの処理（tray、終了の横取りと signal、多重起動の制御、updater）は `cfg(desktop)` で Android の build から外す。Android の OS 通知は #1197 AC-4 まで `unavailable` を返し、更新は Google Play が管理する（`update_managed_by_google_play`）。frontend は Tauri CLI が build に渡す `TAURI_ENV_PLATFORM=android` で配布方式を `google-play` にし（`vite.config.ts` の `envPrefix`、`src/lib/distribution.ts`）、更新の確認を予約せず、設定の「リリース」に Google Play が管理する旨を出す（#1199 AC-3）。

```bash
cargo xtask android-check
npx pnpm@10.16.1 --dir apps/desktop tauri android dev
npx pnpm@10.16.1 --dir apps/desktop tauri android build --debug --apk --target x86_64
npx pnpm@10.16.1 --dir apps/desktop tauri android build --debug --apk --target aarch64
npx pnpm@10.16.1 --dir apps/desktop tauri android build --apk --aab --target aarch64
```

- `cargo xtask android-check` は src-tauri を `aarch64-linux-android` 向けに `cargo check` する。NDK の clang と llvm-ar を C を含む依存の build に使う。CI では `Kukuri Fast` の `android-check` job が runner の NDK で実行する。
- `tauri android dev` は接続中の emulator か端末へ入れて起動する。Windows では Tauri CLI が端末の LAN の address を devUrl に使い、`TAURI_DEV_HOST` に入れる。Android の設定（`tauri.android.conf.json`）の開発 server は host を固定せず、`vite.config.ts` がその値で待ち受ける（同じ LAN から届く）。port は 5173 で固定。
- `tauri android build --debug --apk --target <x86_64|aarch64>` は debug 署名の APK を `apps/desktop/src-tauri/gen/android/app/build/outputs/apk/universal/debug/` に出す。既定の debug 情報では Rust の共有 library だけで 1.3 GB を超え、差分の再 packaging で 2 GB を超えると `adb install` が失敗するので、CI と同じ `CARGO_PROFILE_DEV_DEBUG=line-tables-only` を付ける（x86_64 で約 480 MB）。upload 鍵で署名した AAB と配布は #1199 が所有する。Play へ出す release の build の versionCode は release tag から作る（[Google Play 配布](android-play-release.md#versioncode)）。
- `tauri android build --apk --aab --target aarch64`（release）は R8 で最適化した未署名の APK と AAB を `apps/desktop/src-tauri/gen/android/app/build/outputs/{apk/universal/release,bundle/universalRelease}/` に出す（手元で約 17 分）。端末で動かすときは、APK に Android SDK の debug 鍵（`~/.android/debug.keystore`）で `apksigner` の署名をする（確認用。配布には使わない）。
- 16 KiB ページ（#1194 AC-3）: NDK r29 の linker の既定で、native library の LOAD segment は 16 KiB 境界になる。`llvm-readelf -lW` の LOAD の Align と `zipalign -c -P 16 -v 4` で確かめ、起動は 16 KiB の emulator（`system-images;android-36.1;google_apis_ps16k;x86_64`、`getconf PAGE_SIZE` が 16384）で見る。
- Android の開発版は配布版と同じ applicationId で、署名が違うため同じ端末に並ばない。desktop の開発版の兄弟 dir（`<identifier>.dev`、#1105）は使わず、OS の app data dir（`/data/user/0/app.kukuri.android`）をそのまま使う。

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

以下の手動確認は、合意した利用条件と自動検証で不足する点から選ぶ参照表である。対象OS・操作・期待結果・確認を打ち切る時点を先に固定する。Social graph、Private channel、DHT、Windowsの各節も同じ扱いとし、未依頼のlaneや異常系を追加しない。

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

最小laneはstatic-peer ticket import。`invite_only`、`friend_only`、`friend_plus`のうち、今回の受入条件に含めたaudienceだけを確認する。

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
7. 端末 A でその private channel に post し、端末 B の当該 channel view にだけ表示され、`Public` には出ないことを確認する。
8. 端末 B でその private post に reply し、thread が同じ private channel 内でのみ見えることを確認する。
9. 端末 A でその private channel 上に live session を作成し、端末 B が `join -> leave -> end` を追従できることを確認する。
10. 端末 A でその private channel 上に game room を作成し、score / status 更新が端末 B に反映されることを確認する。
11. 両端末を再起動し、invite 再入力なしで joined private channel が復元され、private post / thread / live / game が再表示されることを確認する。
12. 端末 B でも `Create Invite` が可能で、fresh invite を再発行できることを確認する。
13. 3 台目の未招待端末 C を使う場合は、同じ topic の `Public` から private channel content が見えないことを確認する。

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

- CNのsessionは期限の来たnodeだけを、観測送信はnodeごとの独立laneで実行される。15秒ごとのself-healは撤去し、neighborの無いgossip topicはtransportがtopicごとに再joinする（#1221 R2-B、ADR 0055 §3.1）。応答待ちのCNがある場合は、`kukuri_connectivity`の失敗段階・retryと、正常CNのheartbeat/rendezvous期限を分けて確認する。共通HTTP clientの上限は接続5秒・本文込み10秒。上限を長くするだけで回復したと判定しない。
- `kukuri_connectivity=info`は既定filterに含まれる。scheduler開始/停止、session ready/失敗、local docs actor不通、stack再構築の世代/commitを記録する。明示的な`RUST_LOG`を使う場合は必要に応じこのtargetを追加する。token・鍵・本文をログへ追加しない。
- `sending to iroh_docs actor failed`はCNのHTTP失敗とは別層。正規のstack切替中か、再構築失敗後かを世代とcommit記録で確認する。remote peerがofflineなだけならlocal actorは維持される。local probeのtimeoutはactor破損の証拠にしない。
- dedicated profileで表示/非表示のみ/画面ロック/suspendを区別し、開始・格納・復帰時刻、版・OS/WebView、CN期限、peer数、実投稿/返信/blobの到達を記録する。既定bufferは2000件/1MiBで過去が落ちるので、調査中は明示的に標準出力を保存する。製品が常時ログファイルを書き出す機能ではない。
- 回帰testは先に原因を特定し、channel制御・一回のfuture poll・仮想時間で確認する。CIへ長時間の放置・実時間sleepを追加しない。`cargo test --locked -p kukuri-desktop-runtime idle_`、`cargo test --locked -p kukuri-desktop-runtime stalled_`が本件の最小確認。実peerの接続確認には既存のconnectivity scenarioを使う。

### 一般的なWindows動作確認

1. native Windows hostで必要な環境を確認し、選択した操作に関係する検証を実行する。全体確認のCI結果は再利用する。
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

## Phase5 Cutover Check（完了時の記録）
1. `cargo xtask doctor` を通す。
2. `cargo xtask check` を通す。
3. `cargo xtask test` を通す。
4. `cargo xtask e2e-smoke` を通す。
5. `cargo xtask check` に含まれる Tauri backend compile が通ることを確認する。
6. `sqlite_deletion_does_not_lose_shared_state` と `restart_restores_from_docs_blobs_without_sqlite_seed` が green であることを確認する。
7. `missing_gossip_but_docs_sync_recovers_post` と `gossip_loss_does_not_lose_durable_post` が green であることを確認する。
8. `compat_event_gossip` が current code から除去されていることを確認する。

上記1-8は当時のPhase5完了時の確認記録であり、現在のHEADの検証結果や各変更への再実行指示ではない。現在の確認は固定した受入条件と変更影響から選ぶ。

補足:
- 当時のdesktop shellは約2秒ごとにtimeline / sync status / local ticketを再取得していた。周期的な全件読取りを現在の設計目標や必須挙動として固定しない。
- `Refresh` は強制再取得用で、通常の確認では押さなくても反映される想定。

## 現在の注意点
- `kukuri-transport` の `transport_static_peer_can_connect_endpoint` は required。
- `kukuri-transport` の `transport_two_process_roundtrip_static_peer` は required に戻した。
- deterministic な required lane は `FakeTransport` と `kukuri-harness` が担う。
- Tauri wrapperのcompileは関連する `cargo xtask tauri-check` またはCIで確認する。
- `cargo xtask desktop-package` はWindows hostでcurrent-user NSIS installer、Linux x86_64 hostで署名付きAppImageとDebを生成する。Linuxの前提と検査は [AppImage手順](linux-appimage-smoke.md)／[Deb手順](linux-deb.md) を参照する。

補足:
- GitHub branch protection の required check 名は repo 外設定なので、`Next Fast/Nightly` から `Kukuri Fast/Nightly` への手動更新が必要。
## community-node topic rendezvous
- community-node の local/dev/CI runtime は Postgres に加えて `cn-valkey` (Valkey/Redis-compatible KV) を必須とする。
- `cn-user-api` は `COMMUNITY_NODE_RENDEZVOUS_REDIS_URL` で KV に接続し、`/v1/rendezvous/topics/heartbeat` の TTL 付き ephemeral topic presence を管理する。
- `cn-iroh-relay` は topic state を持たず、純粋な iroh relay として維持する。
- 通信優先度は `Direct P2P -> Relay Supported P2P -> Relay Fallback`。relay URL があるだけでは fallback ではなく、topic rendezvous による接続補助は `Relay Supported P2P` として扱う。
- `Relay Fallback` は Direct P2P と Relay Supported P2P が成立せず、gossip/docs/blob など実データが relay 経由になる場合だけを指す。

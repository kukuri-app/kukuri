# kukuri docs

## 目的
- 現行 kukuri 実装に必要な情報だけを置く。
- 仕様は ADR、実行手順は runbook、状態は progress に分ける。
- `docs/progress/` は 2 種を置く: (a) named な current-state / 計画文書（最新のマイルストーン状態・ロードマップ）と (b) `YYYY-MM-DD-<issue# または WP 名>-<slug>.md` の per-issue / per-WP 作業報告（個別 issue・ワークパッケージの実装ログ。例: `2026-07-06-wp-c1-...`）。current-state は「今どうなっているか」、per-issue / per-WP は「その作業で何をしたか」を書き分ける。

## 現行スコープを把握する参照順

次は現状を読む順番であり、異なる責務の規則を上書きする優先順位ではない。作業に関係する文書を選んで読む。

1. `docs/progress/2026-04-16-mvp-builder-preview-plan.md`
2. `docs/progress/2026-03-10-foundation.md`
3. `docs/progress/2026-03-24-shell-ui-production-migration.md`
4. `docs/runbooks/dev.md`
5. `docs/runbooks/mvp-user-quickstart.md`
6. `docs/runbooks/mvp-troubleshooting.md`
7. `docs/adr/0001-linux-first-mvp.md`
8. `docs/adr/0007-windows-desktop-support.md`
9. `docs/adr/0008-dht-discovery-data-classification.md`
10. `docs/adr/0009-community-node-relay-auth-data-classification.md`
11. `docs/adr/0014-uiux-dev-flow.md`
12. `DESIGN.md`（root・ビジュアル仕様）
13. `harness/scenarios/`
14. `docs/adr/0003-image-post-data-classification.md`
15. `docs/adr/0004-video-post-data-classification.md`

## 現在の対象
- `desktop + core + store + docs-sync + blob-service + desktop-runtime + cn-* + harness`
- desktop target は Linux / Windows
- current connectivity scope は `static-peer + seeded DHT + community-node connectivity/auth`
- current product scope には `social graph v1 + private channel audience v1` を含む
- root 実行入口は `cargo xtask ...`
- 日常 validation は `cargo xtask check` + `cargo xtask test`
- browser-level UI change は `cargo xtask desktop-ui-check`
- community-node / Postgres slice は `cargo xtask cn-check` + `cargo xtask cn-test`
- targeted rerun は `cargo xtask rust-check|rust-test|tauri-check|desktop-lint|desktop-test|desktop-storybook|desktop-browser-test`
- 新 feature 着手前に `docs/adr/0002-feature-data-classification-template.md` を埋める。

## Development process

- Issueの起票、scope固定、計画、実装、PR、独立監査、Close/Reopen: `docs/runbooks/issue-lifecycle.md`
- 実装計画の要否、粒度と記録形式: root `PLANS.md`
- path別validationとリファクタリング境界: root `REFACTORING.md`
- 初回campaignの完了記録: `docs/progress/2026-09-08-872-refactoring-campaign-phase-4.md`。比較起点と初期signalは `xtask/refactoring-audit-baseline.json`、形状は `docs/schemas/refactoring-audit-baseline.schema.json`。継続checkは `cargo xtask refactoring-audit-check`、週次／manualの発火判定と監査Issueの集約は `Kukuri Refactoring Audit`。実行・復旧・review付きbaseline更新は `docs/runbooks/dev.md` の「リファクタリング監査の発火要否」、#873の証跡は `docs/progress/2026-09-08-873-refactoring-audit-trigger.md`。
- GitHub Issue / PRは追跡面。既存仕様と今回の変更要求の区別は次節に従う。

## 正本と変更要求

人間とAIの共通入口は root `AGENTS.md`。`CLAUDE.md` と `kilo.json` はその読み込み設定、`AGENTS.local.md` は任意の個人設定である。個人設定の欠落だけで停止せず、共通の品質条件や製品契約を個人設定で黙って変更しない。

| 判断すること | 正本 |
| --- | --- |
| 製品・protocol・データ境界の仕様 | 関連する `docs/adr/`。UIの製品・視覚契約は root `DESIGN.md` |
| 実装済みの挙動 | 現行実装と tests / contracts / `harness/scenarios/` |
| Issue工程、承認、再現、リスク別記録、独立監査、Close | `docs/runbooks/issue-lifecycle.md` |
| 計画の粒度と形式 | root `PLANS.md` |
| リファクタリング境界、path別検証の選定・中断 | root `REFACTORING.md` |
| コマンドの実行方法・環境差 | `docs/runbooks/dev.md` |
| UI作業の事前成果物・確認・例外 | `docs/adr/0014-uiux-dev-flow.md` |
| CSS・state・dataの実装配置 | `docs/architecture/desktop-ui-implementation.md` |
| UI採用記録のschema・履歴管理 | `docs/ui-reviews/README.md` |
| 現状と個別の作業証跡 | `docs/progress/`。過去の計画・監査・UI記録は当時の証拠であり、現行規則を上書きしない |

Issue・PR・セッションの記述だけで「実装済み」「検証成功」と判断しない。一方、ユーザーが今回承認した変更要求は作業範囲と判断権限の根拠であり、既存仕様と異なることだけを理由に拒否・再承認しない。変更要求を対応する正本文書・実装・testsへ反映し、変更前の事実と区別する。

規則が食い違う場合は、適用対象と上表の責務を確認する。読む順番、日付の新しさ、強い表現だけで優先順位を決めない。承認済み範囲内の参照漏れ・要約・雛形は担当者が同期し、製品契約や依頼範囲を変える未承認の判断だけをユーザーへ確認する。

入口の短い要約は正本へのリンクとともに維持し、正本の変更時に参照元・雛形・例を同じ差分で確認する。機械検査されるミラーは同期検査を維持する。規則の見直しでは観測した効果・負担・環境変化を根拠に維持・強化・緩和・統合・撤廃を選び、理由を作業記録へ残す。過去の判断本文は書き換えず、再流入する経路に後継先や失効範囲を示す。

## Ops
- Windows x64 MSIXのbuild、identity、local署名、WACK、Microsoft Store提出: `docs/runbooks/windows-microsoft-store.md`
- Dome Hosting の有効化・割当・終了・split-brain復旧: `docs/runbooks/dome-hosting.md`
- Dome prop、layout commit、manifest/asset保持: `docs/adr/0040-dome-prop-layout-retention.md`
- Metaverse resource budget: `docs/adr/0041-metaverse-resource-budget.md`
- Spatial Context entryとauthoritative safe spawn: `docs/adr/0044-spatial-context-entry-safe-spawn.md`
- Dome offline、Connection draining、Return Home: `docs/adr/0045-dome-offline-draining-return-home.md`
- community node production rollout / live media verification / recovery: `docs/runbooks/community-node-production-rollout.md`
- 専用OpenAI Moderation・動画抽出・共有内容cacheの運用: `docs/runbooks/community-node-openai-moderation.md`
- community node GCP Terraform デプロイ（deployment profile: low-cost / managed-db / ha）: `docs/runbooks/community-node-gcp-terraform.md`（実装は `infra/terraform/`）
- community node 権利侵害申出の受付・審査・送信防止: `docs/runbooks/community-node-rights-infringement-requests.md`
- community node 発信者情報開示・案件限定保全: `docs/runbooks/community-node-sender-information-disclosure.md`

## Architecture
- P2P-first community node の責任境界: `docs/architecture/p2p-first-community-node-responsibility-boundary.md`（operator docs / safety / report routing の共通前提）
- desktop UI のCSS / state / data配置: `docs/architecture/desktop-ui-implementation.md`（製品・視覚契約はroot `DESIGN.md`、開発フローはADR 0014）
- desktop の blob キャッシュと取得の再試行: `docs/architecture/blob-cache.md`（保持範囲、取得経路と I/O、自動再試行の回数・リセット条件、既知の欠落）
- タイムラインの反映と復旧を総件数に依存させない設計: `docs/adr/0052-scale-independent-timeline-sync.md`（#1239。同期は best effort、反映は個別反映・窓の追いつき・遡りの取得の 3 つ、docs の読み出しは key の索引と上限つき、replica の時間分割の方向）
- アカウントの署名鍵から docs author を導出し、著者の record を docs author と key の組で読む: `docs/adr/0053-account-derived-docs-author.md`（#1258。同じ key に不正な record を積まれても、著者の投稿と取り下げを定数の読み出しで反映する）
- replica の読み出しの inventory: `docs/architecture/replica-read-inventory.md`（prefix の全件読みの全 caller、比例する総数、解消する段階）
- Linux GUI配布とCLIのローカル制御経路: `docs/adr/0049-linux-gui-cli-control-plane.md`（#885。CLI専用profile、常駐プロセス／IPC、command登録簿、要求単位の実行、配布成果物のデータ分類）
- moderation event / safety advisory の trust semantics + deterministic (CSAM / known-hash) critical safety: `docs/adr/0027-deterministic-moderation-critical-safety.md`（optional trust input であり network-wide command ではないことを固定。旧 `community-node-critical-safety.md` / `moderation-event-trust-semantics.md` を集約）
- community node trust / relation foundation: `docs/adr/0026-community-node-trust-relation-foundation.md`
- 非決定論的（VLM）moderation と general 判定の content advisory 化（#1051）: `docs/adr/0028-nondeterministic-moderation-vlm.md`（§8。nsfw / objectionable は index + advisory、trust 寄与 0。trust 側は ADR 0026 §7、index 側は ADR 0025 §7、client 側は ADR 0046 §6）
- default community node 依存低減ロードマップ: `docs/architecture/default-community-node-dependency-reduction.md`（default node は onboarding infrastructure であり network-wide authority ではない）

## Legal
- app-level 利用規約 / プライバシーポリシー（client 自体への同意。per-node consent とは別建て）: `docs/legal/terms-of-service.md` / `docs/legal/privacy-policy.md`（canonical SSoT。アプリ内表示は i18n `legal` namespace がミラー）
- app-level 外部送信表示と実装突合表: `docs/legal/external-transmission-notice.md` / `docs/legal/app-data-flow-inventory.md`
- feature data classification: `docs/legal/app-consent-data-classification.md`
- Community Node 法務文書と per-node 同意: `docs/legal/community-node-legal-documents-data-classification.md`（運用手順は `docs/runbooks/community-node-operator-docs.md`）
- 18歳以上の自己申告と成人向け表現の既定非表示(#858): 仕様は `docs/adr/0046-age-attestation-adult-content-gating.md`、分類は `docs/legal/age-attestation-data-classification.md` / `docs/legal/adult-content-display-data-classification.md`
- 端末バックアップ / 復元(#855): 仕様・脅威モデルは `docs/adr/0048-device-backup-restore.md`、移行対象分類は `docs/legal/device-backup-data-classification.md`
- 開発者向けアプリ内ログの閲覧・書き出し(#978): 分類は `docs/legal/developer-log-viewer-data-classification.md`、外部送信一覧の行は `docs/legal/app-data-flow-inventory.md`
- 公開投稿の外部URL link／OGP preview(#1174): 取得・SSRF・cache境界は `docs/adr/0051-link-preview-fetch-boundary.md`、分類は `docs/legal/link-preview-data-classification.md`、外部送信一覧は `docs/legal/external-transmission-notice.md` / `docs/legal/app-data-flow-inventory.md`

## UI/UX
- flow: `docs/adr/0014-uiux-dev-flow.md`
- visual spec: `DESIGN.md`（root）
- migration plan: `docs/progress/2026-03-24-shell-ui-production-migration.md`
- accepted review records: `docs/ui-reviews/`

## 告知素材・LP
- 制作仕様の正本（配布候補の事実、JA/EN原稿、shot list、固定出力一覧、公開先）: `docs/progress/2026-09-15-promo-lp-brief.md`
- 撮影（Playwright）とレンダリング（Remotion）の実行手順: `docs/runbooks/promo-production.md`（実装は `tools/promo/` と `apps/desktop/tests/promo/`）
- LP（`kukuri.app`、実装は `apps/lp/public`）の確認と Cloudflare Pages への公開: `docs/runbooks/lp-publish.md`

# AGENTS.md

このファイルは詳細仕様ではなく、現行の kukuri 実装で作業するための短いポインタです。

## まず読む
- `AGENTS.local.md`（存在する場合だけ読む個人設定。欠落時は継続し、作成・変更しない）
- `docs/README.md`
- `docs/runbooks/dev.md`
- `docs/runbooks/issue-lifecycle.md`（Issueの起票・計画・実装・監査・Close・Reopenを行う場合）
- `PLANS.md` (プランモード・プラン作成時)
- `DESIGN.md`（UI/UX 作業時のビジュアル仕様。フロー/ガードレールは `docs/adr/0014-uiux-dev-flow.md`）
- `REFACTORING.md`（リファクタリング・構造整理・大きめの移動/抽出を行う場合）

## 作業対象
- 新規実装・修正は原則 root workspace の現行実装のみ。
- 現行スコープの参照順と規則の正本は `docs/README.md` に従う（現行アクティブマイルストーンは builder preview）。
- builder preview / 配布 / 初回体験は `docs/progress/2026-04-16-mvp-builder-preview-plan.md`。
- その capability baseline は `docs/progress/2026-03-10-foundation.md`。
- Windows desktop support、seeded DHT discovery、community-node connectivity/auth、social graph v1、private channel audience v1 は current scope に含まれる。

## 実行入口
- `cargo xtask doctor`
- `cargo xtask check`
- `cargo xtask test`
- `cargo xtask e2e-smoke`
- frontend 単体操作: `cd apps/desktop && npx pnpm@10.16.1 <install|dev|test>`

## 真実の置き場所
- 仕様: `docs/adr/`
- 実行手順: `docs/runbooks/`
- 現状: `docs/progress/`
- ビジュアル仕様: `DESIGN.md`
- UI/UX フロー・ガードレール: `docs/adr/0014-uiux-dev-flow.md`
- UI review record: `docs/ui-reviews/`
- 振る舞い: `crates/*` のテストと `harness/scenarios/`
- 既存仕様・実装済みの事実は repository の実装・docs・tests・scenarios で確認する。今回承認された変更要求は作業範囲の根拠として扱い、変更時に対応する正本へ反映する。詳細は `docs/README.md` の「正本と変更要求」を参照する。

## 設計原則: 件数に依存しない処理
kukuri は不特定多数が参加する P2P SNS であり、1 つの topic の投稿・peer・author は上限なく増える。設計・実装・review では次を前提にする。
- 「今は件数が少ないから足りる」「N 件までは足りる」で判断しない。どれだけ件数が増えても利用者に不利益が無い、非同期的な処理を原則とする。
- 取りこぼしゼロの全件取得は不可能であり、目標にしない。ユースケース上ユーザーが必要としない限り同期・復旧はしない。欠けている状態でも表示と操作が成立するようにする。
- 処理量・メモリ・通信量が、topic / replica / 台帳の総件数に比例する経路（全件の読み込み・deserialize・書き直し・ソート・再 sync・再購読）を作らない。差分、event 駆動、cursor、索引、上限つきの窓で組み、既存の該当経路は欠陥として扱う。頻度を下げるだけでは解決としない。
- 利用者の操作と画面表示の経路に、件数に比例する待ちを置かない。重い処理は背景で小分けに進め、途中で止まっても再開できるようにする。
- 台帳・cache・task・購読は、上限と削除を持つ。
- 計測は「足りる」の根拠にせず、件数に対する増え方を示すために使う。完了条件は、件数を増やしても処理量と待ち時間が増えないこととする。

## ガードレール
- 既存コードの丸ごとコピーは禁止。contract または scenario を先に置いてから必要最小限だけ移植する。
- 不具合は修正前に再現する。自動化可能な挙動の失敗testと、実機・視覚でしか確認できない場合の扱いは `docs/runbooks/issue-lifecycle.md` の「修正前の再現」を参照する。
- Issue作業の承認、リスク別の必須記録、独立監査、Close条件は `docs/runbooks/issue-lifecycle.md` に従う。
- リファクタリングの変更境界、変更pathごとの必須validation、重い検証の選定・中断は `REFACTORING.md` に従う。
- root に新しい長文ドキュメントを増やさない。必要なら `docs/` に置く（例外: ビジュアル仕様 `DESIGN.md` ）。
- CI は検証の場ではない。実装中の確認はローカルで CI と同じ内容を実行し、CI は PR 作成後の最終確認だけに使う。手順は `docs/runbooks/dev.md` の「ローカル先行検証」に従う。
- `console.error` は使わない。
- コミットはユーザーの依頼範囲で行う。PR作成・マージの依頼には、そのために必要なコミットを含む。承認範囲の判断は `docs/runbooks/issue-lifecycle.md` に従う。

## 調査ツール
- `.codegraph/` がある場合、コード探索は CodeGraph の MCP または `codegraph explore` / `codegraph node` を優先する。利用不能ならその旨を記録し、`rg` とファイル読みへ切り替える。index の新規作成はユーザーの判断とする。
- `.codegraph/` がなければ通常の検索を使う。文書・設定などindex対象外のファイルは直接読んでよい。

## 通信経路
- 本プロジェクトの基本優先度は `Direct P2P -> Relay Supported P2P -> Relay Fallback`。
- `Direct P2P` は manual ticket / `addr_hint` / DHT などの直接到達情報で接続し、relay URL を候補に含めない経路。
- `Relay Supported P2P` は topic rendezvous / discovery / hole punching / endpoint assist に community-node や relay を使い、同じ topic を subscribe している client 同士の P2P 接続を成立させる経路。これは fallback ではない。
- `Relay Fallback` は Direct P2P と Relay Supported P2P が成立しない場合だけ、gossip/docs/blob など実データを含む通信が relay 経由になる経路。
- `cn-user-api` は topic rendezvous state の owner。topic presence は Valkey/Redis-compatible KV に TTL 付き ephemeral state として置き、`cn-iroh-relay` は純粋な iroh relay のままにする。
- relay-only の実装やテストは通常成功経路として扱わず、`Relay Fallback` として明示する。

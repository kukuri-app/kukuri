# AGENTS.md

このファイルは詳細仕様ではなく、現行の kukuri 実装で作業するための短いポインタです。

## 作業原則

作業開始前に完了範囲を閉じ、その範囲を満たす最小の最終実装を作る。
本節は設計原則とは別に、作業自体の開始・選択・終了を制御する。作業に関する個別のルール・条件・分類は、本原則に従う。

### 1. 作業開始前に、完了を判定できる状態にする

目的を有限個の受入条件へ分解し、それぞれの対象・期待結果・判定方法を明示する。有界に判定できない条件は子項目へ分解し、実装開始前に末端の条件まで明示的に評価できる形にする。

未解決の設計・調査事項がある場合は、それ自体に終了条件を置く。子項目の追加で親の完了を先送りし続けてはならない。実装中の発見によって受入条件を増やす必要が生じたら、作業を続けながら拡張せず、先に完了範囲を再定義する。

### 2. 受入条件を満たす中で、最終的なコード量を最小にする

目的の達成を制約として、完成後に残るコード行数の最小化を設計の最優先事項とする。「変更差分が小さい」「既存構造を温存できる」「既に工数を投入した」は選定上の優先事項にしない。

既存処理の置換・統合・削除まで含めて最終形を比較する。同種の問題を個別経路へ繰り返し実装する場合は、共通の責務へ集約できるかを見直す。将来の利用を想定した汎用化も、最終コード量を増やすなら採用しない。受入条件に必要性を結び付けられない未使用基盤や新旧経路の併存は、完成形に残さない。

行の圧縮や別ファイルへの移動による見かけの削減ではなく、実装・テスト・補助コードを含む保守対象の総量で評価する。

### 3. 明示的に依頼されていないエッジケースは扱わない

合意した利用場面・入力・状態・失敗条件の外側は、対応対象へ含めない。「起こり得る」「将来必要になる」「監査で指摘された」という理由だけでは追加実装しない。

合意済みの利用場面を成立させない不具合は、受入条件の未達として扱う。それ以外の状況への対応には明示的な指示が必要である。監査にも同じ境界を適用し、監査を新しい要件の発生源にしない。

計画・Issue分割・PR・テスト・監査・進捗報告はすべて手段である。必要な受入条件を満たした時点で終了し、「さらに改善できること」の存在を継続理由にしない。個別のルール・条件・分類は、この作業原則を基準に見直す。

### 4. 受入条件・利用者体験・後方互換性に関わる変更は、ユーザーの判断を確認してから進める

次のどれかに当たる変更は、着手前にユーザーへ選択肢と影響を示し、判断を得てから進める。設計・ADR・実装・監査の指摘への対応のどの段階でも同じで、自分で決めてから報告する形にしない。

- 受入条件の変更（追加・削除・緩和、言い換えによる範囲の変更）。
- 利用者体験への影響（機能・表示・操作が変わる・減る・増える）。件数の上限による打ち切り、非対応とする機能、保留や待ちの発生を含む。
- 後方互換性への影響。保存データ・backup・wire・API・CLI の出力の形や意味が変わり、旧版のデータや利用者が読めなくなる・挙動が変わるもの。

設計原則（件数に依存しない処理など）を満たすための制限も、利用者に見える影響があれば同じ扱いとする。判断を待つ間は、影響の無い作業だけを進める。

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
- 仕様: `docs/adr/`（現行の設計原則と矛盾する既存ADRは改訂対象）
- 実行手順: `docs/runbooks/`
- 現状: `docs/progress/`
- ビジュアル仕様: `DESIGN.md`
- UI/UX フロー・ガードレール: `docs/adr/0014-uiux-dev-flow.md`
- UI review record: `docs/ui-reviews/`
- 振る舞い: `crates/*` のテストと `harness/scenarios/`
- 既存仕様・実装済みの事実は repository の実装・docs・tests・scenarios で確認する。今回承認された変更要求は作業範囲の根拠として扱い、変更時に対応する正本へ反映する。詳細は `docs/README.md` の「正本と変更要求」を参照する。

## 設計原則: 件数に依存しない処理
kukuri は不特定多数が参加する P2P SNS であり、1 つの topic の投稿・peer・author は上限なく増える。設計・実装・review では次を前提にする。

既存ADRには、この原則の制定・更新前の設計も含まれる。ADRと現行の設計原則が衝突した場合は原則を優先し、ADRの維持を理由に原則違反の処理を実装・正当化しない。利用者向け要件を保ちながら原則に沿う設計へ改め、ADRの該当箇所に新しい判断と旧案の失効範囲・移行条件を記録する。両立に製品要件の変更が必要な場合だけユーザーの判断を求める。

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
- ローカルの検証は変更関連箇所に限定し、全体確認は PR CI で行う。CI に含まれない必要な検証は対象と実行先を明示する。手順は `docs/runbooks/dev.md` の「ローカル先行検証」に従う。
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

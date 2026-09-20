# REFACTORING.md

リファクタリングIssueも、一般の起票・計画・PR・監査・Closeについては `docs/runbooks/issue-lifecycle.md` に従う。本書はそれに加えて、挙動維持、凍結境界、path別validationを規定する。

この文書は、AIエージェントと人間が kukuri でリファクタリング作業を行うときのルールを定義する。

この文書の構造調査・凍結境界・成果物の規定はリファクタリングに適用する。「path別検証マトリクス」と「検証の選定・中断」は他種別の変更でも使う。文書だけの修正にコードの構造調査や分割計画を要求しない。

## リファクタリングモード

リファクタリング作業では、機能追加・仕様変更・依存更新を混ぜない。

リファクタリングとは、外部挙動を維持したまま、内部構造・命名・責務境界・重複・ファイル構成を改善する作業である。

外部挙動を変更する場合は、それは refactor ではなく feature / fix / migration として扱う。

## 非目的

- リファクタリング作業を使ってプロダクト挙動を追加しない。
- タスクで明示されない限り、CI の必須 / 任意 job 設定を変更しない。
- ついでの整理として crate 分割や大型ファイル分割を行わない。

## 基本原則

- 1 PR = 1意図。
- 挙動変更は構造整理と分離する。rename / move / extractionは一つの構造上の成果に必要な場合にまとめてよいが、差分上で機械的な移動と責務変更を追えるようにする。独立して検証・差し戻しすべき変更は別PRにする。
- 明示指示なしに public API、protocol object、storage schema、docs/blobs canonical source、community-node endpoint contract を変更しない(具体的な対象は「凍結境界」の章)。
- 既存テストを削除しない。削除が必要な場合は、削除理由と代替カバレッジを示す。
- 振る舞いが変わる可能性がある場合は、先に characterization test / contract / scenario を追加する。
- 大きな抽象化を導入する前に、現在の責務境界と呼び出し方向を調査する。
- 便利な境界横断の共通化より、明確な境界を保つことを優先する。

## リファクタリングで達成すること

リファクタリングの目的は、単にコードを整形したり行数を減らしたりすることではなく、外部挙動を
維持したまま、次の変更に必要な理解・変更箇所・波及範囲と回帰リスクを小さくすることである。
候補ごとに、次のうち何を改善するかを具体的に示す。

- 責務の所有者と依存方向を明確にする。
- 同じ状態、判断、変換の正本を一つにする。
- 必要以上に広い公開面、境界横断、暗黙の結合を縮小する。
- 同一責務内で意味が同じ重複を統合する。
- I/O、時刻、乱数、network、永続化などの副作用を、挙動を検証できる境界へ隔離する。
- 到達不能な処理、未参照経路、sunset 条件を満たした互換経路を、入口・参照・契約を確認して除去する。
- 名前、型、配置、文書を現在の責務とリポジトリの現行規約へ一致させる。

次は候補発見の signal にはなり得るが、単独では着手理由にしない。

- 見た目や記述様式が好みと異なること。
- 行数、ファイル数、複雑度、coverage など一つの数値だけが閾値を超えたこと。
- 一般的な best practice と異なっていても、kukuri で変更摩擦や回帰リスクが観測されていないこと。
- 将来必要になるかもしれない抽象化を先回りして作ること。
- 変更予定も圧力もない安定領域を、全体統一のためだけに書き換えること。

監査の正当な結論として「現時点では変更しない」を認める。観測した問題、構造上の成果、挙動維持の
証拠を具体化できない候補は着手しない。

## 定期監査と個別実行

定期監査は原則として product code を変更せず、有限の範囲で候補を収集し、根拠を確認して分類する。
監査結果をそのまま一つの大型リファクタリング PR にしてはならない。実施する候補は、一つの構造上の
成果を持ち、単独で検証・review・差し戻しできる個別 Issue / PR として扱う。

### 監査を開始する trigger

固定した週次・月次で repository 全体を掃除するのではなく、次のいずれかを観測したときに開始する。

- 大規模な feature または milestone が完了した。
- release の安定化へ入る前である。
- 次の変更を現構造が明確に妨げている。
- 同種の不具合、review 指摘、変更漏れ、変更時の摩擦が繰り返された。
- 複数の実装が重なった領域で、局所的な別実装や二重 state が増えた。
- `cargo xtask oversized-files` など既存 ratchet の基準値増加や、責務境界からの逸脱を観測した。
- 互換経路、feature flag、一時実装の明示済み sunset 条件を満たした。

### 監査の有限化と成果物

開始前に、一つの subsystem、直近の変更領域、または具体的な責務境界へ scope を限定し、時間枠または
確認する入口・symbol・path の集合を固定する。成果物は、根拠付き候補一覧、優先順位、各候補の分類と
理由、今回着手する候補、必要な個別 Issue とする。

各候補は次のいずれかへ分類する。

- `実施`: 開始条件を満たし、個別 Issue / PR へ進める。
- `延期`: 問題は観測できるが、変更圧力、検証網、時期の条件が未充足である。
- `却下`: 構造上の成果を示せないか、変更コスト・回帰リスクが価値を上回る。
- `別種別`: 挙動不良、新機能、依存更新、protocol / storage 変更、性能改善、文書だけの不一致として
  `fix` / `feature` / `deps` / `migration` / `docs` などへ分離する。

固定 scope の候補をすべて分類した時点、または事前に決めた時間枠を使い切った時点で監査を終了する。
PR が一つも発生しなくても正常な完了とする。

### 候補に必要な根拠

| 観点 | 確認対象 | 着手判断に必要な根拠 |
|---|---|---|
| 不要物・互換経路 | dead code、未参照 export、古い flag、一時経路、古い comment | 参照検索、runtime の入口、sunset 条件、関連 test / contract。serde / IPC / macro / config / 動的登録 / generated code も確認する |
| 重複・正本 | domain rule、validation、key 生成、state 更新、projection の二重実装 | 各実装の利用者と責務所有者、意味が同一である証拠、変更漏れや摩擦の実例 |
| 責務・依存 | 複数責務を持つ module、境界横断、循環依存、広い公開面 | 現在と目標の責務・依存、変更波及の実例 |
| 制御・副作用・検証可能性 | 深い分岐、隠れた state、順序依存、副作用との混在 | 維持する transaction / retry / cancel / lifecycle / concurrency / P2P 順序、error semantics、保護する test / scenario |
| 現行規約とのずれ | 共通機構を使わない局所実装、古い実装様式、命名・型・配置のばらつき | repository 内の正本と現行の模範実装。一般論だけを根拠にしない |
| 永続的な知識 | 実装と ADR / runbook / comment の不一致、検証手順の欠落 | 現行実装、test、repository 内文書。session-local な資料へ依存しない |

### 選定、開始、停止

優先順位は、変更頻度、今後の具体的な変更予定、開発・review の摩擦、不具合や互換性への波及、
検証可能性、実施コスト、差し戻しやすさを合わせて判断する。数値は候補発見の signal として使い、
数値だけで自動選定しない。

個別作業は、次をすべて説明できる場合だけ開始する。

- 観測した問題と、それを示す path、symbol、参照、履歴、test などがある。
- 現在の責務、依存、外部挙動、凍結境界、互換パスを特定している。
- 目標とする構造上の成果と、その before / after の成功判定がある。
- 維持する挙動と、それを証明する最小の validation がある。
- 対象と対象外が固定され、一つの意図として review・検証・差し戻しできる。

次のいずれかが判明した場合は作業を止め、再調査または別種別へ再分類する。

- 現在の挙動または正本を特定できない。
- 凍結境界、protocol、storage、public API、利用者に見える挙動の変更が必要になった。
- validation で挙動維持を十分に証明できない。
- 目的が `fix` / `feature` / `deps` / `migration` / 性能最適化へ変わった、または required CI の変更が必要になった。
- 当初の構造改善が不要と分かったか、具体的な成果を示せなくなった。
- 一つの意図として review・差し戻しできる範囲を超えた。

### 個別作業の成果物ベースの完了条件

個別作業は次をすべて満たしたときに完了とする。

- 観測した問題と、実施前後の構造上の変化が path、symbol、依存、責務などの証拠へ対応している。
- 状態の正本数、重複経路、依存、公開 symbol、境界横断、未参照経路など、目的に合う指標が改善している。
- public API、protocol、storage、config、event、UI など対象領域の外部挙動が維持されている。
- 変更前後の必要な validation が記録され、既知の失敗を除いて新しい failure / warning がない。
- test / contract / scenario の削除、skip、弱体化で成功させていない。
- code の移動や分割だけでなく、定義した責務、依存、正本の問題が実際に改善している。
- 必要な ADR、runbook、comment、本書が現行実装と一致している。
- PR が一つの意図に限定され、単独で review・差し戻しできる。
- 対象外と残存候補が分類され、必要な別種別の作業は混ぜずに別 Issue へ分離されている。

「行数が減った」「ファイルを分割した」「coverage が上がった」「AI が成功と報告した」だけでは
完了としない。

## PR種別

PRタイトルまたはタスク概要では、以下のラベル / prefix のいずれかを使う。

- `refactor:rename`: 名前変更のみ。ロジック変更禁止。
- `refactor:move`: ファイル移動のみ。ロジック変更禁止。
- `refactor:extract`: 関数・型・モジュール抽出。外部挙動変更禁止。
- `refactor:boundary`: crate / module 境界整理。先に計画を書く。
- `refactor:delete`: dead code 削除。参照経路調査を添える。
- `contract`: 仕様固定テスト追加。実装変更禁止。
- `scenario`: harness scenario 追加/更新。プロダクト実装変更は別PR。
- `fix`: バグ修正。修正前の証拠は `docs/runbooks/issue-lifecycle.md` の「修正前の再現」に従う。
- `deps`: 依存更新。リファクタリングと混ぜない。
- `docs`: ドキュメント更新。実装変更と混ぜる場合は理由を書く。

PR を作成または説明するときは、タスク種別を明確にする。リファクタリングPRに機能追加や挙動変更を隠さない。

推奨 PRタイトル prefix:

```text
[codex][refactor:rename] ...
[codex][refactor:extract] ...
[codex][refactor:boundary] ...
[codex][refactor:delete] ...
[codex][contract] ...
[codex][scenario] ...
[codex][fix] ...
[codex][deps] ...
[codex][docs] ...
```

PR共通欄は `.github/PULL_REQUEST_TEMPLATE.md` を使い、本書の「記録テンプレート」にある構造上の成果・挙動維持の証拠へリンクする。テンプレートを別々に複製しない。

## 禁止する混在変更

以下を同じPRに混ぜない。

- rename + logic change
- file move + behavior change
- dependency update + refactor
- storage migration + UI refactor
- protocol shape change + internal cleanup
- test deletion + implementation change without replacement
- formatting-only change + semantic change

## 凍結境界(変更 = 挙動破壊)

以下は「1 バイトの変更で既存データ・既存署名・ネットワーク互換・契約が壊れる」凍結対象である。
明示的なタスク(migration 計画込み)なしに変更しない。多くは contract テストが検出装置として
固定している(fail した場合、テストではなく変更側の互換影響を評価する)。
判断に必要な根拠と検出 test は本節へ集約し、repository 外や非追跡の計画を前提にしない。

1. **署名 canonical 3 系統**: envelope の 6 要素配列(`crates/core/src/envelope.rs` の
   `canonical_envelope_payload`)、DM frame/ack の canonical 配列(`crates/core/src/direct_messages.rs`)、
   moderation event のキー辞書順 canonical(`crates/cn-safety/src/event.rs`)。
   固定: `crates/core/src/tests/signing_canonical.rs` / `crates/cn-safety/tests/domain_model.rs` /
   `crates/cn-safety-runtime/tests/signer.rs`。
2. **rendezvous / 派生文字列**: replica 命名と `kukuri-docs:` 派生(`crates/docs-sync/src/replicas.rs`)、
   gossip topic id(`crates/transport/src/iroh/topics.rs` の `topic_to_gossip_id`)、
   rendezvous ドメイン(`crates/core/src/rendezvous.rs`)、DM の HKDF/AAD ドメイン群、
   wire prefix 定数(`crates/core/src/wire.rs`)。
   固定: `crates/core/src/tests/derivation_golden.rs` / `wire_constants.rs`、
   `crates/docs-sync/src/tests/replicas.rs`、transport の `topic_to_gossip_id_matches_golden`。
3. **serde フィールド名・variant 名 = wire 形式**: `GossipHint`(externally-tagged、受信側は
   デコード失敗を黙殺)、docs エントリ値の `KukuriEnvelope` / `*DocV1` 群、envelope kind/tag 文字列。
   固定: `crates/core/src/tests/wire_snapshot.rs`。
4. **`PayloadRef` の PascalCase タグ**: 周囲の snake_case と不整合だが署名済み content と
   SQLite に永続済み。「統一リファクタ」は全既存データの破壊。固定: 同上 wire_snapshot.rs。
5. **docs エントリ key スキーム**(`objects/{id}/state` 等): app-api の `stable_key` 生成と
   cn-indexer の prefix 走査の暗黙契約。固定: cn-indexer の ingestion_contracts(常時実行)。
6. **永続スキーマと identity**: SQLite / Postgres の migration 済みスキーマ、identity ファイル群
   (keyring account は db パス依存 — パス変更 = 鍵ロスト)、endpoint secret(iroh::SecretKey の
   serde 表現に直結)。固定: migration テスト(拡充は後続 WP)。
7. **community-node HTTP 契約と manifest**: cn-user-api の endpoint 群と `CommunityNodeManifest`
   (server 完全版 ⇔ client slim 版)。変更は後方互換な optional フィールド追加のみ可。
   固定: `crates/cn-user-api/tests/`、round-trip は
   `crates/desktop-runtime/src/community_node/manifest_support.rs` +
   `crates/cn-operator/tests/manifest_golden.rs`(**この 2 ファイルを触る PR は
   `cargo xtask rust-test` の round-trip テストと `cn-test` の golden の両方を実行する**)。
8. **cn-operator の capability 提供状態と公開条件**: `community_index` / `moderation` /
   `community_local_trust` は #616 の readiness / fail-closed gate を経て #617 で
   `Availability::Available` へ移行済み（[ADR 0025](docs/adr/0025-community-node-indexing-foundation.md)
   2026-08-16改訂、`crates/cn-operator/src/capability.rs::availability`）。提供可能であることと
   個々の配備で有効・公開であることは区別し、設定と有効な readiness 記録の条件を維持する。
   ADR 0027 §2.9 の昇格条件は当時の判断として保持する。将来向けの `Planned` 区分を削除せず、
   リファクタで availability、manifest の表明、operator の有効化条件を変えない。
9. **Tauri IPC 契約**(`crates/app-api/src/views.rs` ⇔ `apps/desktop/src/lib/api/types.ts`):
   同一バイナリ内契約のため両側同時変更なら改名可能だが、片側変更は silent break。
   固定: 高頻度 8 型グループは共有 fixture(apps/desktop/src/lib/api/__fixtures__/views/)を
   Rust 側 crates/app-api/src/tests/views_wire_snapshot.rs と TS 側 viewsContract.ts(+ .test.ts)が
   両側から検証する(再生成手順は各ファイル冒頭)。入力方向(requests.rs ほかの request DTO)は
   WP-B6 で codegen 対象になり、runtimeApi.ts の request literal が生成型への `satisfies` で
   拘束される(再生成 diff + tsc の二重検出)。CommunityNode 系 view の fixture 化は未。

## 地雷リスト(直したくなるが、してはいけない)

凍結境界とは別に、「一見 dead code / 不整合 / 冗長に見えるが、消す・揃える・golden 化すると
壊れる」ものを挙げる。判断に必要な理由と検出経路は各項目へ集約し、repository 外や非追跡の
計画を前提にしない。

1. **`ConnectionPath::RelayFallback` は削除禁止の第3経路**: `AGENTS.md`「通信経路」節が規定する
   `Direct P2P -> Relay Supported P2P -> Relay Fallback` の 3 番目。variant は
   `crates/transport/src/config.rs` に定義され、TS 側表示分岐(生成型
   `apps/desktop/src/lib/api/types.generated.ts` / `apps/desktop/src/shell/presentation.ts` の
   `relay_fallback`)も実在する。判定は実装済み(issue #573):
   `crates/transport/src/iroh/peer_state.rs` が snapshot 時に `Endpoint::remote_info` の
   active transport addr を観測し、topic の全 connected peer が relay 経由でしか
   実データを流せない場合に RelayFallback と `fallback_peer_ids` を報告する
   (実 relay 検証は `crates/transport/src/iroh/tests/relay_connectivity.rs` の
   relay-only テスト)。**variant は削除・改名しない**。serde 表現(`relay_fallback`)は
   IPC 契約であり、既存 2 経路(direct / relay_supported)の判定を変える変更は
   characterization テスト(`peer_state.rs` の unit test)を先に更新しない限り禁止。
2. **`apps/desktop/src/styles/shell-scoped-overrides.css`は現役の本番 CSS**:
   portalとshellで意図した実効値差がある。重複という見た目だけで削除しない。
   配置・cascadeと統合時の確認は[UI実装配置](docs/architecture/desktop-ui-implementation.md)の
   「Portalとscoped override」、実行する検証は本書のマトリクスを参照する。
3. **署名バイトは非決定的なので golden 化しない**: `KukuriKeys::sign_schnorr`
   (`crates/core/src/crypto.rs`)は毎回 aux_rand を生成するため署名バイトは実行毎に変わる。凍結対象は
   canonical バイト列と「過去 fixture の verify 継続」であり、署名バイトを snapshot golden にすると
   毎回 fail する(理由は `crates/core/src/tests/signing_canonical.rs` の doc）。

## 互換パスと sunset 条件

以下は後方互換のために残している経路で、「いつ削除してよいか」を明文化せずに消すと既存データ・
既存ユーザーが壊れる。撤去条件は WP-C8(2026-07-07)で確定した。条件を満たすまで削除しない
(このリストが「消してよいか」の判断入口。コード側の該当箇所にも同じ条件をコメントで明記している)。

| 互換パス | 所在 | 誤削除の影響 | sunset 条件(WP-C8 で確定) |
|---|---|---|---|
| 旧 `.nsec` 鍵ファイル読込 | `crates/desktop-runtime/src/identity.rs`(`legacy_key_file_path`。テスト `legacy_nsec_file_still_loads` が固定) | **利用者が気づかないまま別人の鍵になる**: 旧ファイルは読み込んでも新形式へ再保存されず残り続け、鍵が見つからないと `load_or_create_keys` が黙って新しい鍵を生成するため | **撤去する際は、`.nsec` ファイルを検知したら起動を止めて案内を出す処理(fail-loud)とセットで行うこと**。黙って新しい鍵を生成する現状のまま読込パスだけを消してはならない。鍵は preview 段階でも黙って失ってよいものではない |
| epoch `"legacy"` 互換 | `crates/app-api/src/service/projection_support.rs`(`legacy_epoch_id` / `private_channel_replica_for_epoch` / `joined_private_channel_state_from_capability`) | epoch 導入前に保存されたプライベートチャンネル capability(epoch_id が空)のチャンネル履歴が読めなくなる。現行ビルドが空の epoch_id を新規に書くことはない | **正式リリースの節目で削除可**(preview データの保全は保証しない方針)。削除時は空の epoch_id を持つ capability をエラーで拒否すること(黙って読めなくならないようにする) |
| ~~CRLF checksum 自己修復~~ | ~~`crates/store/src/sqlite/connection.rs`~~ | - | **撤去済み**(2026-07-06、WP-C6 / PR #485)。checksum 不一致は fail-loud で起動失敗し、回帰テストで固定済み |

**互換パスではないと再分類したもの(WP-C8):**

- `resolved_urls` の未解決時の扱い(`crates/desktop-runtime/src/community_node/requests_support.rs`。
  認証時に base_url で代用する fallback と、解決されるまで再取得を繰り返す判定)は、旧設定の互換では
  なく**恒常経路**。コミュニティノードを新規追加した直後は必ず未解決(None)であり、この経路が初回接続
  そのものを支えている。削除対象ではないため sunset 条件は持たない。

## 真実の置き場所ルール

正本、読む順番と優先関係、承認済み変更要求、過去記録の扱いは[docs/README.md](docs/README.md)の「正本と変更要求」に従う。

## path別検証マトリクス

まず変更の内容と影響pathを選ぶ。実行方法・環境差は[開発手順](docs/runbooks/dev.md)、UI固有の確認観点は[ADR 0014](docs/adr/0014-uiux-dev-flow.md)を参照する。
文書・コメントの誤字など挙動・描画・機械契約に影響しない区分Aは、配置pathだけで製品suiteを要求せず、`git diff --check` と対象記述・参照の確認を行う。設定・雛形・機械検査用ミラーは構文と読み込み先、対応する既存検査も確認する。`REFACTORING.md` 自体の検証は下表の専用行に従う。
挙動に影響する変更は該当する行を併用し、同じcommandの重複実行は不要。frontend行から `src-tauri` を除外するが、IPCやfrontendにも影響する場合は両方の行を適用する。凍結境界の個別検証とCIの必須設定はこの選定で免除しない。

| 変更path | 必須validation |
|---|---|
| `crates/core/**` | `cargo xtask rust-test` |
| `crates/store/**` | `cargo xtask rust-test` + 永続化の振る舞いが変わる場合は関連 scenario |
| `crates/transport/**` | `cargo xtask rust-test` + peer の振る舞いが変わる場合は関連 connectivity scenario |
| `crates/docs-sync/**` | `cargo xtask rust-test`（実 relay replication テスト `src/tests/relay.rs` を含む）+ replication / relay 経路の振る舞いが変わる場合は `cargo xtask scenario community_node_public_connectivity` |
| `crates/blob-service/**` | `cargo xtask rust-test` + media/blob に影響する場合は関連 scenario + blob の取得・状態、peer 間の同期・復旧の振る舞いが変わる場合は `cargo xtask app-api-slow-test`（実 Iroh の結合 test。PR の CI では実行されず nightly だけが実行する） |
| `crates/app-api/**` | `cargo xtask rust-test` + payload 形状が変わる場合は frontend test + blob の取得・状態、peer 間の同期・復旧の振る舞いが変わる場合は `cargo xtask app-api-slow-test`（実 Iroh の結合 test。PR の CI では実行されず nightly だけが実行する） |
| `crates/desktop-runtime/**` | `cargo xtask rust-test`（community_node / identity_restart / seeded_dht / media_blob_restore 等の実挙動テストを含む）+ 起動 / 永続往復が変わる場合は `cargo xtask e2e-smoke`、peer 間 connectivity・CN セッションが変わる場合は `cargo xtask scenario community_node_public_connectivity` |
| `crates/cn-*` | `cargo xtask cn-check` + `cargo xtask cn-test` |
| `harness/scenarios/**` | `cargo xtask scenario <changed-scenario>` |
| `apps/desktop/**`（`src-tauri/**` を除く） | `cargo xtask desktop-ui-check`（視覚回帰 `test:e2e:visual` を含む。CSS/スタイル変更で見た目が変わると CI の視覚 step が赤くなる。意図的な変更時は baseline を再生成する — 手順は `docs/runbooks/dev.md` の「視覚回帰」を参照） |
| `apps/desktop/src-tauri/**` | `cargo xtask tauri-check` + `cargo xtask tauri-test`（lib 単体 test。CI では `Kukuri Linux Package` の `linux-appimage` job だけが Linux で実行するため、`cfg(windows)` の test は Windows ローカルでの実行が唯一の確認になる）+ `cargo xtask e2e-smoke` |
| `docs/adr/**` | 対応する tests / contracts / scenarios を確認または更新する |
| `docs/runbooks/**` | runbook 内の command と path を確認する |
| `REFACTORING.md` | `git diff --check` + 記載した repository path / command の存在確認 + `cargo xtask oversized-files` |

`cargo xtask e2e-smoke` は `desktop_smoke_post_persist`（`FakeNetwork`・in-process 単一 runtime）1 本を回す desktop 永続 smoke であり、post 作成 → timeline → restart → 再表示の永続往復のみを検証する。実 transport / docs-sync の peer 間経路・relay replication は通らない。peer / replication / CN 接続の検証は該当 crate の `cargo xtask rust-test`（実 iroh テストを含む）と `cargo xtask scenario <connectivity scenario>` が担う。CI 実配線の connectivity scenario は `community_node_public_connectivity`（fast / release）、加えて `community_node_multi_device_connectivity`（nightly）。connectivity scenario は cn-postgres 起動 + 実 iroh peer 複数で重いため、マトリクスでは「振る舞いが変わる場合」の条件付き要求とする。

### 検証の選定・中断

- 実行前に、変更に必要な検証、利用可能なOS・依存、既存結果で確認済みの範囲を特定する。ローカルで重すぎる・実行不能な場合は最も狭い関連commandを先に使い、未実行の必須検証と理由、CIまたは別環境で補う方法を記録する。狭い検証の成功を全suiteの成功とはしない。
- 開始済みのtestは、長い再リンクや一時的な無出力だけを理由に止めず原則完走する。ユーザーの停止指示、資源枯渇、明確なハング、実行前提の誤りで有効な結果を得られない場合は担当者が中断し、command、観測根拠、完了済み範囲、未確認範囲、再開条件を記録する。
- 必須CI、適用される独立監査・境界検証を満たす前に完了・マージ可能と報告しない。実行不能や中断は未確認のまま残す。必要な検証が成功した後は、対象変更・新たな失敗・未解決の懸念がなければ同じ検証を繰り返さない。

PR 作成前または `main` merge 前は、可能なら `cargo xtask check` + `cargo xtask test` を推奨する。

## 大型ファイルポリシー

`cargo xtask oversized-files` は大型(1000行以上)の手書きファイルを報告し、
`xtask/oversized-baseline.json` との比較で **ratchet 方式の CI ゲート**として動作する。

ゲートの判定:

- baseline に無い 1000 行以上の新規ファイル → **fail**。
- baseline 記載ファイルが記録行数を超えて成長 → **fail**。
- 行数不変・減少 → pass(減少時は baseline 更新推奨の note を表示)。
- baseline の更新は `cargo xtask oversized-files --update-baseline` で再生成し、
  正当化を PR 本文に書いた上でコミットする(レビューを通すことが ratchet の意図)。
- ファイルを分割して 1000 行未満にした場合も `--update-baseline` で baseline から除去する。

ルール:

- 明示的な正当化(= baseline 更新コミット)なしに、1000行以上の新規手書きファイルを追加しない。
- 既存の大型ファイルを編集する場合は、差分を最小に保つ(行数を増やす変更は baseline 更新が必要になる)。
- 大型ファイルで複数責務に触る場合は、変更の理解・検証・差し戻しを妨げる具体的な構造問題を確認する。分割が必要なら別の構造整理として計画し、行数だけで分割を必須にしたり、依頼外の分割を混ぜたりしない。ratchetのCIゲートは維持する。
- formatting-only change と semantic change を混ぜない。
- generated file、lock file、icon は明示的に対象化されない限り、この方針の対象外とする。

## AI利用時の証拠規範

- 「観測した事実」「そこからの推測」「未検証事項」を区別する。説明の詳しさや確信度を証拠にしない。
- 「未使用」「重複」「全 test 成功」「互換性に影響しない」は仮説として扱い、参照、型検査、実行ログ、
  test / contract / scenario で確認する。
- 静的検索だけで dead code や意味が同じ重複と断定しない。候補には path、symbol、利用者、入口、
  保護する test を可能な限り添える。
- 新しい helper、wrapper、抽象化を作る前に、既存実装と全利用箇所を検索し、kukuri の現行様式を優先する。
- 一般的な clean code 論や将来拡張だけを根拠に、新しい層、型、ファイルを増やさない。
- 提案した API、command、dependency が実在することを確認する。リファクタリング PR で dependency を追加しない。
- test は観測可能な挙動を固定する。実装順序や source 構造の変更検知だけを目的とする test は追加しない。
  既存 wire / serialization / visual contract を固定する snapshot はこの限りではない。
- 変更と同時に追加した test だけを挙動維持の証拠にせず、変更前の挙動、既存 contract、fixture、scenario と照合する。
- test の削除・skip、assertion の弱体化、warning 抑制、hardcode によって validation を成功扱いにしない。
- 高リスクな境界変更は人間または独立した別コンテキストでも review する。複数担当の合意だけを挙動維持の証拠にしない。

## 記録テンプレート

定期監査では次を記録する。

```md
## 対象と開始trigger

## 有限scopeと終了条件

## 変更前baseline

## 候補一覧

| 候補 | 観測した問題と根拠 | 目標成果 | 優先順位 | 分類 | 理由 / 個別Issue |
|---|---|---|---|---|---|

## 監査終了判定
```

個別作業では次を記録する。凍結境界、互換パス、validation の詳細は本書の既存節を参照し、
テンプレート内へ複製しない。

```md
## 種別

## 観測した問題と根拠

## 現在の責務・依存・挙動

## 目標とする構造上の成果

## 維持する挙動

## 対象 / 対象外

## 凍結境界・互換パス

## 変更前baselineと成功判定

## Issue / 段階 / PR分割

## Validation

## 差し戻し手順

## 見送った候補・別Issue
```

## 必須AIワークフロー

リファクタリング監査または個別作業では、AIエージェントは以下の順序に従う。

1. `AGENTS.md`、この文書、対象の ADR / runbook / test / scenario を読む。
2. 対象領域の構造を把握し、候補と観測根拠を収集する。
3. 候補を分類・優先順位付けし、一つの意図を選ぶ。監査だけなら固定 scope の分類完了で終了する。
4. 現在の挙動、構造上の問題、目標成果、対象外、凍結境界、互換パス、validation を記録する。
5. 影響 path と必須 validation を特定し、変更前に実行して既知の失敗を今回の回帰と区別できるよう記録する。
6. 挙動を固定する網が不足する場合は、先に contract / scenario / characterization test を追加して変更前に確認する。原則は別PRとし、小さな同一責務の変更で変更前の実行証拠を残せる場合は同じPRでもよい。凍結境界で個別に要求する検証は省略しない。
7. 一つの意図を小さく差し戻し可能な段階へ分け、各段階の後に最小の関連 validation を実行する。
8. 停止条件を検出したら先へ進まず、再調査または別種別へ再分類する。
9. 最後に path 別検証マトリクスの validation と、差分全体の review を行う。未実行項目は理由を報告する。
10. before / after の構造上の成果、挙動維持の証拠、残存リスク、見送った候補を完了報告へ記録する。

## レビューチェックリスト

本書の「個別作業の成果物ベースの完了条件」に証跡を対応付ける。特に凍結境界の見落としを防ぐため、
diff内の `#[serde` 属性、wire文字列、canonical実装を確認する。共通の監査判定・停止条件はIssue運用手順に従う。

## 完了報告形式

本書の「記録テンプレート」を更新し、結果・構造上の成果・挙動維持の証拠・検証結果・残存リスクを要約する。
詳細へのリンクで足り、別形式へ全項目を転記する必要はない。非該当欄は省略できるが、必須検証の未実行・失敗・中断は理由と補完方法を残す。

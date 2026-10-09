# ADR 0039: テスターフィードバックの受付と蓄積

## Status

Accepted

## Date

2026-08-28

## Context

kukuri は仕様・機能の変更頻度が高く、詳細なチェックリストや E2E テストを先に整備しても短期間で陳腐化しやすい。一方で、テスターが実際に利用した際の「何をしようとして、何が起きて、どこに違和感を持ったか」を継続的に蓄積できれば、品質観点の発見や、安定した仕様に対する自動テスト化の材料として利用できる。

そのため、分類・優先度付け・自動分析を行わず、テスターが低コストで自由記述レポートを送信し、Community Node 側で蓄積・一覧確認できる最小構成を提供する(#802)。

一般通報(`/v1/report`、ADR 0027/0028)は対象コンテンツへの safety 判定の入口であり、subject / capability / reason を必須とし匿名受付・連絡先暗号化・運用ワークフローを持つ。テスターフィードバックは対象を特定しない利用経験の自由記述であり、必要な情報・保存方針・閲覧目的が異なるため、record と endpoint を分離する。

## Feature Data Classification

- Feature 名: テスターフィードバックレポートの収集・集約
- Durable / Transient: Durable
- Canonical Source: 受信した Community Node の Postgres (`cn_admin.tester_feedback`)
- Replicated?: No
- Rebuildable From: 再構築不能。node operator の backup 対象
- Public Replica / Private Replica / Local Only: Local Only(node operator private state)
- Gossip Hint 必要有無: 無
- Blob 必要有無: 無(添付ファイルは受け付けない)
- SQLite projection 必要有無: 無(client 側にレポートの正本・複製を持たない)
- 必須 contract: capability 無効時 404 fail-closed、bearer 認証 + 同意必須、空欄 / 2000 字超の 400、保存 → 一覧(新しい順)→ 詳細、retention 期限切れ削除、client version / OS の自動付与
- 必須 scenario: 新規 harness シナリオは追加しない。client → node の HTTP 送信経路は既存 `community_node_report_routing.yaml` が構造的に実証済みであり、シナリオレーン追加のコストが本 feature の最小スコープに不釣合いのため、各境界(protocol / storage / handler / desktop-runtime / UI)の contract test で固定する

## Decision

### 1. opt-in capability `tester_feedback` とする

テスターフィードバックの受付は `tester_feedback` capability を有効化した node だけが提供する(`features: tester_feedback: true`)。capability は manifest の `capability_scope.available_enabled` に載り、client はこれを送信先セレクトの適格条件に使う。無効な node への送信は 404 `TESTER_FEEDBACK_NOT_CONFIGURED` で fail-closed にする。中央窓口は作らない。

送信先がないときは、clientが取得済みの設定・認証/同意・接続状態・公開manifestから理由を説明し、既存のコミュニティノード設定へ案内する。取得中や取得失敗を「受付非対応」と断定しない。検索の提供・規約同意と受付機能の提供は別であり、利用者側の設定だけで運営者の受付を有効化できると案内しない。説明用の状態判定は既存の送信適格条件を緩和せず、新たな外部送信や自動認証・同意を発生させない。表示とfocusの契約は[DESIGN.md](../../DESIGN.md)に従う（#957）。

### 2. bearer 認証 + 同意承認を必須にする

`POST /v1/tester-feedback` は `require_bearer_identity` + `require_consents` を通す(indexing request と同じパターン)。client の送信先セレクトは認証済み・同意承認済みの configured node だけを表示するため、テスターの追加負担はない。匿名受付にしない理由は、自由記述の蓄積 endpoint のスパム面を認証と既存のグローバル per-IP レートリミットで抑えるためである。送信者の識別情報(pubkey)はレポート record に保存しない。

### 3. 入力は 3 つの自由記述のみ、version / OS は runtime 層で自動付与する

ユーザー入力は「やろうとしたこと」「何が起きたか」「何が変だと思ったか」の 3 項目のみで、各 2000 字以内とする。client version(`CARGO_PKG_VERSION`)と OS(`std::env::consts::OS`)は desktop-runtime が wire request 組み立て時に自動付与し、UI には入力させない。

文字数制限は両側とも Unicode コードポイント数で判定する(client: `[...text].length`、server: `chars().count()`)。既存の入力上限 precedent は byte 長だが、要件が「2000 字」であるため本 feature はコードポイント数を採用する。

### 4. 本文は plain TEXT で保存し、reports の retention をミラーする

3 つの自由記述・client version・OS は連絡先 PII ではないため `LegalDataCipher` を使わず plain TEXT で保存する。保存期間は `RetentionPolicy.tester_feedback_days`(既定 180 日)とし、作成時刻を起算点として既存の期限の判定・期限切れ削除(`cleanup_expired`)に組み込む(期限は行に持たない。ADR 0034 §1、#1704)。

### 5. 閲覧は admin dashboard の一覧と cn-cli で行う

admin dashboard(ADR 0029 の IAP 内 read-only surface)に一覧ページを追加し、受信日時・3 項目全文(HTML エスケープ済)・レポート ID・client version・OS を新しい順で表示する。ADR 0029 は通報の自由記述本文を dashboard に出さないが、テスターフィードバックは operator が本文を読むことが目的のデータであるため、意図的にこの制約の対象外とする。read-only のため CSRF / preview→apply は不要。`cn-cli tester-feedback list / show` でも同じ項目を確認できる。

## Consequences

- テスターは Control Center 隣のボタンから 3 項目を入力するだけでレポートを送信でき、operator は dashboard / CLI で一覧確認できる。
- レポートは受信 node に閉じ、replicate されず、180 日(既定)で削除される。
- 検索・分類・集計・ステータス管理・添付・通知・AI 分析は本 ADR の対象外であり、必要になった時点で別 ADR とする。
- capability を有効化しない node には endpoint が存在しないように見える(404)。

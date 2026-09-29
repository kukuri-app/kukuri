# Feature Data Classification: Community Node 法務文書

ADR 0002 (`docs/adr/0002-feature-data-classification-template.md`) に基づく分類。

### Feature Data Classification
- Feature 名: Community Node 法務文書の生成・公開・per-node 同意
- Durable / Transient: Durable
- Canonical Source: 現行の法務上の事実は各 node の `operator-config.yaml` にある `server` / `features` / `retention` / `safety` / `legal` と型付き capability descriptor。descriptor は目的・処理・データ分類・利用条件・保持参照・外部送信・削除／訂正／停止等の請求経路・safety action・効果範囲を持つ。`cn_admin.policies` は公開済み正文 snapshot revision の append-only 履歴、`cn_admin.policy_translations` は厳密な正文 snapshot に従属する参考訳履歴、client の既存暗号化ローカル同意記録はユーザーの同意行為の canonical record。別系統の catalog / consent store は作らない。
- Replicated?: No。文書は public HTTP で配信するが、gossip や P2P state として複製しない。ローカル同意記録も node や peer へ複製しない。
- Rebuildable From: 現行の公開文書と manifest は `operator-config.yaml` から再生成可能。公開済み正文・参考訳履歴とユーザーの同意記録は再構築不可で、retention cleanup の対象にしない。同意記録の喪失時は再同意する。
- Public Replica / Private Replica / Local Only: 現行・過去の正文、対応する参考訳、manifest metadata は Public Replica。DB の正文・参考訳・server consent 履歴は node-private。desktop の受諾 snapshot・日時・表示言語・app version は Local Only。
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要。desktop は既存の暗号化 local consent store を利用する。
- 必須 contract: `cn-operator` の生成・typed descriptor・明示保持値契約、`GET /v1/policies` と version／snapshot revision route、public manifest、`cn_admin.policies` の snapshot 単位 append-only 同期、正文 snapshot に固定された参考訳、desktop の slug/version/snapshot 再同意判定。
- 必須 scenario: 認証前に現行・過去正文と厳密な参考訳を取得できること、訳が無ければ正文 fallback が metadata で分かること、未同意では Node 通信を開始しないこと、法務 snapshot 変更で自動再同意になること、accept 競合が保存されないこと、同一表示 version の新 snapshot が旧正文・旧同意を上書きせず追記されること、version rollback が起動時に失敗すること。

## 境界

### 初回説明Dialog（#914）のデータ分類

- Feature 名: Community Node初回説明と規約確認への案内
- Durable / Transient: 表示段階・再試行表示・起動session中の提示済みアカウントはTransient。
- Canonical Source: Node一覧と既存暗号化local consent record。検索適格性やtokenを同意記録の代替にしない。
- Replicated?: No。表示stateをNodeやpeerへ送らず、再起動時にはローカル同意から再判定する。
- Rebuildable From: Node一覧・ローカル同意・既存の接続status。案内表示履歴の復元は不要。
- Public Replica / Private Replica / Local Only: Local Only。新しい永続同意store・DB tableは追加しない。
- Gossip Hint 必要有無 / Blob 必要有無 / SQLite projection 必要有無: いずれも不要。
- 必須contract/scenario: 全Nodeのlocal状態取得後にだけ全件未同意を判定、一覧index 0へ案内、説明表示の追加外部I/O/同意mutationが0、閉じる/あとでが同意にならない、表示文書のslug/version/snapshotと言語を既存APIへ渡す、同意後のready eventが検索先へ反映される。

説明Dialogを出すことはNode利用への同意ではない。規約確認操作では既存の公開policy取得を使い、明示受諾後だけ既存のlocal記録→認証→server同期へ進む。app-level同意・年齢/restore gate、Node別の現行policy preflight、撤回、他Node/Direct P2Pとの境界は維持する。

### 規約更新時の再同意モーダル（#1420）

- 対象: 有効なlocal consentを持ち、runtimeのpreflightまたは403で`consent_update_pending`になったNode。未同意・撤回済みのNodeは初回説明と手動の同意の対象とし、ここでは開かない。
- 表示: 検知を受けたstatusの更新で、他のDialogが無いときに既存の同意Dialogを一覧順に1つずつ開く。開いた記録はアカウント×Nodeの起動中のメモリだけ（Transient・Local Only）に置き、pendingが解消するまで再表示しない。
- 外部I/O: 表示ごとに同意済みNodeの公開policy取得が1回。同意mutationは明示受諾だけで、閉じる・Escapeは同意にならない。

- 対象は当該 community node の運用だけであり、kukuri クライアント本体の規約・プライバシーポリシーとは別である。
- node が扱わない Direct P2P、他 node、peer が保持する copy は当該 node の削除・送信防止の権限外である。
- 生成物は法的助言・完全性保証ではない。第三者 operator も同じ schema と検証を使えるが、自らの実態、契約する provider、補足記述を確認する責任を負う。

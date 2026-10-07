# Feature Data Classification: App-level legal consent

ADR 0002 (`docs/adr/0002-feature-data-classification-template.md`) に基づく分類。

### Feature Data Classification
- Feature 名: App-level 利用規約 / プライバシーポリシー 同意（app legal consent gate）
- Durable / Transient: Durable
- Canonical Source: ローカル consent ファイル（`<db_path>.app-consent.json`、ユーザー端末のみ）
- Replicated?: No（複製しない。ネットワークへ送らない）
- Rebuildable From: 再構築不可（ユーザーの同意行為そのもの）。喪失時は再同意で再生成。
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要（DB 接続前の起動 gate で読むため、DB とは独立した JSON ファイルに保存）
- 必須 contract: Tauri command `get_app_consent_status` / `accept_app_consents` の payload 形状（startup status の `consent_required` variant を含む）
- 必須 scenario: 起動 gate（未同意 → runtime 非構築 = network 非開始 → 同意 → ready）。frontend は `App.test.tsx`、backend は `src-tauri` のユニットテストで担保。

## 補足
- 同意は文書 slug 単位のレコード(#857)で管理する。現状は全文書が `LEGAL_BUNDLE_VERSION`（単調増加の整数、初期値 1、現在値 9）と同じ版番号を共有している。
- 文書ごとに `accepted_version < current_version` の場合に再同意を要求する。
- version 2 は、投稿コンテンツの権利帰属、権利保有の表明、共有範囲と Community Node capability に限定した技術的利用許諾を追加する重要変更である。
- version 3 は、利用資格（18歳以上）と成人向け表現の既定非表示に関する記載を追加する重要変更である(#858、ADR 0046)。18歳以上の自己申告の分類は `docs/legal/age-attestation-data-classification.md` を参照。
- version 4 は、管理主体、実データフロー、外部送信、診断情報、P2P copy の削除限界、日本語正文と参考訳を明記する重要変更である(#854)。外部送信の突合は `docs/legal/app-data-flow-inventory.md` と `docs/legal/external-transmission-notice.md` を参照。
- version 5 は、主体定義と責任範囲、投稿者責任、適切な権利主体への限定的許諾、鍵の中央復旧不能、Community Node 別規約、通報・利用制限、サービス変更・中断・終了、OSS ライセンスとの関係、責任制限、日本法・合意管轄、変更通知を全面改訂する重要変更である(#856)。
- version 6 は、利用規約 第3条 4 項に成人向け表現の第 2 のラベル源（利用者が設定した Community Node の推定）を追加し、プライバシーポリシーと外部送信表示に推定の照会で送る識別子を追記する重要変更である(#1056、ADR 0046 §6.4)。
- version 7 は、利用規約 第3条に信頼評価による折りたたみ（第 5 項）とブロック・ミュートの任意提供（第 6 項）を追加し、プライバシーポリシーと外部送信表示へ信頼評価の照会で送る項目・送らない情報を追記する重要変更である(#1061、ADR 0026 §8)。ブロック・ミュートの提供は、同意済みの Node capability の範囲内のため外部送信表示では version 6 補記として扱い、利用規約とプライバシーポリシーには version 7 で明記した。
- version 8 は、公開投稿の先頭外部URLについてlink先とOGP画像配信先へ自動取得を行うこと、送信・観測され得る項目、送らない情報、private／非表示contentを対象外とすること、process-memory保持を利用規約・プライバシーポリシー・外部送信表示へ追加する重要変更である(#1174、ADR 0051)。
- version 9 は、デスクトップアプリの設定「公開コンテンツの発見」（既定でオン）による Mainline DHT への参加（Community Node 利用中・`static_peer` を含む）、保持する公開 blob の hash から導いた infohash の告知・検索、kukuri が運用する補助 index への登録・照会、告知・検索しない内容、期限による消去と、Web クライアントが検索を提供する Community Node へ公開 blob の hash を送って保持端末を探すことをプライバシーポリシーと外部送信表示へ追加する重要変更である(#1632)。Web の検索は配布前の version 9 の本文と変更履歴へ足し、版は上げていない。利用規約は条項を変えず、版・施行日だけを同期した。
- 各記録には記録時の build の種別 `build_profile`（配布版 `release`、開発ビルド `development`）を残す(#1105)。同意判定には使わない。#1105 より前の記録には無い。開発ビルドは配布版と別の app data dir を使うため、配布版の consent ファイルへ書き込まない（`docs/runbooks/dev.md`）。
- 同意するまで `DesktopRuntime` を構築せず、iroh endpoint の bind / discovery を開始しない（fail-closed = IP 取得前に同意）。

## 初回表示と言語選択（#917）

### Feature Data Classification
- Feature 名: 同意前のOS言語取得と端末表示言語
- Durable / Transient: OS／navigatorの候補はTransient、選択・初期決定した言語はDurable
- Canonical Source: OSのUI言語設定／起動環境、端末の `localStorage[kukuri.desktop.locale]`
- Replicated?: No。既存の端末バックアップに含まれるlocaleキーの扱いだけを維持する
- Rebuildable From: OS候補は再取得可能。保存済み言語の喪失時はOS／navigatorから再決定、または利用者が選び直す
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要
- 必須 contract: `get_system_locales` は引数なしでOS言語候補だけを返す。同意前も呼べるがruntime、DB、同意ファイル、networkを使用しない。言語変更だけでは受諾しない
- 必須 scenario: locale未保存の日本語OS→日本語で同意表示、保存値優先、取得失敗fallback、明示選択と再起動、同意pending中の言語固定、復元activation後の既存locale適用

優先順位は保存済み対応言語→OS候補→navigator候補→英語。自動決定も既存キーへ保存する。過去のcacheと明示選択を推測で区別・書換えしない。言語と保存可否は同意状態とは独立し、選択言語は従来どおり明示受諾の時点でのみ同意レコードへ記録する。

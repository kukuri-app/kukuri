# #1243: #1224に先行する停止処理の責務分離

## 目的・範囲

- 2026-09-22、ユーザーが#1224着手前に既知の手戻り部分の先行修正を依頼。
- 構造整理、区分C。基準`e652c3df46718d81318ea2462869cbed79863145`。
- Scope revision: `2026-09-22-v1`（#1294の既存契約を維持する限定delta）。
- #1294 AC-4と既存BF-5/6・BT-3〜6を維持する。新しい製品要件は追加しない。
- 共通ownerの数値上限・優先順位・peer共有・lease APIは#1224が所有する。本差分では決めない。

## 観測した結合と変更

`IrohDocsSync::close_replica_owned`のspawn closureに、task所有と実際の停止処理が同居していた。
共通ownerへ管理元を移す際、秘密の失効・event task停止・leave/close・失敗時の隔離まで同じclosureを
変更対象として扱う必要があった。今回、後者をprivate helper `close_replica_under_guard`へ抽出する。

呼出元は引き続き上限32のclose task台帳、registry lockの取得、taskの所有と完了通知を担当する。
helperは渡されたregistryの排他的借用のもとで停止・隔離・失効を完了する。新しい台帳やschedulerは作らない。
共通ownerから呼べる公開APIを推測で追加せず、現在の入口とtaskの所有元はそのままとする。
従って#1224統合そのものの完了でも、将来の実装変更が不要になったという判定でもない。

維持する順序は、task所有確定→必要時のsecret削除→handle除去→event task停止→leave→close。
leave失敗だけはclosing状態のhandleを戻し、明示closeで再試行する。close失敗のDocは戻さない。
registry guardは結果通知まで呼出元が保持し、revoke/open/restartの直列化を維持する。
wire・署名・namespace導出・保存形式・DocsSync trait・依存バージョンに変更はない。

## 検証と監査

- 変更前の基準: 既存4件のlifecycle contractは#1297の完了証跡で成功。
  同一停止コードの#1303修正作業でも全Rust検証内で4件成功（依存修正を含むため別の検証条件として区別）。
- 変更後: lifecycle 4件、`cargo xtask rust-test`、Rust静的検査、oversized-files、diff checkを実施する。
  replication動作の変更はなく、接続scenarioの追加実行はこの抽出差分単独では要求しない。
- 独立監査はBF-5/6とBT-3〜6に限定したdeltaを固定headで行う。
- lifecycle 4件成功。`rust-test`と同じnon-CN package集合・既定nextest設定・stack設定で
  nextest 1,358件成功、既存skip 5件。doctestも成功。workspace fmt、同じ集合のclippy
  `--all-targets -- -D warnings`、`cargo xtask-lite oversized-files`、diff checkが成功。
  別worktreeのxtask実行ファイルと競合させないため、Rust gateの実体commandを直接実行した。
- 未commit差分の独立予備reviewで順序・lock保持・cancel契約の維持を確認。正式head監査とCIは未完了。

差し戻しはこの抽出だけをrevertする。#1303のLocalOnly修正・上流panic修正は別PRで管理し、混ぜない。

## 残る接続作業

#1224の設計後、開始/停止要求、lease、peer選択、作業集合の上限と失敗時の枠管理を共通ownerへ接続する。
これらの具体的変更は現時点で未確定。bucket ID・source locator・LocalOnly・停止時の安全性の契約は維持する。

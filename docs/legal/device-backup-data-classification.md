# Feature Data Classification: 端末内データのバックアップと復元

ADR 0002（`docs/adr/0002-feature-data-classification-template.md`）に基づく分類。仕様はADR 0048。

### Feature Data Classification: 暗号化バックアップファイル
- Feature 名: 端末内データの暗号化バックアップ
- Durable / Transient: Durable（保存先と保持期間は利用者が管理する）
- Canonical Source: 作成時点の選択中アカウントdirectory、identity storage、許可済みfrontend localStorage key
- Replicated?: No（アプリは自動送信しない）
- Rebuildable From: 同じ端末内状態が残る間だけ再生成可能。端末喪失後はバックアップとパスフレーズがなければ復元不能
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 必要（保護所有先のblob。1MiB以下は`kukuri.db`の行、超えるものは保護された`kukuri.remote-blobs/`のfileを暗号化entryとして保持。旧`iroh-data`は含めない。#1221 R5-G）
- SQLite projection 必要有無: 必要（停止後の`kukuri.db`を保持し、復元時に現行migrationを適用）
- 必須 contract: 暗号化chunkの順序・完全性・長さ・hash、容量上限、秘密の非露出、原子的な一時ファイル確定
- 必須 scenario: offline未同期投稿、下書き、bookmark/mute/block、private channel、Node設定、添付を含む作成から新規instance復元までのround-trip

## 対象分類

| 区分 | データ | 復元後の扱い |
|---|---|---|
| 必須 | アカウント鍵、SQLite、private channel能力、gossip購読状態、discovery設定、Community Node接続先・招待情報、保護所有先の本人データ（本人投稿・bookmark・private参加状態・未送信outbox・DM添付・avatar・custom reaction/自作Dome asset・自分のlive/game manifest）、下書き | 同じ公開鍵のアカウントとして復元する |
| 含めない | 旧`iroh-data`（remote内容が混在するSDKのDocs/Blob）、非保護のremote cacheのfile | 復元後は必要になったときに取り直す |
| 任意適用 | workspace layout、theme、locale | previewで利用者が適用を選択する |
| 移行不可 | iroh endpoint secret、Community Node bearer token、実行中session、通知cursor、OS通知権限 | 新端末で再生成、再認証、再設定する |
| 新端末で再同意 | app-level同意、Community Node同意、18歳以上の自己申告、成人向け表示許可 | 記録を含めず、成人向け表示をOFFとして明示操作を求める |

## 脅威モデル

- バックアップ漏えいは、アカウントのなりすまし、DM・非公開内容・下書きの漏えいにつながる。作成前に安全な保管を警告する。
- 弱いパスフレーズによる総当たりを軽減するためArgon2idを使用し、最低文字数を要求する。運営者はパスフレーズを復旧できない。
- 細工された入力によるKDF資源枯渇、巨大entry、過剰entry数、path traversal、chunk並べ替え、重複、欠落、切詰めを上限と認証検証で拒否する。
- 容量不足、cancel、書込み失敗、migration失敗、runtime構築失敗、process停止では、journal、staging、rollbackを使い、次回起動を含めて既存状態または再同意待ちの復元状態へ収束する。任意適用のfrontend stateは`Activated`後のdurable markerからだけ適用する。適用または確認応答がerrorを返した場合は旧localStorageへ戻し、process停止では残ったmarkerから同じ復元値を再適用する。
- 復元したidentityの通常runtime、remote取得、scheduler、background通知は、app-level同意と18歳以上の自己申告をresetした後、利用者が明示的に再同意するまで開始しない。
- バックアップ削除や復元はP2PまたはCommunity Node上の既存copyを削除しない。

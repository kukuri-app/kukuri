# ADR 0066: Android Chrome の PWA・オフライン操作・更新

## Status

Accepted（#1767、2026-10-11のユーザー判断）。実装・検証の完了は各ACで判定する。
詳細表の正本は[作業計画](../progress/2026-10-11-1767-pwa-mobile-plan.md)。

## Context

Android native / Google Play公開を中止し、既存Webへ集約する。移管の照合は#1766、PWAの実装は#1767が所有する。
基準main / `codex/integration/pwa-mobile` は `5537ca6da79cf19b62986fbd4fe81554d77a064d`。
既存runtime・保存・通信・QR/同期はADR0056〜0062を使う。基準にmanifest・Service Worker・installed PWAの検証はない。
ユーザーはAndroid Chromeを先に完成させ、オフラインで閲覧・下書き・送信を受け付け、復帰後に配送する方針と、入口・更新・追加機能の初回案を選んだ。

## Decision

### 対応環境と入口（D1 / D2）

- 初回のPWA保証対象はAndroid Chrome。既存CIのAndroid 15/API35・Chrome124と現行stableを検証基準とし、実際の版を結果へ記す。手元のAndroid 16/API36.1 AVDはモバイル入力の確認先に使う。古いOS/browserの保証とiPhoneのinstalled PWAは初回対象外。desktop Firefox/Safariの既存Webの扱いは変更しない。
- URL / 保存originは `https://app.kukuri.app` のまま。manifestのidentity・start URL・scopeは同originの `/`、nameは `kukuri`、displayは `standalone`。起動URLにaccount・招待や移行の秘密を付けない。
- Chromeのインストール操作への任意の案内を置く。取消でも通常tabのSNS利用を保つ。installを初回同意・account開始の必須条件にしない。
- ホーム画面から独立表示で既存の初回同意と現在accountへ到達する。実際のWebAPK / shortcut形式とstandaloneを記録し、tab起動の成功で代えない。

### オフライン操作と送信（D3）

対象は初回online利用後、同originに必要なshellとデータが保存されている状態。

- 保存済みtimeline / thread / DMと取得済みmediaを既存ページ窓で表示する。未取得の本文・media・続きは取得できない状態を示し、空一覧や取得済みと扱わない。
- 投稿・返信・DMの文章下書きは既存account別保存で再起動・更新後も維持する。未送信の添付下書きの再起動復元を新しく保証しない。
- 既に参加した公開topic / private channelと、手元で既存の許可が確認できる相互DMの文章・画像・動画を受け付ける。返信先は手元にあるものを使う。保存の確定後に受付を示す。保存失敗は成功とせず、本文と再試行を残す。
- 投稿は既存の本人docs/blobの保護保存、DMは既存保護outboxを使う。受付と相手への送達を分け、ACK前のDMはpendingのまま。復帰・再起動後の有効な需要で同じobject/message IDを配送する。既存所属・private・accountの検査を維持する。
- 閉じたPWAの背景送信、全履歴の完全同期、全topic再購読は要求しない。Service Workerに送信queue・署名・P2P runtimeを作らず、既存索引・有限窓・保護/回収・retry ownerへ集約する。

### shellと更新（D4）

- Service WorkerはWeb buildの同originの固定静的資源だけをcacheする（HTML / JS / CSS / WASM / runtime snippet / icon / 同梱locale）。CN・relay・STUN、API応答、投稿/DM/鍵/token/招待/移行リンクをcacheしない。Cache Storageを既存IndexedDBの代わりにしない。
- 新しいshellの準備が完了するまで旧版を維持し、更新を案内する。利用者の明示操作でだけ切り替え、入力中の自動再読込をしない。適用前に文章下書きを保存し、失敗したら現在画面と入力を保って知らせる。
- 通常tabとPWAは既存Web Lockを共用する。鍵・endpoint identity・設定・文章下書き・private capability・受付済みの保護データを維持する。保存schema / backup / wire / 既存commandの意味は変えない。
- build単位のshell cacheには保持と削除を持たせる。使用中の旧assetを先に消さず、使用を終えた版から背景で回収する。永久にcache名が増える経路を残さない。
- 新版の準備失敗・更新延期でも旧版を使える。rollbackは静的buildの差し替えで、保存schemaの逆migrationは追加しない。既存schema更新の案内/排他はADR0059を維持する。

### モバイル適応と資産の分類（D5 / M0〜M6）

移管元はAndroid統合 `cee9179ec1837d44ea28c85dc395e076f717c8cb`。必要な操作条件をWebの呼出元へ接続し、native adapterをコピーしない。

| 資産 | 分類・初回範囲 | 責務と根拠 |
| --- | --- | --- |
| M0 | 既存Webで充足 | #1766のmainの11資産を継承。基準SHAの包含と既存契約を維持 |
| M1 | 既存lifecycleを再利用しPWAで確認 | #1745/#1749の操作条件。`web-runtime/lifecycle.rs` → host → 有効需要/既存outbox。旧世代破棄。別worker/retry loopなし |
| M2 | Webへ適応 | #1761/#1764。投稿/返信/DMのIME、keyboard→overlay→composer→画面の1層の戻る、draft保持、回転、初回同意/設定の余白・文字拡大・TalkBack。Browser history/viewportと共通UI |
| M3 | Webへ適応 | #1748/#1752。明示したFile/Blob入力、原本の全量base64化を避ける経路、中断回収。原本/poster/manifest/privateを維持。容量/枚数の新しい製品上限なし |
| M4 | 既存Webの入口で充足、PWAで確認 | #1728/#1755。file input/download、外部HTTP(S) URL、招待/移行の貼付と明示確認、取消/失敗。OS link/share targetは対象外 |
| M5 | 既存アプリ内通知で充足 | 通知ID/現在account/所属確認を維持。OS通知の許可/配送/tapと停止中pushは対象外 |
| M6 | 既存Web保存で充足 | #1697/#1733。IndexedDB/WebCryptoの破損/missing/未保存の区別、保存案内、鍵export/import・QR移行。Keystore/native restore/完全device backupは対象外 |

M3の原本は選択操作に所有されたFile/Blobで保持する。送信は有限chunkで既存accountのblob保存へ接続し、取消/失敗/account切替/次回起動で仮入力を回収する。必要なstagingはaccount束縛・完成の印・期限・索引・背景回収を持つ。送信前の仮入力を投稿済み/配送可能と扱わない。
Android-only ADR0065と#1193〜#1205のnative/Play配布・OS adapter・Play製品条件はPWAの正本ではない。失効範囲と移管元は#1766を参照する。mainのrevertと生年月日入力・検索/おすすめ/block/通知/CN同意の変更を行わない。

### 検証と終了

局所validationと実PR CI、Cの固定head独立監査で判定する。モバイルはemulator優先。install/launcher/standalone/IME/戻る/touch/回転/文字拡大/TalkBackと採用したoffline/resume/updateを確認し、実結果と未確認を分ける。
実機が必要なら試験ごとに元の接続・設定・起動状態と変更対象を記録し、成功/失敗/中断のいずれも終了時に復元を確認してから次へ進む。
main最終統合後の内容一致で#1767を完了とする。本番配備・公開URL/DNS/CN設定変更は別作業。

## Feature Data Classification

- Feature名: Android Chrome PWA、静的shellの保存/更新、オフライン操作。
- Durable / Transient: shellはCache。install/update状態・選択中FileはTransient。受付済み投稿/DM/文章下書きは既存Durable保存先。
- Canonical Source: 静的buildは配信artifact。本人データは既存account別保存。
- Replicated?: shell/下書き/仮入力は複製しない。投稿/DMは既存protocol。
- Rebuildable From: shellは同originの同じbuild。本人データはADR0059の復旧。
- Public Replica / Private Replica / Local Only: shell/下書き/仮入力はLocal Only。投稿/DMの公開範囲は維持。
- Gossip Hint必要有無: PWA固有はなし。投稿は既存hint/配送。
- Blob必要有無: shellはCache Storage、送信本文/原本/posterは既存blob-service。
- SQLite projection必要有無: PWA固有はなし。Webは既存IndexedDB projection。
- 必須contract: 静的資源限定、明示更新、使用終了後の旧cache回収、identity/private/受付済みデータ保持、原本byte一致と仮入力回収。
- 必須scenario: 作業計画のinstall/launcher・offline受付/復帰/restart・update/tab競合・モバイル実入力。
- 新しい外部送信: なし。shell取得は既存Web配信元。背景P2P/pushなし。

## References

- AGENTS.md、PLANS.md、Issue lifecycle runbook、DESIGN.md、ADR0014、ADR0056〜0062。
- [#1766の固定資産と失効判断](https://github.com/kukuri-app/kukuri/issues/1766)。
- [PWAのインストール](https://web.dev/learn/pwa/installation)、[PWAの更新](https://web.dev/learn/pwa/update)。

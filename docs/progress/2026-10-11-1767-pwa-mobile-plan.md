# #1767 Android Chrome PWA の実装計画

製品境界は[ADR0066](../adr/0066-pwa-mobile-install-offline-update.md)。
Scope revision: `2026-10-11-pwa-r2`。基準commit: `5537ca6da79cf19b62986fbd4fe81554d77a064d`。
Current statusは[#1767](https://github.com/kukuri-app/kukuri/issues/1767)へ集約する。
D1〜D5は2026-10-11の回答で確定。iPhone/push/OS link・share/完全backup/新しい削除導線/本番配備は対象外。

## 末端ACとPR

各行は1成果と1PR。実装PRのbaseは `codex/integration/pwa-mobile`。
contract・caller切替・不要経路の削除・validationを同じPRに含める。結果や利用条件の変更は先にユーザー判断を得る。

| PR枠 / 末端AC | 対象・期待結果 | 判定方法・実行先 | 依存 / 区分 |
| --- | --- | --- | --- |
| PR-PLAN / AC-PLAN | 承認されたD1〜D5/M0〜M6を正本へ反映。未分類0、Androidの失効/流用元、有限AC/INVAR/経路/検証を固定 | 回答とADR/関連正本/本表の照合、リンク・`git diff --check`、実PR CI | なし / A |
| PR-ENTRY / AC-ENTRY | Web artifactにmanifest/icon/任意install案内を含め、同origin `/` からホーム画面standaloneで既存同意と現在accountへ到達。取消でもtab利用を継続 | build/CSP、frontendの案内/取消test、emulatorで実install/launcher/standalone/公開鍵一致。Chrome124/stableの実版・install形式を記録 | PLAN / B、独立監査不要（既存gateを維持） |
| PR-SHELL / AC-SHELL | 初回online後のoffline起動・保存済み閲覧・文章下書き・投稿/返信/DMの文章/画像/動画受付を完成。復帰/restart後に同じIDを配送。明示更新で鍵/設定/draft/private/受付済みデータを維持し、使用を終えた旧shell cacheを回収 | 実Web/WASMとemulatorでonline→offline→受付→reload→online。保存失敗で受付なし、未取得と空を区別。同originの2 buildで更新延期/準備失敗/明示適用・tab/PWA lock競合・使用中assetの維持/終了後の回収。cacheにAPI/秘密/本人内容なし、page/chunk/due窓が有界 | ENTRY / C、固定head独立監査 |
| PR-INPUT / AC-INPUT | 投稿/返信/DMのIME・1層の戻る・touch・回転、初回同意/設定の余白/200%文字/TalkBack、既存file入出力/外部URL/招待・移行確認をPWAで成立。既存hostの世代/需要/outboxへ復帰を接続 | emulatorの実入力。変換Enterで誤送信なし、keyboard→overlay→composer→画面の順で本文を保持。回転でscope/draft維持。320px/電話の縦横、JA/EN、既存themeで到達性確認。file取消/失敗、鍵export/import、確認前の参加/接続なし。対象test・before/after・UI記録 | SHELL / C（変更したリンク/account境界を独立監査。未変更の既存判定は再利用） |
| PR-ATTACHMENT / AC-ATTACHMENT | Webの明示File/Blob選択を原本全量base64化を避ける入力へ統合。投稿/返信/DM/プロフィール/カスタムリアクションの共有callerと原本/poster/private境界を維持。取消/失敗/切替/次回起動で仮入力を有界回収。容量/枚数の新上限なし | 原本hash/poster/manifestと公開/private/DM結果を実Web経路で照合。選択取消/入力失敗/account切替/中断restartと受領前の非公開/送信後の保護。1MiB以下のchunkと索引のstep、全添付/全履歴読込みなし。対象frontend/Rust/browser test、emulator file input | INPUT / C、固定head独立監査 |
| PR-INTEGRATE / AC-INTEGRATE | 採用した固定PWA headをmainへ最終統合し、全AC/INVARの証拠に一致。未接続の代替経路・不要コピー・永久cacheを残さない | 各ACの固定head/監査/必須CI、最終統合PR CI、merge後のtree/差分一致。既存11資産を保持。同じ対象範囲の実装/test/補助コード量を比較。Issueの実結果を更新/Close | 上記全て / C、統合差分独立監査 |

順序は依存のためで、PR数を進捗率に換算しない。環境で判定できない必須操作は未確認とし、必要最小限の実機へ限定する。

## 維持条件

IssueのINVAR-1〜5を維持する。

- 既存同意/年齢申告/成人表示/検索・おすすめ/private audience/CNの意味を変更せず、mainをrevertしない。
- origin内1runtime、main-thread WASM、既存host/storage/IndexedDB/WebCrypto。Service Workerは静的shellだけで秘密の別保存先/背景P2Pを持たない。
- Direct P2P → Relay Supported P2P → Relay Fallback。全件同期/再購読を完了条件にしない。
- app cache回収で鍵/設定/文章draft/private/受付済みデータを消さない。既存wire/backup/commandの意味を維持。
- install/update/OS permissionを通常tabのSNS利用の必須条件にしない。
- mock/browser tab/installed PWA/別OSの結果を相互に代替しない。

## 経路

| ID | AC / 境界 | 入口 → 責務 → 結果・副作用 | 主な予定path |
| --- | --- | --- | --- |
| T1 | ENTRY/SHELL、同意/identity | launcher/navigation → entry/static shell → 既存gate/Web Lock/現在account。同意前のP2P/CNなし | `apps/desktop/{index.html,vite.config.ts,web-public,src/main.tsx,src/lib/webRuntime.ts}`、Web build/fixture |
| T2 | SHELL、保存/配送/private | offline送信 → 共通API → 保護blob/docs・DM outbox → 既存需要/due ownerで同じIDを配送。保存前の成功/別account・公開への配送なし | `crates/{web-runtime,desktop-runtime,app-api,blob-service}`、`apps/desktop/src/shell` |
| T3 | SHELL、更新/回収 | 更新案内 → draft確定/明示適用 → 同じaccount再起動。旧cacheは使用終了後だけ回収 | build/Service Worker/account別draft保存、Web E2E/CI |
| T4 | INPUT、scope/draft/guard | IME/back/touch/rotate/lifecycle → Composer/browser history/host → 文脈維持。link/招待・移行は既存明示確認 | `apps/desktop/src/{components,shell,lib}`、`crates/web-runtime/src/lifecycle.rs` |
| T5 | ATTACHMENT、原本/非公開 | File選択 → 操作所有Blob/chunk入力 → 既存blob保存・送信。取消/失敗/切替/中断 → 仮入力の索引付き回収 | `src/lib/attachments.ts`、`src/shell/{media.ts,data}`、必要な共通runtime/blob入力とWeb保存 |

変更した共通helperの全callerをCodeGraphで逆引きし、合意済み利用場面の結果へ接続する。
未変更の契約は既存test/実結果を再利用。未使用adapterや設計資料だけを移管済み/完了としない。

## 検証・復元・終了

- ローカルは対象frontend test/型検査、Rust対象test/wasm compile、必要な実browserを選択。全体は実PR CI。未変更suiteを一律反復しない。
- install/update/通信stateはDESIGN/ADR0014の全stateとkeyboard/pointer/touchを確認し、新規導線のUI記録を残す。
- emulator優先。既存CI API35と手元AVDでOSのinstall/launcher/IME/TalkBackを確認。macOS SafariをiPhoneの成果とせず、実回線切替を保証へ追加しない。
- 実機試験は開始時の接続/設定/起動状態と変更内容を記録し、成否/中断を問わず終了時に復元確認。未復元で次の実機試験へ進まない。

D1〜D5/M0〜M6の採用調査は終了。技術調査は該当ACの入口と結果に限定する。
新しい入力制限・platform保証・保存/通信の意味の変更が必要なら、ユーザー判断を得る。
次はAC-PLANの照合/PR CI、その後AC-ENTRYのcontractと実装。

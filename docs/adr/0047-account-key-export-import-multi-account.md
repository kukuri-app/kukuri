# ADR 0047: アカウント鍵の暗号化エクスポート・インポートと複数アカウント

## Status
Accepted

## Context
- Issue #859(Parent #853 Phase A)。秘密鍵の紛失で本人性を復旧できない・端末変更時にアカウントを移行できない状態は、無償 Preview 公開のブロッカーである。中央集権的な「ログイン」ではなく、鍵による本人性の移行として設計する。
- 本 ADR の対象は本人性を表すアカウント鍵(secp256k1、`KukuriKeys`)に限定する。ローカル DB・添付・設定・非公開チャネル秘密等を含む完全な端末移行/バックアップは #855 の領分であり、対象範囲の違いを UI と文書の双方で明示する。
- 従来の識別子ストレージは「db path につき 1 identity」(keyring `db:<canonical path>` / `<db>.identity-key`)を前提としており、複数アカウントの概念が存在しなかった。
- インポートは再起動での反映ではなく「アカウントの追加」として扱い、複数鍵の管理・切替を同時に導入する(ユーザー決定)。後方互換より単一レイアウトへの統一を優先する(破壊的変更可、ユーザー決定)。

## Decision

### 1. エクスポート形式(`kukuri-account-key.v1.<base64url(JSON)>`)
- 暗号化: XChaCha20-Poly1305。鍵導出は argon2id(パラメータは envelope に記録、インポート時は DoS 防止の上限内でそのまま使用)。パスフレーズは 8 文字以上を必須とする。
- envelope(JSON)は `version` / `kdf` / KDF パラメータ / `salt_hex` / `nonce_hex` / `public_key`(fingerprint)/ `ciphertext_hex` を持つ。**version・KDF パラメータ・salt・public_key は AEAD の AAD に束縛**し、改竄・バージョン偽装を認証エラーで検出する(整合性検証)。
- `preview` はパスフレーズなしで version / kdf / fingerprint のみ返し、復号は行わない。復号後は導出 pubkey と envelope の `public_key` の一致を検証する。
- 平文秘密鍵は UI・IPC・ログ・クリップボード・診断レポートのどこにも出さない。IPC に載るのは暗号化 envelope のみで、パスフレーズを含む request DTO は `Debug` を `<redacted>` にする。
- 実装: `crates/core/src/identity_export.rs`(純粋・I/O なし)。

### 2. アカウント毎データディレクトリと registry
- ストレージは `<app_data>/accounts/<account_id>/kukuri.db` の単一レイアウトに統一する。`account_id` は公開鍵 hex の先頭 16 文字(安定・衝突実質なし)。
- db path から導出される既存の per-identity 状態(identity keyring/file、CN 設定・トークン、非公開チャネル capability、gossip 購読状態、iroh データ、content-display 設定)は、この変更だけで**すべて自動的にアカウント毎**になる。単一 SQLite DB の複数アカウント共有は行わない(`dm_conversations.peer_pubkey UNIQUE` 等が衝突するため)。
- アカウントの列挙と `active_account_id` は `<app_data>/accounts.json`(registry)で管理する。registry は**秘密情報を含まないメタデータのみ**(id / pubkey / label / timestamps)を持ち、既存の temp→fsync→rename プリミティブで原子的に書く。
- 端末レベルの状態(アプリ同意・年齢自己申告 `<app_data>/kukuri.app-consent.json`、テーマ/言語、OS 通知設定)は base ディレクトリに残し、アカウントに紐づけない(アカウント追加/切替で再同意を発生させない)。
- 旧 flat レイアウト(`<app_data>/kukuri.db`)は初回起動時に一括移行し、以後サポートしない。移行順序は「鍵を新レイアウトへ複製→検証→registry 書き込み(commit point)→ファイル移動・keyring 再登録→旧実体削除」で、どの時点でクラッシュしても鍵が最低ひとつの場所から読める。registry 書き込み後の残骸は次回起動時に冪等に再開する。
- 実装: `crates/desktop-runtime/src/accounts.rs`。

### 3. インポート = アカウント追加(原子的反映)
- インポートは復号・検証をメモリ上で完了させ、新しいアカウントディレクトリと identity を作り切ってから registry に追記する。既存アカウントには一切触れないため、途中失敗で既存状態は壊れない(残骸ディレクトリは再実行で上書き)。
- 同一 pubkey が登録済みの場合はエラーで拒否する(既存鍵の in-place 置換は存在しない。preview の `already_registered` で事前に UI 警告する)。
- インポート前に fingerprint(公開鍵)を preview で確認できる。

### 4. 再起動なしのアカウント切替
- `DesktopState` は `RwLock<Arc<DesktopRuntime>>` を持ち、切替コマンドは「新 db path で runtime を構築→成功後に registry の `active_account_id` 更新→Arc スワップ→旧 runtime `shutdown()`」の順で行う。構築失敗時は旧 runtime が無傷で残る。切替は guard mutex で直列化する。
- 旧 runtime のイベント購読チャネルは閉じるため、通知ディスパッチタスクは再 subscribe し、OS 通知の pubkey キャッシュ / dispatch cursor はリセットする。`DesktopStartupState` を Initializing→Ready と駆動して frontend を再同期する。
- frontend は切替完了後に列下書き(localStorage)を破棄して再読み込みし、新アカウントの状態で UI を再構築する。ワークスペースレイアウトは端末レベルの表示設定として共有のままとする。

### 5. マルチ端末・競合・失効/ローテーションの制約(v1)
- 同一鍵の複数端末利用: 各端末が独立に投稿・購読する。投稿は署名済み envelope としてマージされ衝突しない。DB・下書きは端末間で同期しない。非公開チャネル capability などは、本人の端末間の同期（ADR 0061）が、同期の対象の item だけを運ぶ（2026-10-04 改訂。旧: 同期されない）。同時利用による本人性の競合は発生しない(同一鍵 = 同一本人)。
- 失効・ローテーション: v1 では提供しない。鍵が漏えいした場合、その鍵を無効化する中央機構は存在せず、なりすましを止められない。エクスポート前警告でこのリスクと「運営者は復旧できない」ことを明示する。鍵ローテーション(新鍵への本人性移行の告知)は将来の別 issue の領分。
- パスフレーズ喪失 = エクスポートの復元不能。運営者を含む誰にも復元できない。

### 6. 別の端末で使う 3 つの方法の対象の差（#1211 AC-5、2026-10-04）

設定の「アカウント」は、次の差を短い一覧で示す（ユーザー判断）。Web には backup・restore が無いので、その行を出さない。移行の画面（ADR 0062 §8）も、移るもの・移らないものを示す。

| 方法 | 運ぶもの | 運ばないもの | 使える所 |
| --- | --- | --- | --- |
| 別の端末へ移す（QR・専用リンク。ADR 0062） | アカウント鍵、profile、フォローとブロック、「この作者を常に表示する」の指定、参加中の private channel とその鍵。選んだときは、移行元で書いた自分の投稿の履歴（本文と添付） | ミュート、DM と通知の履歴、Community Node の設定・同意・token、アプリの同意と年齢の申告、成人向けの表示の設定、endpoint の秘密鍵と端末 ID、OS の permission | desktop と Web |
| アカウント鍵の export・import（本 ADR） | アカウント鍵だけ | それ以外のすべて。import の後は、本人の別の端末との同期（ADR 0061）が、同期の対象の item だけを運ぶ | desktop と Web |
| 端末の backup・restore（ADR 0048） | 鍵と、端末のデータ全体（ADR 0048 §3） | ADR 0048 §3 の除外（endpoint の秘密鍵、token、session、OS の権限など） | desktop だけ |

## Consequences

### 2026-09-13 追加: アカウントメニューとログアウト（#1005）

- Control Center左隣のアバターメニューは、プロフィール表示、登録account一覧、鍵import、設定account、logoutを提供する。プロフィール表示は本人の既存カラムをfocusし、なければ追加する。
- 「アカウント追加」Dialogには鍵importと「新しいアカウントを作成」を置く。明示的新規作成はoperation ID付き生成予約で冪等にし、既存登録を保持して新accountへ切り替え、初回CN同意／skip後にプロフィール設定へ進む（Scope revision v3）。
- logoutはローカルデータ・鍵を残して管理登録から外す操作。確認後、直前に使用した登録accountへ戻る。履歴が無効なら残存履歴、旧状態ならlast_used_at降順/id順を使う。他の登録が0件の場合だけ鍵ペアを生成して有効化する。
- 切替履歴は成功した切替のみを記録する。logout対象の登録除外と次activeは一つのregistry commitで反映する。生成予約と回復情報を永続化し、再試行・restartで二重生成やlogout対象の復活を起こさない。
- 同じ鍵を再importすると完全pubkeyを照合して残存account directoryを再利用する。DB・blob・鍵を上書きせず、未登録directoryを自動列挙しない。
- 新規生成accountは初回プロフィール設定対象を永続化する。初回CN案内で同意完了または明示skipした後、プロフィール設定Dialogを表示する。既存accountを空の表示名だけで新規扱いしない。
- 初回プロフィール保存は既存profile/avatar公開契約を利用する。完了状態はLocal Onlyかつaccount単位。別accountへ遅延した保存や完了通知を適用しない。
- 固定AC・INVAR・inventory・transitionは [#1005実装計画](../progress/2026-09-13-1005-account-quick-menu-plan.md) に置く。実装・検証状態は同計画の証跡で判断する。
- ADR 0002 分類は `docs/legal/account-key-export-data-classification.md` に定める。
- `KUKURI_INSTANCE` / `KUKURI_APP_DATA_DIR` で指定したディレクトリ配下も同じ accounts レイアウトになる(dev runbook の手順は据え置き。ディレクトリ構造だけが 1 段深くなる)。
- 「アカウントを識別する鍵はユーザーの端末にのみ保存される」という法務文言は維持される(エクスポートはユーザー自身の明示操作であり、アプリが外部へ送信することはない)。`LEGAL_BUNDLE_VERSION` は変更しない。
- `.nsec` legacy 読込経路は per-account ディレクトリ配下でも従来どおり機能し、その sunset 条件(REFACTORING.md)は本 ADR で変更しない。

# ADR 0059: Web の鍵・設定・projection の保存と、複数 tab・lifecycle

## Status

Accepted（Issue #1217 W4 AC-1。実装は同 Issue の AC-2〜5）

## Context

Web クライアントは、ページを閉じても回線が変わっても、アカウント鍵・private channel の鍵・設定と、その意味に必要な最小の状態を保ち、再び開いて使い続けられる必要がある（#1213 D-4・D-5・D-6・D-14）。
実行の境界は ADR 0056（main thread の単一 WASM、既存 crate の共用、`Send` の境界）、blob と docs の保存は ADR 0058（account ごとの cache の database、保存 trait）で決めた。

基準（統合 branch `b6533bdd0`）の native の事実:

- 秘密は `KeyringStore`（`crates/desktop-runtime/src/identity.rs`。`get_password`・`set_password`・`delete_password` の**同期**の trait）に置く。アカウント鍵、private channel の capability の registry（参加中の全 channel と過去の epoch を 1 つの JSON にしたもの）、gossip の購読状態、Community Node の node ごとの token・invite・同意、private index の grant。
  keyring が使えないときは file に置く。
- 平文の file（`accounts.json`・`community-node.json`・`content-display.json`・`subscriptions.json`・`app-consent.json`・`trust-display.json`・`trust-observations.json` など）と、account の SQLite（projection、DM・取り下げ・epoch 制御の outbox、private channel の参加者など）がある。
- 起動時に community-node.json と capability の registry を全件読み、capability を 1 件ずつ登録し直す。capability が変わるたびに registry 全体を書き直す（参加 channel 数 × epoch 数に比例する）。
- projection の trait（`Store` と `ProjectionStore` の 9 つの trait）の SQLite 実装は 9 file・約 3,300 行・query 約 126。`MemoryStore`（約 2,200 行）は試験向けで、一覧を全行の収集と整列で作り、上限も回収も無い。
  reload で失うと困る端末だけのデータ（DM の履歴と未送信の outbox、通知の既読、bookmark、mute、取り下げの outbox、owner の参加者の記録）が projection の trait にある。
- native の通信の復帰は、iroh の自動検知、`stack.rs` の差し替え、Community Node の 15 秒の tick で成り立つ。共通の復帰の入口は無い（#1196 は Open・Blocked）。`Endpoint::network_change()` は呼ばれていない。

ブラウザの事実:

- WebCrypto の `CryptoKey` は non-extractable のまま IndexedDB へ保存できる（AES-GCM は主要な browser で保存できる）。secp256k1 と iroh の鍵は WebCrypto に無いので、署名のときは WASM のメモリで復号する。
- Web Locks と BroadcastChannel は Chromium・Firefox・Safari 15.4 以降・Android Chrome で使える。lock は document の終了で自動的に解放される。
- IndexedDB の transaction の `complete` は既定で relaxed（OS への書込みの時点）で、`durability: 'strict'` を指定できる。
- Page Lifecycle の `visibilitychange`・`pagehide`・`pageshow` と、`online`・`offline` は全 browser で使える。`freeze`・`resume` は Chromium だけ。
- storage の LRU の eviction は origin 全体を消す。`navigator.storage.persist()` が許可された origin は対象外。Safari は 7 日間操作の無い origin の script が書いた storage を消す（ITP。ホーム画面の web app は対象外）。

## Decision

### 1. 保存 trait と IndexedDB の database

- `KeyringStore` を、同じ 3 つの操作（`purpose` と `key` で引く get・set・delete）の**非同期**の trait 1 つに置き換える。desktop-runtime の file で書く設定・状態の読み書きも同じ trait へ寄せる（ADR 0056 §2）。
  native の実装は今の keyring・file の読み書きを移したもので、Web の実装は IndexedDB で作る。新旧の経路を並べて残さない。
  trait の切り出しと native の呼出元の切替は W1 AC-4（desktop-runtime の command の接続）で行い、Web の実装は W4 AC-2 が作る。
- IndexedDB の database は 3 種類に分ける。

| database | 中身 | durability | 回収 |
| --- | --- | --- | --- |
| `kukuri-device-v1`（origin に 1 つ） | account の一覧、端末の iroh endpoint 秘密鍵、アプリの同意、秘密を包む AES-GCM の `CryptoKey`（non-extractable） | strict | しない |
| `kukuri-vault-v1-<account の公開鍵>` | `secrets[purpose, key]`（アカウント鍵・capability・token 等を AES-GCM で包んだもの）、`settings[name]`（設定と最小状態） | strict | しない（明示の削除だけ） |
| `kukuri-cache-v1-<account の公開鍵>`（ADR 0058） | blob・docs の record の保護と cache、projection（§2） | relaxed | ADR 0058 §4 の規則 |

- 保存の成功は transaction の `complete` を基準にする（`beforeunload` に頼らない）。quota・拒否・破損・schema の更新・部分的な保存を区別して返し、既存の identity を失敗の隠蔽のために作り直さない。
- 秘密の保護: vault の秘密は、device の database の non-extractable な `CryptoKey` で包む。これは profile の file を持ち出されたときの保護で、origin の中で動くコード（XSS・供給網）からは守れない（信頼境界）。常時のパスワード入力は加えない。
- 起動時に vault を全件読まない。起動に要るもの（アカウント鍵、有効な需要の capability、設定）を key で読む。

### 2. projection

- Web の projection（`Store` と `ProjectionStore` のすべての trait）は、ADR 0058 の cache の database に IndexedDB で実装する。SQLite の実装と同じ意味・同じ窓（件数・順序・上限）を、複合索引と cursor で作る。
- 端末だけのデータ（DM の履歴と outbox、通知の既読、bookmark、mute、取り下げと epoch 制御の outbox、owner の参加者の記録）は保護し、再取得できる行（remote の投稿の projection 等）は ADR 0058 §4 の規則で回収する。
  projection の更新と保護参照の置き換え（ADR 0058 §2）は同じ transaction で行う。
- Web の capability の外の機能（W1 AC-4・W8 の capability matrix で非対応とするもの）の trait の method は、共通の「この platform では使えない」error を返す。
- 採らない方式: `MemoryStore` で動かして reload で作り直す（端末だけのデータを失い、全行の収集と整列が件数に比例する）。一部の trait だけを IndexedDB にする（実装が 2 つになり、メモリ側も上限と索引を作り直すことになる）。

### 3. capability と件数

- private channel の capability は、channel（と epoch）ごとの行で保存し、有効な需要があるものだけを読む。起動時の全件の読み込み・登録し直しと、変更のたびの全件の書き直しを Web に持ち込まない。
- native の registry（1 つの JSON の全件読み・全件書き）は設計原則に反する既存の欠陥である。行ごとの保存への移行は、同じ保存 trait を使う W5 AC-4（(channel, epoch) の鍵の保持。各操作は対象項目だけを読む）で native と Web を同時に行う。
- account の一覧・Community Node の設定も、key ごとの行で保存する（起動時は選択中の account と、起動に要る node の設定だけを読む）。

### 4. 複数 tab

- origin の中で runtime を動かす tab を 1 つに限る。Web Locks の `kukuri-runtime-v1` を `ifAvailable` で取れた tab だけが runtime を起動する。account の切替は同じ tab の runtime の中で行う（ADR 0056 §7）。
- lock を取れなかった tab は「別のタブで使用中」を示し、利用者の明示の操作でだけ `steal` する。奪われた tab は lock の request の reject を受けて runtime を止め（世代の終了。旧世代の callback は反映しない）、同じ表示へ戻る。
- tab 間で状態を同期する仕組み（BroadcastChannel 等）は作らない。runtime を持つ tab が 1 つなので要らない。
- 端末固有の ID と iroh の endpoint 秘密鍵は device の database に 1 つで、tab ごとに複製しない。WebRTC の session も runtime の所有に従う（W9・W10）。

### 5. lifecycle と通信の復帰

- `desktop-runtime` の host に、共通の復帰の入口を 1 つ置く（理由: 可視・online・`pageshow` の bfcache からの復帰・`resume`）。入口は既存の処理を 1 回ずつ呼ぶだけにする。
  - `Endpoint::network_change()`（経路の再確認）
  - Community Node の期限・同意の判定を 1 回
  - DM・取り下げ・epoch 制御の outbox の due を、実行枠の分だけ同じ ID で再送
  - W10 への世代の通知（接続交渉は有効な需要だけ）
- Web の adapter（`crates/web-runtime`）は browser の event をこの入口へ渡すだけで、Web だけの retry の loop を作らない。Android（#1196）も同じ入口を使う。
- 全 topic・author・epoch の列挙や一括の再購読をしない。freeze・閉じた tab の間の接続の維持は約束しない。

### 6. データの喪失と復旧

- account の作成・import の後に `navigator.storage.persist()` を要求し、許可の状態を設定の画面に示す（W8）。
- app による cache の回収は vault と device の database を消さない。利用者のサイトデータの削除・browser の eviction（Safari の 7 日間の削除を含む）で失われた鍵は、既存の暗号化 export（ADR 0047）と QR・リンクの移行（#1211）から戻す。保持を偽らない。
- export の鍵の導出（argon2id 64 MiB）は main thread を数秒止めうる。明示の操作のときだけ行い、進行中を示す（W8）。

### 7. 所有する AC

| 内容 | 所有 |
| --- | --- |
| 保存 trait の切り出しと native の呼出元の切替 | W1 AC-4 |
| device・vault の IndexedDB の実装、projection の IndexedDB の実装、秘密の保護、保存の結果の区別 | W4 AC-2 |
| 共通の復帰の入口と Web の lifecycle の adapter | W4 AC-3 |
| 複数 tab の lock と引継ぎ | W4 AC-4 |
| persist の要求、喪失時の復旧の導線 | W4 AC-5 |
| capability の行ごとの保存（native と Web） | W5 AC-4 |

## Consequences

- W4 AC-2 は、vault と projection の IndexedDB の実装を 1 つの PR で行う（projection の SQLite の実装と同じ規模、約 3,000〜4,500 行と見込む）。
- `KeyringStore` の呼出元は非同期になる。desktop-runtime の起動・切替の経路の関数が async になる。
- origin の中で同時に使える tab は 1 つになる。

## Data classification

ADR 0002 の template に従う。データの意味と公開範囲は native と同じで、置き場所だけが違う。

- Feature 名: Web の鍵・設定・projection の保存
- Durable / Transient: device・vault の database は Durable。cache の database の保護行は Durable、非保護の行は Cache。
- Canonical Source: 本人の端末の保存先（native と同じ）。
- Replicated?: 変わらない。endpoint 秘密鍵・同意・CN の token は複製しない（#1213 D-14）。
- Rebuildable From: Cache は相手から再取得できる。秘密は export・移行からだけ戻せる。
- Public Replica / Private Replica / Local Only: device と vault の database は Local Only。
- Gossip Hint 必要有無: なし（変更なし）。
- Blob 必要有無: なし（変更なし）。
- SQLite projection 必要有無: Web は IndexedDB の projection。
- 必須 contract: W4 AC-2〜5 の判定方法。
- 必須 scenario: W8（#1220）の reload・復帰・消去からの復旧。
- 新しい外部送信: なし。

## References

- Issue #1213、#1217（本 ADR）、#1196、#1211、#1218
- ADR 0047・0048・0055・0056・0058
- W3C Web Locks、IndexedDB（durability）、Page Lifecycle、WebCrypto、MDN Storage quotas and eviction criteria

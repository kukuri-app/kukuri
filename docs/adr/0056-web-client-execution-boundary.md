# ADR 0056: Web クライアントをブラウザ内の Rust/WASM で動かす実行境界

## Status

Accepted（Issue #1214 W1 AC-1。実装は同 Issue の AC-2〜4 と #1215〜#1217・#1220・#1421・#1422 が段階的に行う）

## Context

インストールせずに URL を開いて kukuri に参加できるようにする（統括 Issue #1213）。本人のクライアントが鍵を持ち、署名・復号・P2P 参加を行う。
サーバーがそれらを代行する gateway は採らない（#1213 D-1）。ブラウザ自身が iroh の Endpoint を動かし、既存クライアントと同じ protocol で通信する。

現行のクライアントは React → Tauri invoke → `crates/desktop-runtime` → `crates/app-api` の順に呼び、下層は filesystem・redb・SQLite（sqlx）・tokio の
multi-thread runtime に依存する。基準 commit `8bd3badcf`（main、2026-09-30）で wasm32-unknown-unknown 向けの `cargo check` を実測した。

| 対象 | 結果 | 止まる原因 |
| --- | --- | --- |
| iroh（`KingYoSun/iroh` fork rev `4d7b079c`、上流 v1.3.0＋上流 #4447。#1422 W10 AC-1 で #4565 を足した `c47e860f` へ更新）、iroh-gossip（`net`）、iroh-docs、iroh-blobs | 既定 feature を外すと通る | なし（docs・blobs はメモリの store だけ） |
| `kukuri-core` | clang があれば通る | secp256k1-sys の C build |
| `kukuri-metaverse-host` | uuid に `js` を足すと通る（rapier3d を含む） | uuid の乱数源 |
| reqwest（`json`・`query`・`rustls`） | 通る | なし |
| `kukuri-store` | 通らない | sqlx（tokio の `net` → mio） |
| `kukuri-transport` | 通らない | `SqliteStore` の直接保持、n0-mainline（DHT、tokio の `net`） |
| workspace の tokio | 通らない | 共通 feature の `rt-multi-thread`・`fs`（root `Cargo.toml` の `tokio`） |
| iroh-docs・iroh-blobs を既定 feature で使う経路 | 通らない | `rpc` → noq の `runtime-tokio` → tokio の `net` |

実行時の制約（wasm32-unknown-unknown）:

- `std::time::Instant::now` と `SystemTime::now` は panic する。tokio の time driver と `tokio::spawn` は tokio runtime を要する。`chrono::Utc::now` は既定の `wasmbind` で動く。
- 共有 crate の非 test の使用数（数え方で数行の幅がある）: `Instant::now` 約 70 行、`SystemTime::now` 約 10 行、tokio の `timeout` 約 70 行・`sleep` 約 20 行、`spawn` 約 40 行（transport・iroh-node・docs-sync・app-api が大半）。
- iroh（`Endpoint`・`Router`・`Gossip`・`MemStore`・`DocsApi`）は wasm32 でも `Send + Sync` で、`connect`・`subscribe`・`bind` の future も `Send`（wasm32 で assert を check して確認）。
- iroh の wasm は UDP・DNS・portmapper・`bind_addr`・`bound_sockets` を持たない。relay には WebSocket で接続する。Custom Transport（`unstable-custom-transports`）は wasm でも有効。
- `RTCPeerConnection` は Window だけで使える（WebRTC 1.0 の IDL が `[Exposed=Window]`）。`RTCDataChannel` は Dedicated Worker へ transfer できるが、生成した task の中で `postMessage` したときに限る。

`AppService`（`crates/app-api/src/service/mod.rs`）は store・projection store・transport・hint transport・docs・blob を `Arc<dyn Trait>` で受け取り、
全 projection trait を実装した `MemoryStore`（`crates/store/src/memory`）で app-api の test が動いている。`crates/desktop-runtime`（非 test 約 1.86 万行）は、
keyring・fs・sqlx を直接使う module（identity・backup・paths・legacy 移行・`accounts/display.rs`、約 3.2k 行）と、fs を一部で直接書く module（stack・runtime/mod・accounts・host・community_node の保存系、約 6.2k 行）と、
platform の印が無い module（`runtime/*_api.rs`・community_node の通信系など、約 8.7k 行）に分かれる。frontend の Tauri command は 174 件で、約 140 件は `DesktopRuntime` の method を呼ぶだけの委譲である。

## Decision

### 1. 実行の主体と配置

- ブラウザの main thread（Window）で、1 つの WASM instance が kukuri の Rust runtime 全体（Endpoint・gossip・docs・blobs・app-api・host）を持つ。Worker は使わない。
- 理由: WebRTC の `RTCPeerConnection` は Window にしか無く（#1421 の browser backend が使う）、Worker へ移せるのは生成直後の `RTCDataChannel` だけである。
  Worker に置くと、main thread の接続交渉と Worker の QUIC を橋渡しする処理と、UI と runtime の間の command の橋渡しが増える。先行する iroh 用 WebRTC transport（SuddenlyHazel/iroh-webrtc-transport）も Worker 構成をやめ、main thread に一本化している。
- 結果として、runtime の 1 回の処理は main thread を占有する。設計原則（件数に依存しない処理）により 1 回の処理は上限つきなので、占有時間も上限つきになる。W8（#1220）で固定 workload の long task を計測する。Worker へ移すことは受入条件の変更として扱う。
- 例外（2026-10-02 のユーザー判断、#1220 W8 AC-2c）: 鍵の export・import の passphrase の鍵の導出（argon2id、64 MiB・3 回。ADR 0047 §1）だけを Dedicated Worker で行う。
  導出は件数でなく 1 回で決まる重さ（数百 ms〜数秒）で、上限つきの小さな処理に分けられないため。export の形式と KDF の parameter は native と同じで、後方互換は変えない。
  - Worker は導出ごとに作り、結果を受けたら止める。main thread の `WebAssembly.Module`（同じ web-runtime の wasm、`wasm_bindgen::module()`）を受け取り、glue を使わずに instantiate して、導出の export（`kukuri_kdf_alloc`・`kukuri_kdf_derive`）だけを呼ぶ。導出は JS の import を呼ばないので、import は呼ばれたら投げる関数で埋める（`crates/desktop-runtime/src/kdf.rs`・`kdf_worker.js`）。
  - Worker へ渡すのは passphrase・salt・parameter だけ、返すのは導出した鍵だけ。runtime・アカウント鍵・保存は Worker に置かない。native は同じ導出を blocking の thread で行う（`kukuri_core::encrypt_account_key_export`・`decrypt_account_key_export` は導出の実行を呼出し側から受ける）。
  - Worker の script は `wasm_bindgen::link_to!` の snippet。ADR 0060 §2 の CSP（`script-src 'self'`）のまま読めるよう、配信の build は `wasm-bindgen --split-linked-modules` で別の file にする（既定は `data:` の URL で、CSP が拒む。wasm-bindgen-test の runner は CSP を付けないので既定のまま動く）。Worker が glue を読む形は採らない（Vite で bundle した後は glue の URL が定まらない）。

### 2. crate の構成

- 既存 crate を target 別の依存で Web と共用する: `core`・`store`・`transport`・`iroh-node`・`docs-sync`・`blob-service`・`app-api`・`metaverse-host`・`desktop-runtime`。Web 用に同じ責務の runtime を別に作らない。
  `desktop-runtime` の名前は変えない（#886 で native の共通 host になっており、改名は利用箇所の書き換えだけを増やす）。
- `crates/web-runtime`（新規、`cdylib`、wasm-bindgen）を Web の入口にする。JS へ公開するのは §6 の API だけとし、業務ロジック・署名・権限判定を持たない。
  ブラウザ専用の保存 adapter（IndexedDB）もここに置く。vault と設定は #1217 W4、blob の保護・cache と自分の docs record は #1215 W2・#1216 W3 が実装する（ADR 0058）。
  iroh-blobs・iroh-docs は fork しない（#1213 D-3、2026-09-30 改訂）。
- QUIC over WebRTC DataChannel の transport crate は `crates/webrtc-transport`（ADR 0057、#1421 W9 AC-1）。
- `desktop-runtime` の platform 固有 module（keyring・fs・sqlx を直接使うもの）は file 単位で `cfg(not(target_family = "wasm"))` にする。
  fs を一部で直接書く module の読み書きは、既存の `KeyringStore`（`identity.rs`）と同じ形の保存 trait 1 つへ寄せる。native の実装は今の file 読み書きを移す。Web の実装は W4 が IndexedDB で作る。
- W1 AC-4d の実装: desktop-runtime は wasm32 で build できる（CI の wasm32 の clippy に入る）。native だけにしたもの:
  - device backup・restore と restore の調停（`backup`・`host/restore_lifecycle`）、旧 store の退役と保護データの移行
  - profile の dir と lock（`host/profile`）、他の account の DB を読む表示（`account_display`）、file path を受け取る media の読み出し（`get_blob_media_file`）
  - 通報の送信（ブラウザの fetch は転送を止められず、通報本文を別の host へ再送しない保証（#703）を保てない）。W1 AC-5 で Web でも送れるようにした（下記）
  - runtime と stack の組み立て（永続の node・SQLite の候補の台帳。`DesktopRuntime::new`・`from_env`、`SharedIrohStack::new`・`rebuild`）と、それを呼ぶ host の起動・切替・作成・logout。W1 AC-5 で Web の組み立てを足した（下記）
- W1 AC-5 の実装（2026-10-03）:
  - Web の runtime と stack の組み立て: `SharedIrohStack::open` が node の出所（`NodeSource::Persistent`（native）・`NodeSource::Memory`（保存した endpoint の秘密鍵と WebRTC の transport を渡すメモリの node））と候補・cache の store（`StackStore`。Web は `IndexedDbCache`）を受ける。作り直しも同じ出所で行う。`DesktopRuntime::open_in_memory_node` が Web の runtime を作る（旧 store は無いので新形式の writer で始め、discovery は保存した設定、無ければ static peer）。
  - host の起動・アカウントの作成・切替・logout・作り直しは、platform の組み立ての手順（`RuntimeBuilder`。native は `NativeRuntimeBuilder`、Web は web-runtime の IndexedDB とメモリの node）を受ける共通の処理にした（`ClientHost::start_if_consented_with`・`from_runtime_with_builder`）。
  - wasm の lib.rs の `dead_code`・`unused_imports` の許可を外した。native だけの処理（restore の登録・backup の検証）は cfg で native にした。
  - Community Node の HTTP は、応答の本文まで 10 秒の期限で打ち切る（native は client の期限、Web は request ごとの期限。`CommunityNodeHttpClient`）。
  - 通報の送信は、HTTP の 1 回（`post_report`）だけを platform で分けた。native は reqwest で転送を追わず 3xx を `REPORT_REDIRECT_REJECTED` にする。Web はブラウザの fetch を `redirect: "error"` と期限（`AbortSignal.timeout`）で呼び、転送を失敗にする（本文を転送先へ送らない）。送信先の構成・origin・同意の確認と応答の解釈は共通。
  - native だけの command（device backup・media file・他の account の表示）は、Web の dispatch で `unsupported_platform` を返す。

### 3. 依存と feature

- 時刻と task は、共用 crate で `n0_future::{time, task}` に一律で置き換える（`tokio::time`・`tokio::spawn`・`std::time::Instant::now`・`SystemTime::now` の使用箇所）。
  n0-future は native では tokio、wasm では web-time と wasm-bindgen-futures を使う（lockfile に既にある）。呼出箇所ごとの `cfg` 分岐は作らない。`tokio::sync` と `tokio::select!` はそのまま使う。
- workspace の tokio の共通 feature は `macros`・`sync`・`rt`・`time` にし、`rt-multi-thread`・`fs`・`net` は、共用 crate では native の target 節で、native だけの crate では通常の依存で有効にする。`time` は wasm32 でも compile でき、native だけの crate が使うので共通に残す（W1 AC-2）。
  `n0_future::time` に無い `timeout_at` は、期限までの残りを `timeout` に渡す helper で置き換える（W1 AC-4a。app-api の `timeout_at`）。
  そのため共用 crate が tokio・std の時刻と task を直接使っても wasm32 の build は通り、ブラウザで実行時に止まる。`clippy.toml` の `disallowed-methods` を共用 crate の lib.rs で wasm32 の時だけ warn にし、CI の wasm32 の clippy で検出する（native では `n0_future`・`web_time` がそれらの再公開なので検出しない）。
- iroh-docs・iroh-blobs は workspace で `default-features = false` とし、native だけ `fs-store` 等の必要な feature を有効にする。iroh は workspace で `unstable-custom-transports`（ADR 0057）を有効にし、既定の feature のまま wasm32 で build できる（W1 AC-2）。
- n0-mainline・iroh-mainline-address-lookup（DHT）は native だけで使う。ブラウザには UDP が無いので DHT は成り立たない。Web の接続先の発見は relay・Community Node の rendezvous・peer ticket に限る。未使用の pkarr 依存は W1 AC-2 で消した。
- uuid は wasm で `js` を有効にする。getrandom の backend は `.cargo/config.toml` の wasm32 target の rustflags（`getrandom_backend="wasm_js"`）で指定する。
- secp256k1 はそのまま使う。wasm の build には clang が要る（CI の Linux runner。Windows のローカルでは clang 入りの Docker image。手順は W1 AC-2 で `docs/runbooks/dev.md` に書く）。wasm だけ pure Rust の実装へ替えると、同じ鍵・署名の処理が 2 つになる。
- iroh は fork rev `c47e860f`（上流 v1.3.0＋#4447＋#4565 の cherry-pick。#4565 は接続中に custom path を追加する `Endpoint::add_remote_addrs`。#1422 W10 AC-1）を使い、Web 固有の差分を fork へ加えない（#1213 D-2）。
  iroh-docs・iroh-blobs は fork せず、上流の版（iroh-docs は root `Cargo.toml` の patch rev）を使う（#1213 D-3 の改訂）。

### 4. `Send`・`Sync` の境界

- kukuri の trait（`Store`・projection の各 trait・`Transport`・`HintTransport`・`DocsSync`・`BlobService`）の `Send + Sync` と、`async_trait` の `Send` future を変えない。`?Send` へは切り替えない（`spawn` の `Send` 要求が連鎖する）。
- ブラウザの JS オブジェクト（IndexedDB の handle、`RTCPeerConnection`、`RTCDataChannel`）は、`spawn_local` で起こした 1 つの task の中だけで持つ。Rust 側の型は、その task と channel でやり取りする `Send` の handle だけを持つ。
  `unsafe impl Send`・`SendWrapper` による偽装はしない（誤って別 thread から触れたときに panic へ変わるだけで、境界が型に残らない）。iroh の Custom Transport の trait も `Send + Sync` を要求するので、W9 の browser backend も同じ形にする。
- iroh-blobs は wasm で `Send` でない future・stream（blob の reader、remote の fetch）を返す。`kukuri_iroh_node::confine_local` が呼んだ時点で `spawn_local` の task へ移し、結果だけを channel で受け取る。待つ側が止めたら task も止める。native ではそのまま返す（W1 AC-2）。

### 5. 保存の境界（W2〜W4 の所有）

| データ | native | Web | 所有 |
| --- | --- | --- | --- |
| アカウント秘密鍵・private capability・世代・退会・採用済み version・設定・同意・CN の token | keyring・file | IndexedDB の durable 領域（cache の削除と分ける） | W4 AC-1〜2 |
| iroh の endpoint 秘密鍵（端末固有 identity） | account ごとの file | IndexedDB の durable 領域（account ごとの vault。ADR 0059 §1）。ブラウザと account ごとに生成し、アカウント同期・移行で複製しない | W4 |
| projection（`Store`・`ProjectionStore`） | `SqliteStore` | W4 AC-1 が IndexedDB の実装か、メモリと再構築かを決める（D-4・D-5）。W1 の転送試験は既存の `MemoryStore` を使い、これを永続化の証拠にしない | W4 |
| docs の replica | redb（persistent） | 上流の `Store::memory()`。自分の record は保存 trait の IndexedDB 実装（ADR 0058） | W3 |
| blobs | `FsStore` | 上流の `MemStore`（blob-service は書かない）と、保存 trait の IndexedDB 実装（ADR 0058） | W2 |
| remote read の cache と本人の書込みの保護（`SqliteStore` の直接保持。iroh-node・docs-sync・blob-service） | `SqliteStore` | 保存 trait（ADR 0058。`ContentCacheStore`）の IndexedDB 実装。W1 AC-2 で trait を切り出し、native の呼出元を trait object へ切り替えた | W1 AC-2・W2・W3 |
| peer candidate（transport の `account_store`） | `SqliteStore`（hot endpoint は 16 件で古いものから `MemoryLookup` から外す） | account の cache の database（ADR 0058）の IndexedDB に保存し、reload の後も使う。上限と回収は native と同じ定数（store の `PeerCandidateStore` の trait と `LEARNED_*`・`MAX_ADDR_BYTES`）。store を持たない経路（docs-sync の台帳）の learned・imported は、それぞれ 256 件（`STORELESS_PEER_LIMIT`）を超えたら古いものから台帳と `MemoryLookup` から外す。hot endpoint は store の有無によらず 16 件（W1 AC-2。保存は 2026-10-02 のユーザー判断で W4 AC-2 に改めた） | W1 AC-2・W4 AC-2 |
| file path を受け取る API（`BlobService::put_remote_blob_file`・`fetch_blob_ephemeral_to_file`、`get_blob_media_file`） | file | 使わない。media は payload と Blob URL の経路を使う | W1 AC-5・W8 |

保存・復元・cache の回収は、key・cursor・chunk で有界な単位にする（#1213「作業・設計原則を適用する境界」）。

### 6. JS への公開 API と command の接続

- `web-runtime` が JS へ公開するのは、`start(config)`・`shutdown()`・`invoke(command, args)`・`listen(callback)` の 4 つに限る。
  `invoke` は Tauri と同じ command 名・DTO（ts-rs の生成型）を受け取り、同じ形の結果・エラーを返す。`listen` は `RuntimeEvent`（`ClientHost::subscribe_events`）を渡す。
  既存の page・cursor つきの DTO をそのまま使い、上限の無い配列や namespace 全体の export を ABI にしない。
- frontend の差し替え点は 2 つに限る。command は `invokeDesktop`（`apps/desktop/src/lib/api/invoke/desktop.ts`）、`RuntimeEvent` は `useRuntimeEventBridge`（`apps/desktop/src/shell/data/useRuntimeEventBridge.ts`。今は Tauri の `listen` を直接使う）。
  Web の build では、それぞれを `web-runtime` の `invoke` と `listen` へ向ける。
  Tauri の API を直接使う file（updater・OS 通知・dialog・deep-link・`convertFileSrc`）は Web では使わない。対応状況は W8 の capability matrix で示す。
- `DesktopRuntime` の method へ委譲するだけの command は、`desktop-runtime` に置く 1 つの dispatch 表（command 名 → request の型 → method）から呼ぶ。
  `web-runtime` の `invoke` と Tauri の invoke handler の両方がこの表を使い、Tauri 側の約 140 の委譲 wrapper を消す（W1 AC-5）。
  起動中・終了中の command の受付の判定は、W1 AC-3 で host（`admit_command`）へ移した（src-tauri の `invoke_gate.rs` は Tauri の invoke をこの判定へ通すだけ）。Tauri 側に処理がある command のうち Web でも要るもの（起動・同意・アカウント切替の調停）は `desktop-runtime` の host へ移す（W1 AC-5）。Tauri 専用の処理（updater・tray・window・OS 通知・file dialog）は `src-tauri` に残す。
- Web で使えない command は、共通の error code で「この platform では使えない」と返し、frontend が capability として判別できるようにする（W1 AC-5）。
- W1 AC-5 の実装（2026-10-03）:
  - dispatch 表は `desktop-runtime` の `command.rs`。`DesktopRuntime` へ委譲する表と、起動の状態・アカウントの操作（作成・秘密鍵の取込み・切替・logout・初期 profile・一覧）の表の 2 つで、どちらも command 名・引数の key（Tauri と同じ camelCase）・結果の型を持つ。表に無い command は `unsupported_platform`、呼ぶ前と後で host の世代が違う委譲の結果は `stale_runtime` にする。引数の無い呼出しは空の object と同じに扱う。項目ごとの future は heap に置く（全項目の状態を 1 つの future に持つと debug build の stack を使い切る）。
  - アカウントの操作の調停（排他・終了中の確認・起動の状態の遷移・切替後の後始末）は、platform の状態を `ClientGate`（host・起動の状態・排他の lock・終了中の確認・切替後の hook）で受ける共通の関数にした。Tauri は app の state（切替後の hook は OS 通知の既読の位置）、Web は web-runtime の状態で実装する。
  - アプリの同意（`get_app_consent_status`・`accept_app_consents`）は、端末の復元の再開（native だけ）と起動の手順が platform で違うので、検証・記録の共通の関数を platform 側の command から呼ぶ。
  - Tauri の invoke handler は、受付の判定（`admit_command`）の後に表の command を表へ渡す。委譲の wrapper と identity・起動の状態・投稿の再試行の wrapper を消した（Tauri に残るのは Tauri 専用と native だけの command）。CLI の対応表の試験は、Tauri の登録と表の和を GUI の入口として数える。
  - web-runtime の JS API: `start(config)`（`config.communityNodeConfig` は初回の起動で保存する Community Node の設定。省略可）は、端末の保存（`BrowserStorage`）を入れ、同意があればアクティブなアカウント（無ければ作る）の runtime を始めて、起動の状態を返す。`invoke` は受付の判定の後に同意の command か表を呼び、結果か `{ code, message }` の error を返す。`listen` の callback は `shutdown` で外れ、止めた後の event を受けない。`shutdown` は行っている操作の終わりを待ってから runtime を止める。

### 7. 起動・停止・アカウント切替

- runtime の世代・需要の owner・停止は、既存の `desktop-runtime` の host（restore lifecycle）と `NetworkWorkRuntime`（ADR 0055）を使う。Web のための独立した retry や全再購読を作らない。
- JS から入る callback（event、IndexedDB の完了、WebRTC の event）は runtime の世代を持ち、停止・切替の後に届いた旧世代の callback は反映しない（W1 AC-3）。W10（#1422）の接続交渉と候補の登録も同じ世代に属する。
- W1 AC-3 の実装: `ClientHost` が runtime の世代を持つ（起動で 1、runtime の差し替え（切替・restore・restart）と停止のたびに進む。`generation()`）。event は差し替えの時点の世代を付けて配り、`ClientEventReceiver::recv` は今の世代でない event を返さない（差し替え・停止の前に host まで届き、まだ受け取っていない event も含む）。node の停止（`IrohDocsNode::shutdown`）は WebRTC の交渉の世代も終え（`Signaling::reset`）、交渉中・開いた session を閉じる。command の結果の世代の照合は、Tauri と web-runtime が共用する dispatch 表で行う（W1 AC-5）。

### 8. 通信経路

- Web の Endpoint は relay（WebSocket）と W9 の Custom Transport だけを持つ。native の経路（UDP・DHT・relay）と優先順位（Direct P2P → Relay Supported P2P → Relay Fallback）は変えない。
- Web では relay が初回の接続と fallback を担う。relay で接続交渉して WebRTC DataChannel に実データを流す通信は Relay Supported P2P、実データが relay を通る通信は Relay Fallback として区別する（W10）。

### 9. 実装の順序

1. W1 AC-1（本 ADR）→ W9 AC-1（transport の判断）→ W9 AC-2（transport crate と、ブラウザでの試験環境）
2. W1 AC-2（共用 crate の wasm build・依存の分離・Endpoint の組立てへの transport の注入口・ブラウザ↔native の有界な読み出し）→ W10 AC-1
   W1 AC-2 の wasm build は Endpoint と有界な reader までの crate（core・store・transport・iroh-node・docs-sync・blob-service・webrtc-transport）。app-api・metaverse-host は W1 AC-4a、desktop-runtime は W1 AC-4d で加える（2026-10-01 に W1 AC-4 を 4a〜4d に分割）。
3. W1 AC-3（host の世代・event の隔離・gate の移設・交渉の世代。native で完結）→ W1 AC-4（desktop-runtime・app-api の wasm 化と保存 trait）→ W1 AC-5（command の dispatch 表・web-runtime・capability）（2026-10-01 のユーザー判断で旧 AC-4 を AC-4・AC-5 に分割）

W2・W3 の AC-1（保存 trait の操作の固定。ADR 0058）は W1 AC-2 より前に行う。W4 の AC-1 は本 ADR の後に並行して進める。W5・W6 の規則は native で先に実装できる。

## 採らない方式

- Worker に runtime を置く: §1。WebRTC の接続交渉と QUIC の間の橋渡しが増え、`RTCDataChannel` の transfer の条件も守り続ける必要がある。
- Web 用の runtime crate を別に作る: `desktop-runtime` の印の無い約 8.7k 行を重複させる。
- SQLite を WASM で使う（sqlite-wasm-rs・rusqlite）: sqlx は wasm に対応しない。store の約 1 万行を別の driver へ書き直すことになり、OPFS の VFS は Worker を要する。
- wasm だけ pure Rust の secp256k1 を使う: 同じ鍵・署名の処理が 2 つになる（§3）。
- 呼出箇所ごとに `cfg` で tokio と wasm を分ける: §3 の置き換えより分岐が多くなる。
- gateway・サーバーでの署名・復号: #1213 D-1。

## Consequences

- 共用 crate は wasm32 の build を保つ必要がある。W1 AC-2 で CI（`linux-web-transport`）に wasm32 の clippy と、ブラウザ↔native の読み出しの browser 試験を足した。
- main thread の占有は、1 回の処理の上限で抑える。重い処理の Worker への移動を、件数依存の解消の手段にしない（`AGENTS.md`）。
- 共用 crate で `tokio::time`・`tokio::spawn`・`Instant::now` を直接使わない規則が増える。W1 AC-2 で置き換え、wasm32 の clippy（§3 の `disallowed-methods`）で新しい使用を検出する。
- `desktop-runtime` は Web でも使う。platform 固有の module は file 単位の `cfg` で分かる形にする。

## Data classification

ADR 0002 の template に従う。対象は「ブラウザ内の Web クライアントの実行」。データの意味と公開範囲は native と同じで、置き場所だけが変わる。

- Feature 名: Web クライアント（ブラウザ内の Rust/WASM runtime）
- Durable / Transient: アカウント秘密鍵・private capability・設定・最小状態・endpoint 秘密鍵は Durable（IndexedDB の durable 領域）。projection と投稿の cache は Cache。WASM と JS の配布物は静的な資産。
- Canonical Source: native と同じ（署名済み envelope、本人の local の秘密の保存先）。
- Replicated?: native と同じ protocol で複製する。endpoint 秘密鍵・CN の token・同意は複製しない。
- Rebuildable From: projection と cache は docs・blobs と peer から再取得できる。秘密鍵は明示の移行・export からだけ戻せる。
- Public Replica / Private Replica / Local Only: native と同じ。endpoint 秘密鍵・CN の token・同意は Local Only。
- Gossip Hint 必要有無: native と同じ。
- Blob 必要有無: native と同じ。
- SQLite projection 必要有無: native だけ。Web の projection は W4 AC-1 で決める。
- 必須 contract: 共用 crate の wasm32 build（CI）、ブラウザ↔native の有界な読み出し（W1 AC-2）、世代の隔離（W1 AC-3）、command の capability（W1 AC-5）。
- 必須 scenario: W8（#1220）の実ブラウザの主要導線と復帰。
- 新しい外部送信: Web の配布物の取得（静的 host。W8 AC-6）、STUN（W9・W10）。relay・Community Node への通信は native と同じ。

## References

- Issue #1213（統括）、#1214（本 ADR）、#1215・#1216・#1217・#1220・#1421・#1422
- ADR 0052・0053・0054・0055、`docs/architecture/network-work-inventory.md`、`docs/architecture/p2p-first-community-node-responsibility-boundary.md`
- iroh の WASM 対応: https://docs.iroh.computer/languages/wasm-browser
- WebRTC 1.0（`RTCPeerConnection` の Exposed）: https://w3c.github.io/webrtc-pc/ 、WebRTC extensions（`RTCDataChannel` の transfer）: https://w3c.github.io/webrtc-extensions/
- iroh-docs PR #75（wasm の memstore）、iroh-blobs PR #259・#86（外部 store。open）、iroh PR #4565（open）

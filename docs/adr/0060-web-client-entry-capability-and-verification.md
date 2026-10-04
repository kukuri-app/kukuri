# ADR 0060: Web クライアントの entry・capability・配信条件と実ブラウザの検証環境

## Status

Accepted（Issue #1220 W8 AC-1。実装と検証は同 Issue の AC-2〜6）

## Context

Web クライアントは、既存の React の UI と共通の domain・API を使い、ブラウザ内の Rust/WASM（`crates/web-runtime`、ADR 0056）を本人の client として動かす。
W8 は Web の entry と build、共有 UI の adapter、capability matrix、配信できる artifact と、実ブラウザでの統合受入を所有する。

基準（統合 branch `b6533bdd0`）の事実:

- `apps/desktop` の build は `tsc --noEmit && vite build` で、`vite.config.ts` は Tauri 専用の設定を持たない。Playwright は `VITE_KUKURI_DESKTOP_MOCK=1` の Web の bundle を `vite preview` で実ブラウザに配信している。
- UI・API・i18n は `apps/desktop/src` に約 63,000 行ある（components 約 33,000、shell 約 24,000、lib 約 7,900、i18n 約 8,100）。Tauri 専用の UI（window close、link preview、system panel の一部）は `isTauriRuntime()` で止まる。
- Tauri の command は 174 件（`apps/desktop/src-tauri/src/lib.rs` の `generate_handler!`）。
  - (a) `DesktopRuntime` の method を呼ぶだけ: 134 件（profile・投稿・反応・DM・private channel・topic・Community Node・live/game/Dome など）
  - (b) 端末固有: 19 件（updater・window・OS 通知・外部 URL・developer log・system locale・link preview の HTTP 取得・file の media）
  - (c) Web でも要るが処理が Tauri 側にある: 21 件（identity 10、同意 2、起動 1、device backup 6、app の版を AppHandle から読む `accept_community_node_consents`・`enable_community_node_observation_sharing` の 2）と `invoke_gate.rs` の起動中・終了中の gate
- `wasm-bindgen --target web` の出力は、Vite が plugin 無しで asset として読む。`WebAssembly.instantiateStreaming` には CSP の `'wasm-unsafe-eval'` と `application/wasm` の MIME が要る。
- LP（`kukuri.app`）は Cloudflare Pages で `_headers` の CSP を付けて配信している。Pages の 1 file の上限は 25 MiB。
- CI の runner: ubuntu-24.04 には Chrome・Firefox と各 driver、macOS 15 arm64 には Safari と safaridriver がある。標準の Linux runner は KVM を使えるので Android の emulator を動かせる。

## Decision

### 1. Web の entry と build

- `apps/web` は作らない。`apps/desktop` に Web の build mode（`VITE_KUKURI_TARGET=web`、出力は `dist-web`）を足す。UI・API・i18n・Playwright の設定を共有し、package・lockfile・lint の設定を増やさない。
- `main.tsx` は Web の mode で `web-runtime` の WASM を初期化（`init()`）してから `App` を描く。WASM は `wasm-bindgen --target web --split-linked-modules` の出力を Vite の asset として読み、main thread で動かす（ADR 0056 §1）。`--split-linked-modules` は鍵の導出の Worker の script（ADR 0056 §1 の例外）を別の file にし、§2 の CSP のまま読めるようにする（既定の `data:` の URL は CSP が拒む。#1220 AC-2c）。
- ADR 0056 §6 の 2 つの差し替え点（`invokeDesktop`・`useRuntimeEventBridge`）を、Web の mode で `web-runtime` の `invoke`・`listen` へ向ける。DesktopApi を丸ごと置き換える mock の方式は Web の本番の経路に使わない。
- Tauri の API を静的に import する file は bundle に入ってよい（実行時に `__TAURI_INTERNALS__` を触らない限り動く）。Web で使わない機能は §3 の capability で止める。
- W8 AC-2a の実装（2026-10-03）: Web の mode は `VITE_KUKURI_TARGET=web`。`@kukuri/web-runtime` を wasm-bindgen の出力（`apps/desktop/web-runtime-pkg`、または `KUKURI_WEB_RUNTIME_PKG`）へ解決し、Tauri の build では使えないことを示す代わりの module へ解決する（Tauri の bundle に WASM を入れない）。`main.tsx` は描画の前に `start` を呼ぶ（Community Node の初期設定は native と同じ配布の設定。開発・試験は `VITE_KUKURI_COMMUNITY_NODE_BASE_URL`）。web-runtime の `listen` の callback は外せないので、frontend は 1 つだけ渡して購読者へ配る。device backup の復元の反映と他の account の表示は、Web では呼ばない。live・game・metaverse・Dome の 35 件の command は desktop-runtime の native だけの表に移し、Web の dispatch では `unsupported_platform` を返す。
- 配信の artifact は「WASM の build → `wasm-bindgen --target web --split-linked-modules` → Web の mode の Vite の build → `_headers`」を 1 つの command にまとめる（W8 AC-6。`cargo xtask` の入口にする）。

### 2. 配信の条件

- CSP（`_headers`）: `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src 'self' https: wss:; img-src 'self' blob: data:; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`。
  `style-src` の inline は、依存の UI 部品が `<style>` を差し込むため（Tauri の CSP と同じ）。script の inline は許さない。
  `connect-src` の `https:`・`wss:` は Community Node の API と iroh relay（WebSocket）のため。第三者の script・analytics を読み込まない。
- `.wasm` は `Content-Type: application/wasm` で配る。COOP・COEP は付けない（SharedArrayBuffer・thread を使わない）。
- secret を扱う Web クライアントは LP と別の origin に置く（同じ origin の第三者 script から IndexedDB の vault を守るため。ADR 0059 §1 の信頼境界）。公開の URL・DNS・公開の時期は artifact の完成と別の操作とする（#1220 の Non-goals）。
- release build の WASM の大きさを AC-6 で測り、25 MiB を超えるなら分割・圧縮を AC-6 の中で決める。

### 3. capability matrix

| 分類 | 件数 | Web での扱い |
| --- | --- | --- |
| (a) `DesktopRuntime` への委譲 | 134 | ADR 0056 §6 の dispatch 表から、Web でもそのまま使う。ただし live・game・metaverse・Dome の 34 件は今回の合意の外なので、Web では「この platform では使えない」を返し、画面で未対応と示す |
| (b) 端末固有 | 19 | Web では「この platform では使えない」を返す。media は payload と Blob URL の経路（ADR 0056 §5）、locale は `navigator.languages`、外部 URL は通常の link で置き換える。link preview の HTTP 取得は Web では行わず、投稿者が書いた record を (a) の `read_link_preview_record` で読む（ADR 0051 §7、#1220 AC-2d で追加） |
| (c) Tauri 側に処理がある | 21 | 起動・同意・account 切替・identity の処理と起動 gate を host へ移して Web でも使う（起動 gate は W1 AC-3、残りは W1 AC-5）。device backup（完全な端末 backup）は Web では使えないとし、鍵の export・import（ADR 0047）と QR・リンクの移行（#1211）で代える |

- 必須の主要導線（初回の同意、account の生成・移行・切替、profile、topic、投稿・返信・反応、DM、private channel、設定）は、すべて (a) のうち Web で使う command と (c) の移設で成り立つ。matrix だけで省略しない。
- 最終の matrix は W1 AC-5 の capability の実装と一緒に確定し、W8 AC-5 の実測の結果と合わせて記録する。
- 2026-10-03 のユーザー判断（W8 AC-2a）: 「画面で未対応と示す」は、Web では使えない機能（live・game・metaverse・Dome、device backup・restore、アプリの更新、ウィンドウを閉じるときの設定、OS 通知の設定、開発者ログの閲覧）の入口を出さず、設定の About に短い説明を 1 つ出す形で満たす。

### 4. 実ブラウザの検証環境

| browser | 実行先 | 判定する範囲 |
| --- | --- | --- |
| desktop Chromium | ubuntu-24.04 の CI（Chrome と chromedriver） | 主要導線・復帰・native↔Web と Web↔Web の直接経路・fallback |
| desktop Firefox | ubuntu-24.04 の CI（Firefox と geckodriver） | 主要導線・復帰・native↔Web の直接経路・fallback |
| Safari | macOS 15 arm64 の CI の実 Safari（safaridriver。WebKit の build や Node.js で代替しない） | 主要導線・復帰・native↔Web の直接経路・fallback |
| Android Chrome | ubuntu-24.04 の CI の Android emulator の Chrome（chromedriver の Android の操作） | 主要導線・復帰・native↔Web の直接経路・fallback |

- どの browser でも、直接経路は「同じ job の中で起動した native の node との、host の候補での接続」（到達できる固定 fixture）で判定し、fallback は W10 の T2（UDP の遮断・ICE の失敗）の fixture で判定する。
  relay だけで主要導線が通ったことを、直接経路の判定の PASS にしない（実データの経路と bytes で判定する。ADR 0057 §8）。
- 4 つの browser を、W3C WebDriver の 1 つの driver（WebdriverIO。chromedriver・geckodriver・safaridriver・Android の chromedriver を同じ API で操作できる）で操作し、同じ scenario の手順を流す。既存の desktop の Playwright の試験はそのまま残す。
- job の中で作れない組（emulator の NAT で UDP が通らない等）は、実測した結果を未確認の制約として matrix に記録し、PASS にしない。

- 2026-09-30 のユーザー決定: Safari は CI の実 Safari、Android は CI の emulator で判定する。手元の Mac・Android の実機は使わない。
- CI で作れない条件（別の機器との同じ LAN の直接経路、実回線の切替、モバイル回線）は、Chromium と Linux 実機（`local2`）で確かめ、Firefox・Safari・Android では確かめない制約として matrix に記録する。未確認を PASS にしない。
- native の相手は、CI の job の中で起動する kukuri の native の node とする。direct と fallback の判定は実データの経路と bytes で行う（ADR 0057 §8、W10）。

- W8 AC-2a の実装（2026-10-03）: Chromium の主要導線の試験は `cargo xtask-lite web-e2e`（CI の `linux-web-e2e`）。
  - 相手は harness の `web_e2e_fixture`。同じ process で、in-process の Community Node（user-api と iroh relay。Web の配信の origin を CORS で許可する）、Community Node に同意した native の相手、`dist-web` の配信を動かす。driver（WebdriverIO）は `/fixture/*` で native を Tauri・Web と同じ dispatch 表で操作する。
  - 直接経路と fallback は、受け手が画像（毎回違う乱数の画素の PNG）を取得する間に relay が中継した bytes で判定する。ブラウザには relay と WebRTC の他の経路が無いので、中継が画像より十分小さければ WebRTC を通っている。双方の診断の EndpointId（ブラウザの CONNECTED PEERS と native の接続先）も照合する。
  - fallback の fixture（W10 の T2 の ICE の失敗）は、ページより先に動く script で、送る SDP と受け取る SDP から ICE の候補を除いた Chrome とする。Chrome の UDP を抑える起動の設定（`--force-webrtc-ip-handling-policy=disable_non_proxied_udp`）では ICE が成立し続け、fallback にならなかった。
- W8 AC-2b の実装（2026-10-03）: 同じ試験で、DM と private channel を直接経路の端と fallback の端で確かめる。
  - DM は native↔Web と Web↔Web で送り合う。相互 follow は、相手の profile を開いた時（author の購読の開始）の読みで確かめる（follow の offer は宛先の探索が有界で、届かないことがある）。
  - private channel は、どちらの端でも、Web が作った channel に native と別の Web が共有リンクで参加し、native が作った channel に Web が参加して、投稿を行き来させる。
  - 実データの経路は、AC-2a と同じく画像を添えた DM と channel の投稿で判定する（native→Web と Web→Web）。native の画像は driver が command に base64 で添える。
  - 主要な 3 つの設定（表示と言語、成人向け表示、Community Node の node の保存・同意・認証の状態）は、経路に依らないので直接経路の端だけで確かめる。
  - 作成・参加の dialog を閉じると channel の列が画面外に残る不具合（#1517、desktop と共通）の間は、利用者と同じく列を画面に入れてから投稿を確かめる。
- W8 AC-2g の実装（2026-10-03）: 同じ試験の最後に、リンクプレビュー（ADR 0051 §7）を確かめる。
  - native（投稿者）は OGP を取得せず（試験の site は ADR 0051 §3 の宛先の制限で取得できない）、fixture の手順で自分の公開投稿の record（試験の題と小さい PNG）を書く。Web は card と画像（record の画像と同じ data URL）を示し、record の無い投稿は URL だけを示す。
  - Web は record の無い結果を 60 秒持つので、Web が表示していない topic に投稿して record を書いてから、Web をその topic へ切り替える。
  - 最後に native を止め、新しく開いた Web にも、record を読んだ Web から card と画像が出ること（AC-2f の中継）を確かめる。試験の Community Node は docs を保持しないので、この card は参加者の中継による。
- W8 AC-4 の実装（2026-10-04）: 同じ試験の直接経路の端（Web a）で、fallback の端を開く前に、reload・終了・凍結・回線全断・WebRTC の経路だけの喪失が W4 の保存と復帰の入口（ADR 0059 §4〜§6）へつながり、退会・世代・version を巻き戻さないことを確かめる。reload・終了・凍結・回線全断の各段の後に、投稿の行き来と native の画像の直接経路（relay の中継 bytes）を確かめる。
  - reload: 同じアカウント・EndpointId・設定（runtime が保存する成人向け表示）・private channel の列の下書きで再開し、初回の同意・profile の dialog は出ない。reload の間の投稿も、利用者が取り直す操作（列の開き直し・再読込）をしなくても出る。先頭にいる列では「Show N new posts」の新着として示されるものを含む（この button は受け取り済みの投稿を並べるだけで、取得はしない。2026-10-04 ユーザー判断）。
  - 終了: 別の tab が「このタブで使う」で引き継ぐと、元の tab の runtime は止まって WebRTC の session を全て閉じる。新しい tab は保存から同じ EndpointId で再開する。
  - 凍結: 利用者と同じく、非表示 → freeze → resume → 表示の順にする。chromedriver の freeze は page を非表示にしてから凍結し、resume の後も非表示のまま戻さないので、CDP の `Emulation.setFocusEmulationEnabled` で表示へ戻す（画面は非表示の間は列を読み直さない）。freeze で旧い session を閉じ（旧世代の candidate・callback の解放）、復帰では生きた需要の相手とだけ交渉し直す（試行は有界）。
  - 回線全断: chromedriver の回線の模擬（`setNetworkConditions` の offline）。offline の間、接続の案内はつながっていないことと次の手順を示し、DM は送信待ち（Pending）と示す。online の後、DM は同じ id で 1 回だけ届いて Delivered になる。
  - WebRTC の経路だけの喪失（relay は健全）: page より先に動く script で、画像の転送の途中に DataChannel を閉じる（RTCPeerConnection を外から閉じても runtime に event が届かない）。relay で完了し、表示した画像は原本と同じ hash で、card は 1 つ。表示の取得は 15 秒の転送の期限で打ち切られて relay で取り直すので、転送中の stream の継続は #1482 で判定する（2026-10-04 ユーザー判断）。
  - 旧 state の再送: 退会した channel は、owner の世代の更新（旧い参加への配布）と reload の後も戻らない。更新した世代は reload の後も読み書きできる。native から見た profile の版は reload で変わらない。
  - 履歴の量: reload と引継ぎの前に native が投稿を足し、2 つの量のどちらでも、最初の頁は 20 件以下、閉じていない WebRTC の session は需要のある相手の数以下になる。件数に比例しないことは、W4（ADR 0059）・W10（ADR 0057）・#1221 の試験に対応付ける（2026-10-04 ユーザー判断）。
  - 復帰の直後の 1 回目の交渉は、閉じた経路が選ばれ続ける間（#1482）に期限が切れる。直接経路は次の試行（約 30 秒後）で戻る。

### 5. 測定の workload と STUN

- 接続の確立時間・blob の転送・CPU とメモリ・queue の高水位・relay を通った実データの bytes を、ADR 0057 §8 の固定 workload（E1〜E6、S1〜S3）で測る。W9・W10 の予算と判定方法を使い、測定だけで高速化や費用の削減を宣言しない。
- 開発と試験の STUN は、試験の環境の中で動かすもの（native の相手と同じ job）を使う。本番の STUN（Community Node の基盤、ADR 0057 §6）への反映は別の Issue にまとめる。

### 6. 説明とデータ分類の変更点（AC-6）

- `docs/legal/app-data-flow-inventory.md`: 対象に Web を加え、秘密鍵の保存先に IndexedDB の vault（ADR 0059）、DHT・P2P・relay の行に STUN と ICE の候補、新しい行に静的配信の取得（配信元へ IP・User-Agent・Referer）とブラウザの storage・persist を足す。自動更新と developer log は Web では該当しないと書く。
- `docs/legal/external-transmission-notice.md`: 対象に Web を加え、「Web の配信元」と「STUN」の行を足し、relay の行に WebSocket を書く。legal bundle の版を上げる。LP への同期は既存の手順（`apps/lp/scripts/sync-legal.mjs`）を使う。

## 採らない方式

- `apps/web` を新設する: package・lockfile・vite・tsconfig・lint・Playwright の設定と、約 63,000 行を共有する仕組みが増える。
- DesktopApi を mock と同じく丸ごと置き換える: 174 件分の差し替えを別に持つことになる。差し替え点は 2 つで足りる。
- `wasm-bindgen --target bundler`: Vite の plugin が要る。
- COOP・COEP を付ける: thread を使わないので要らず、第三者の資源の読込みの制約だけが増える。

## Consequences

- `apps/desktop` の build は、Tauri と Web の 2 つの mode を持つ。Web の mode は Tauri の API を実行時に触らないことを、Playwright の Web の試験で確かめる。
- live・game・metaverse・Dome は Web では未対応として示す。

## Data classification

ADR 0002 の template に従う。対象は「Web クライアントの配信と実行」。

- Feature 名: Web クライアントの配信（静的な JS・WASM・資源）
- Durable / Transient: 配信物は静的な公開物。ブラウザの保存は ADR 0058・0059。
- Canonical Source: repository の build（`cargo xtask` の Web の build）。
- Replicated?: 配信元（静的 host）が配る。
- Rebuildable From: 同じ commit から再 build できる。
- Public Replica / Private Replica / Local Only: 配信物は公開。秘密を含まない。
- Gossip Hint 必要有無: なし。
- Blob 必要有無: なし。
- SQLite projection 必要有無: なし。
- 必須 contract: §2 の header（CSP・MIME）と、secret が URL・cookie・analytics・ログへ出ないこと（W8 AC-6）。
- 必須 scenario: §4 の matrix の主要導線と復帰（W8 AC-2〜5）。
- 新しい外部送信: 配信元への取得（IP・User-Agent・Referer）、STUN（ADR 0057 §6）。

## References

- Issue #1213、#1220（本 ADR）、#1214、#1211、#1421、#1422
- ADR 0014・0047・0056・0057・0058・0059
- Vite の WebAssembly、Cloudflare Pages の上限、GitHub Actions の runner image（ubuntu-24.04、macos-15-arm64）、Android の emulator の KVM

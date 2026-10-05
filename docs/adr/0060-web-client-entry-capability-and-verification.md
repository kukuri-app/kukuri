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
- W8 AC-6 の実装（2026-10-04）: command は `cargo xtask web-build` で、出力は `apps/desktop/dist-web`。
  - WASM は LTO の profile（root の `Cargo.toml` の `web-release`。fat・codegen-units 1）で build し、wasm-bindgen で名前の section と producers の section を除く（§2 の大きさ）。
  - Community Node は配布の設定を使う（試験の URL を埋め込まない）。
  - Vite の Web の build は `apps/desktop/web-public`（`_headers` だけ）を public にする。`public` の metaverse の資源は Web では使わないので含めない。
  - build の後に、各 file が 25 MiB 以下であることを確かめ、超えたら失敗にする。
  - 試験（`cargo xtask web-e2e`）の build は、試験の Community Node を埋め込み、速さと失敗時の調べやすさのため LTO と名前の section の除去をしない。
  - 手順は `docs/runbooks/web-client-publish.md`。

### 2. 配信の条件

- CSP（`_headers`）: `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src 'self' https: wss:; img-src 'self' blob: data:; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`。
  `style-src` の inline は、依存の UI 部品が `<style>` を差し込むため（Tauri の CSP と同じ）。script の inline は許さない。
  `connect-src` の `https:`・`wss:` は Community Node の API と iroh relay（WebSocket）のため。第三者の script・analytics を読み込まない。
- `.wasm` は `Content-Type: application/wasm` で配る。COOP・COEP は付けない（SharedArrayBuffer・thread を使わない）。
  W8 AC-6 では `_headers` の `/*.wasm` の規則で付ける（Cloudflare Pages の `_headers` の文書に、拡張子の規則 `/*.jpg` の例がある）。
- secret を扱う Web クライアントは LP と別の origin に置く（同じ origin の第三者 script から IndexedDB の vault を守るため。ADR 0059 §1 の信頼境界）。公開の URL・DNS・公開の時期は artifact の完成と別の操作とする（#1220 の Non-goals）。
- release build の WASM の大きさを AC-6 で測り、25 MiB を超えるなら分割・圧縮を AC-6 の中で決める。
- W8 AC-6 の測定（2026-10-04。wasm-bindgen の出力）: 既定の release は 33.8 MB で、上限を超える。
  - 名前の section の除去だけで 26.1 MB（24.9 MiB）、LTO（fat・codegen-units 1）だけで 25.2 MB になる。
  - 両方で 20.3 MB（19.4 MiB）になる。機能を変えないので、これを採る。
  - opt-level を `s` に下げると 15.1 MB になるが、実行の速さが変わり得るので採らない。
- W8 AC-6 の確認: 実ブラウザの E2E の fixture は、artifact の `_headers` の header を付けて配信する。試験の Community Node と relay は http・ws の 127.0.0.1 なので、CSP の connect-src にだけそれを足す。E2E は、各 client の CSP の違反（`securitypolicyviolation`）が 0 件であることを確かめる（WASM の起動、鍵の export・import の Worker、画像の表示を含む）。

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
  - DM は native↔Web と Web↔Web で送り合う。相互 follow は、相手の profile を開いたまま、同じ列に「Message」が出るのを待って確かめる（#1220 AC-2h、2026-10-04）。
    - 相手の follow の offer は、送れなかったら約 2 分送り直される（#1521 AC-1a）。届くと、開いている列が読み直される（AC-1b）。
    - それまでは、profile を閉じ、相手の follow の後に開き直して確かめていた。
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
  - WebRTC の経路だけの喪失（relay は健全）: page より先に動く script で、1 本の DataChannel が画像の半分ほどを受け取った時点で、開いている DataChannel をすべて閉じる（RTCPeerConnection を外から閉じても runtime に event が届かない。1 本だけ閉じると、他の相手との session で relay を通らずに完了しうる。#1549）。画像が出るまでは、新しい session の SDP から ICE の候補を除いて WebRTC の経路を失ったままにする（直接経路が先に戻ると relay を通らずに完了し、判定が時機に依る）。
    同じ取得が relay で続いて完了する（#1482 J2。閉じた custom path を iroh がすぐ閉じる。ADR 0057 §5）。閉じてから表示まで 10 秒以内で、relay の中継は画像の大きさの 1/4 を超え、画像の大きさ未満（取り直しではない）。表示した画像は原本と同じ hash で、card は 1 つ。hash は、画面が blob の URL を作った Blob から読む（AC-6 の CSP の connect-src は `blob:` を許さないので、`fetch` では読まない）。
  - 旧 state の再送: 退会した channel は、owner の世代の更新（旧い参加への配布）と reload の後も戻らない。更新した世代は reload の後も読み書きできる。native から見た profile の版は reload で変わらない。
  - 履歴の量: reload と引継ぎの前に native が投稿を足し、2 つの量のどちらでも、最初の頁は 20 件以下、閉じていない WebRTC の session は需要のある相手の数以下になる。件数に比例しないことは、W4（ADR 0059）・W10（ADR 0057）・#1221 の試験に対応付ける（2026-10-04 ユーザー判断）。
  - 復帰の直後の 1 回目の交渉は、閉じた経路が選ばれ続ける間に期限が切れ、直接経路は次の試行（約 30 秒後）で戻っていた。#1482 で、閉じた custom path を iroh がすぐ閉じるようにした（閉じてから 2 秒以内に同じ接続が relay で続くことは #1482 の J1、reset の後の resume が 1 回の交渉で開くことは W10 J4・J5 の `resume_renegotiates_only_the_live_demand` で確かめた）。
- #1559 の実装（2026-10-04）: 主要導線の試験を、経路で分けた独立した scenario にした（2026-10-04 ユーザー判断）。上の AC-2a・2b・2g・AC-4 の実装の「同じ試験で」「同じ試験の最後に」「fallback の端を開く前に」という段の順と、AC-3b（#1561）の段の位置は、この構成に置き換わる。
  - 各 scenario は、新しい fixture（Community Node の DB・rendezvous の key・native）と新しい client で始め、前提（相互 follow・channel への参加・成人向け表示の有効化など）もその中で作る。fixture の Community Node は rendezvous の key に fixture ごとの DB の名前を使うので、同じ valkey で動く別の fixture と topic の在席情報を共有しない。
  - 判定を持つ scenario: `direct`（AC-2a・2b の直接経路）、`fallback`（AC-2a・2b の fallback）、`settings`（AC-2b の主要な設定）、`link-preview`（AC-2g）、`lifecycle`（AC-4 の reload・終了・凍結・回線全断と、r6・r9 の証跡。2026-10-04 ユーザー判断）、`webrtc-loss`（AC-4 の WebRTC の経路だけの喪失、#1482 J2）、`site-data`（W4 AC-5）、`transfer`（AC-3b）、`same-account`（AC-3c1 の同じアカウントの Web どうしの担当の不在・引継ぎ・Web → Web の移行と、移行の途中の経路の切替。AC-3c2 の経路だけを落とした間の担当でない端末の鍵の更新）。各 scenario の最後に、その client の CSP の違反が 0 件であることを確かめる（AC-6）。
  - 直接経路の判定（relay の中継 bytes）を持つ scenario は、その判定を終えるまで fallback の端を開かない。fallback の端の relay の通信が判定に混ざらない。`lifecycle` は、AC-4 の段の前に a が DM で画像を送り（r6）、AC-4 の段の後に、AC-4 より前に a が作った channel へ fallback の端が token で参加する（r9）。
  - CI は、build の job（`linux-web-e2e-build`）が wasm・Web・fixture を 1 回だけ作り、scenario ごとの job（`linux-web-e2e (<scenario>)`）が並列に回す。失敗はその scenario の job にだけ出る。merge の条件は全 scenario の job の成功とする（2026-10-04 ユーザー判断）。
  - 今後の AC の段で、既存の scenario に自然に属さないものは、新しい scenario にする（2026-10-04 ユーザー判断）。
- W8 AC-5a の実装（2026-10-04）: 同じ scenario を desktop Firefox で回す。
  - 上の 2 つの job は、再利用の workflow `kukuri-web-e2e.yml` に置く。Fast は Chrome で呼ぶ（merge の条件）。同じ workflow を、夜間（schedule）と手動（workflow_dispatch）で Firefox で回す。Firefox の job は merge の条件にせず、失敗は Issue にする（2026-10-04 ユーザー判断）。schedule と workflow_dispatch は、workflow が default branch に入ってから動く。
  - driver は、ブラウザを `KUKURI_WEB_E2E_BROWSER`（`chrome`・`firefox`）で選ぶ。Firefox は geckodriver で操作し、ページより先に動く script は Chrome と同じに動く（AC-5b で fixture の配信に移した）。
  - fixture の native は、WebRTC の候補を loopback でない既定の経路の IP で出す。Firefox は loopback の候補と組を作らず、`127.0.0.1` の候補では ICE が成立しなかった。Chrome も同じ候補で接続する。
  - 回線全断は、Firefox では WebDriver BiDi の `emulation.setNetworkConditions`（offline）で作り、Chrome と同じ判定（offline の間の案内と送信待ち、online の後の 1 回だけの配送と直接経路での再開）で確かめる。Chrome は chromedriver の命令のままにする（BiDi の offline では、online の後に WebRTC の session が開き直らなかった）。
  - 凍結（と、復帰の後の focus の模擬）は Firefox の driver で作れない。Firefox では未確認の制約とし、PASS にしない（`lifecycle` の PASS の行に `unconfirmed: freeze …` と示す）。
  - Firefox の JSON の viewer は止める（特権の文書になり、`site-data` の page から保存先を消せない）。
  - 実結果（2026-10-04、Firefox 156.0、PR #1573 の CI。一時に Fast から Firefox も呼んだ run）: `settings`・`link-preview`・`transfer`・`webrtc-loss` は毎回 PASS。`lifecycle` は凍結を未確認として 3 回とも PASS。`site-data` は、重なった初回の dialog の閉じ方を直した後に PASS。direct と fallback は relay の中継 bytes で分かれた（native→Web の画像は、直接経路で 17〜47 KB、fallback で画像の大きさ 1.77 MB 以上）。`direct`・`fallback` には、Web 間の画像が受け手に出ない失敗が時々あり、#1577 にした。
- W8 AC-5b の実装（2026-10-05）: 同じ scenario を macOS 15 arm64 の runner の実 Safari で回す。
  - safaridriver は同時に 1 つの session しか開けない。各 scenario の最初の Web の client だけを Safari にし、2 台目からは同じ runner の Chrome にする（2026-10-05 ユーザー判断）。Safari どうしの組は未確認の制約とする。
  - job は `kukuri-web-e2e.yml` の `macos-web-e2e-build`（fixture を macOS で作る）と `macos-web-e2e (<scenario>)`（Web の build は `linux-web-e2e-build` のものを使う）。夜間と手動で回し、merge の条件にしない。macOS の runner には Docker が無いので、Postgres・valkey は Homebrew のものを `cargo xtask-lite web-e2e --no-compose` で使う。Chrome は job の途中で自動更新され runner の chromedriver と合わなくなるので、Chrome for Testing を driver と組で使う。
  - ページより先に動く試験の script（`apps/desktop/tests/web-e2e/page-init.js`）は、fixture が配信の `index.html` に同じ origin の script として足す。Safari の driver には WebDriver BiDi が無い。どのブラウザでも同じ script を使い、配信の artifact は変えない。fallback の端は driver が置く cookie で示す。
  - safaridriver の制約への対処:
    - Chrome の client が動くと、Safari の page が focus を失い、押下と入力が page に届かない。押下・入力の前に `open -a Safari` で focus を戻す（AppleScript の `activate` では、Safari が最前面になっても page に focus が戻らなかった）。
    - key の操作は、同じ文字が続くと 2 つ目を落とすので、本文は要素への入力で入れる。
    - 表示していない要素の文も返すので、dialog は表示も確かめて選ぶ。
    - file の input へ入力できないので、画像は page で File を作って入れる。
  - 未確認の制約（`unconfirmed:` として PASS の行に示す）:
    - 凍結と、復帰の後の focus の模擬。回線全断。Safari の driver で作れない。
    - Safari と同じ runner の Chrome の間の WebRTC の経路。同じ runner では直接の経路が張れず、Web↔Web の画像は relay を通った（約 2〜4 MB）。Web↔Web の直接経路は Safari の判定の範囲の外（上の表）なので、測った bytes だけを示す。移行の途中にこの経路を落とす段（AC-3c1）も作れない。
  - 実結果（2026-10-05、Safari 26.6.1、PR #1584 の CI。一時に Fast から Safari も呼んだ run 37268403845）:
    - `direct`・`fallback`・`settings`・`link-preview`・`webrtc-loss`・`site-data`・`transfer`・`same-account` は PASS。
    - native→Web の画像は、直接経路で 16〜46 KB、fallback で画像の大きさ 1.77 MB 以上の中継になり、2 つの経路が分かれた。
    - `lifecycle` は「終了」の段で落ちる。別の window を開くと、別の tab で開いている案内が出ずに app が動く。#1586 にした。
- W8 AC-5c の実装（2026-10-05）: 同じ scenario を、ubuntu-24.04 の Android の emulator（電話の幅、412×783 の CSS px）の Chrome で回す。
  - Android の chromedriver は同時に 1 つの session しか持てない。各 scenario の最初の Web の client だけを emulator の Chrome にし、2 台目からは同じ runner の desktop の Chrome にする（2026-10-05 ユーザー判断）。Android どうしの組は未確認の制約とする。emulator は電話の幅にし、Android の client の操作は狭い画面の作りで行う（同日ユーザー判断）。
  - job は `kukuri-web-e2e.yml` の `android-web-e2e (<scenario>)`（android-emulator-runner、API 35 の google_apis、x86_64、pixel_7。build は `linux-web-e2e-build` のもの）。夜間と手動で回し、merge の条件にしない。`scripts/ci/android_web_e2e.sh` が emulator の Chrome の版に合う chromedriver を取る（Chrome for Testing。115 より前は旧い配布先）。
  - emulator の `127.0.0.1` から、fixture の配信・Community Node・relay へは adb の port の転送（TCP だけ）で届かせる。fixture の `/fixture/info` が転送する Community Node と relay の port を示す。
  - Android の chromedriver の制約への対処（試験の側だけ）:
    - 入力の欄に focus があると画面の keyboard が出て表示の範囲が縮み、chromedriver の押下が下へずれて button に当たらない。押下と入力の後に、表示の範囲の高さが落ち着くのを待ち、keyboard が出ていれば戻る key で閉じる。IME の停止や物理の keyboard の指定では、keyboard は出続けた。
    - 狭い画面では、別の列の要素は横の画面の外にあり、下の端の要素は下の固定の帯に覆われる。押す前に要素を列ごと画面の中央へ動かす。中心が覆われていれば、利用者と同じく見えている部分を押す（列のページの表示が列の右下の操作に重なる。#1588）。
    - `setValue` は文字が入らないことがあるので、値を入れて input の event を出す（貼り付けと同じ入り方）。1 文字ずつ打つ操作は投稿の本文で確かめる。`getValue` は textarea の値を空で返すので、page で読む。手元の file を file の input へ入れられないので、画像は page で File を作って入れる。
    - WebdriverIO の `newWindow` は電話の platform では使えないので、WebDriver の命令で tab を開く（全ブラウザ共通）。列は列の menu から閉じる（電話の幅では列の見出しの閉じる button は出ない。全ブラウザ共通）。
    - 凍結と回線全断は、desktop の Chrome と同じく chromedriver の命令で作る。
  - 未確認の制約（`unconfirmed:` として PASS の行に示す）:
    - Android どうしの組。
    - Android と同じ runner の Chrome の間の WebRTC の経路。emulator の NAT の外の Chrome との直接の経路は張れず、Web↔Web の画像は relay を通った（約 2〜4 MB）。Web↔Web の直接経路は Android の判定の範囲の外（上の表）なので、測った bytes だけを示す。移行の途中にこの経路を落とす段（AC-3c1）も作れない。
    - `lifecycle` の復帰の直後の native→Android の直接経路。emulator の NAT の上で揺れ、3 回とも画像の 1/4 を超えて relay を通った回があった（47 万〜90 万 bytes）。そのときは測った bytes を示す。
  - 実結果（2026-10-05、emulator の Chrome 124.0.6367.219、PR #1587 の CI。一時に Fast から Android も呼んだ run 37304034027）:
    - 9 本すべて PASS。`lifecycle` は凍結と回線全断も Android で作って確かめた。
    - native→Android の画像は、直接経路で 16〜61 KB（channel の 1 回は 416 KB）、fallback で画像の大きさ 1.77 MB 以上の中継になり、2 つの経路が分かれた。
    - 電話の幅の列のページの表示が列の右下の主操作に重なる不具合を #1588 にした。

- W8 AC-5d1 の記録（2026-10-05）: 4 つのブラウザの実結果の matrix。scenario は `main-flow.mjs` の 9 本で、主要導線（初回の同意、account の生成・移行・切替、profile、topic、投稿・返信・反応、DM、private channel、主要な 3 つの設定）と復帰（reload・終了・凍結・回線全断・WebRTC の経路だけの喪失・サイトデータの消去）を含む。経路は relay の中継 bytes で判定した（native→Web の画像 1.77 MB）。

| browser | 実行先・版 | PASS | 未確認の制約 | 制限・不具合 | run |
| --- | --- | --- | --- | --- | --- |
| desktop Chromium | ubuntu-24.04、Chrome 154、chromedriver | 9 本すべて（直接経路 16〜46 KB、fallback 1.77 MB 以上、Web↔Web の直接経路は数百 bytes まで）。別の機器との同じ LAN の直接経路も PASS（AC-5d2） | 実回線の切替、モバイル回線 | `same-account` の場面 5 が時々落ちる（#1590、試験の時機） | [37306057045](https://github.com/kukuri-app/kukuri/actions/runs/37306057045) |
| desktop Firefox | ubuntu-24.04、Firefox 156.0、geckodriver | 8 本（`same-account` は AC-5a の時点で無く、未実行） | 凍結と focus の模擬、`same-account` | Web 間の画像が受け手に出ないことがある（#1577） | [37204873801](https://github.com/kukuri-app/kukuri/actions/runs/37204873801)・[37205906717](https://github.com/kukuri-app/kukuri/actions/runs/37205906717) |
| Safari | macOS 15 arm64、Safari 26.6.1、safaridriver（2 台目からは同じ runner の Chrome） | 8 本（`lifecycle` 以外） | 凍結と focus の模擬、回線全断、Safari どうし、同じ runner の Chrome との WebRTC の経路 | 別の window で別の tab の案内が出ない（#1586、`lifecycle` が落ちる） | [37268403845](https://github.com/kukuri-app/kukuri/actions/runs/37268403845) |
| Android Chrome | ubuntu-24.04 の emulator（API 35、電話の幅）、Chrome 124、chromedriver の Android の操作（2 台目からは同じ runner の Chrome） | 9 本すべて（凍結と回線全断を含む。直接経路 16〜61 KB、private channel の 1 回は 416 KB） | Android どうし、同じ runner の Chrome との WebRTC の経路、復帰の直後の emulator の NAT の上の直接経路の揺れ | 電話の幅で列のページの表示が主操作に重なった（#1588、PR #1589 で修正済み） | [37304034027](https://github.com/kukuri-app/kukuri/actions/runs/37304034027) |

  - 設定のうち、主要な 3 つ（表示と言語、セーフティの成人向け表示の切替、Community Node の接続設定）は E2E で判定した。その他の設定（About / Legal、Keyboard、Account、Connectivity、Discovery、Reactions、Release、Developer）は、Web で使える（画面に出る）と記録し、E2E では判定しない（2026-10-03 ユーザー判断）。Account の鍵の export・import と、Connectivity・Discovery の診断は、`site-data` と各 scenario の接続の確認で使っている。
  - CI で作れない条件（別の機器との同じ LAN の直接経路、実回線の切替、モバイル回線）は、4 つのどれでも CI では判定していない。Chromium と `local2` での判定は AC-5d2 が所有する。Firefox・Safari・Android では確かめない制約とする。
- W8 AC-5d2 の判定（2026-10-05）: CI で作れない条件を、desktop Chromium と Linux 実機 `local2` で判定した。
  - 別の機器との同じ LAN の直接経路: native の相手（fixture）を `local2`（Ubuntu 24.04、192.168.10.28）で、Web を手元の Windows 11（192.168.10.2）の Chrome 154 で動かした。Web が Community Node・relay・fixture の操作へつなぐ TCP は ssh の port の転送で `local2` の `127.0.0.1` へ通した（配信の origin を secure context の `127.0.0.1` に保つ）。WebRTC（UDP）は ssh を通らず LAN を直接通る。
    - `direct`: PASS。native→Web の画像（1.77 MB）は relay を 8〜217 KB しか通らず、直接経路だった。Web↔Web（同じ Windows の 2 つの Chrome）は 12〜35 KB。
    - `fallback`: PASS。ICE の候補を除いた端では、画像は relay を 1.9〜4.8 MB 通った。
    - 根拠: CI の build の成果物（run 37306057045 の `web-e2e-build`）を `local2` に置き、`main-flow.mjs` の `direct`・`fallback` を回した。`local2` の Postgres・valkey は一時の container で、判定の後に消した。
  - 実回線の切替とモバイル回線: 手元の機器の操作（Wi-Fi の切替・携帯の tethering）が要るので、未確認の制約とする（2026-10-05 ユーザー判断）。PASS にしない。
  - 未確認を PASS にしない: 未確認の段は、各 scenario の PASS の行に `unconfirmed:` として出る。

### 5. 測定の workload と STUN

- 接続の確立時間・blob の転送・CPU とメモリ・queue の高水位・relay を通った実データの bytes を、ADR 0057 §8 の固定 workload（E1〜E6、S1〜S3）で測る。W9・W10 の予算と判定方法を使い、測定だけで高速化や費用の削減を宣言しない。
- W8 AC-5d3 の実測（2026-10-05）: 固定 workload の接続の時間・転送・CPU 時間と最大 RSS・`bufferedAmount` の高水位・relay を通った bytes を、ADR 0057 §8 に記録した。native の UDP・relay の既存の試験は PR CI で全件成功し、回帰していない。
- 開発と試験の STUN は、試験の環境の中で動かすもの（native の相手と同じ job）を使う。本番の STUN（Community Node の基盤、ADR 0057 §6）への反映は別の Issue にまとめる。
  - 実ブラウザの E2E は、fixture が同じ process で `cn-stun` を 3478 番に起動する（#1590）。STUN が応答しないと、browser の offer は候補集めの上限（3 秒、ADR 0057 §7）まで待ち、行き違いの交渉で相手の offer を待つ側の経路の確立が遅れる。

### 6. 説明とデータ分類の変更点（AC-6）

- `docs/legal/app-data-flow-inventory.md`: 対象に Web を加え、秘密鍵の保存先に IndexedDB の vault（ADR 0059）、DHT・P2P・relay の行に STUN と ICE の候補、新しい行に静的配信の取得（配信元へ IP・User-Agent・Referer）とブラウザの storage・persist を足す。自動更新と developer log は Web では該当しないと書く。
- `docs/legal/external-transmission-notice.md`: 対象に Web を加え、「Web の配信元」と「STUN」の行を足し、relay の行に WebSocket を書く。legal bundle の版は上げない（2026-10-04 ユーザー判断。8 のまま再同意を求めず、外部送信表示の「version 8 補記」として記録する）。
  - Web の利用者は、初回の同意でこの内容に同意する。
  - native の送信に加わるのは、Web との接続の交渉での STUN（relay と同じ host）だけで、送信先の運営者・目的・項目（IP address とポート）は relay の行の範囲内である。
  - プライバシーポリシーと利用規約は変えないので、LP への同期は要らない。
- W8 AC-6 の実装（2026-10-04）: 上の 2 つの文書を改めた。一覧の「確認上の境界」に、Web の秘密の出口（URL・cookie・analytics・公開索引・診断）の照合の結果を書いた。
- 画面の設定の「リリース」の「外部送信の確認」も Web に合わせる（2026-10-04 ユーザー判断。AC-6 の監査の N-1）。Web では、更新確認の項目を出さず（見出しの説明からも「更新」を外す）、接続の項目を Web の内容（DHT を使わず、同じサーバーの STUN を含む）にし、配信元の項目を出す。

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

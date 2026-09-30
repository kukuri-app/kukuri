# ADR 0057: iroh の QUIC を WebRTC DataChannel で運ぶ transport

## Status

Accepted（Issue #1421 W9 AC-1。実装は同 Issue の AC-2、接続交渉・経路制御は #1422 W10）

## Context

ブラウザには UDP が無く、iroh の Endpoint は relay（WebSocket）経由でしか通信できない（ADR 0056 §8）。ブラウザが関わる通信でも実データを relay に流さず直接運ぶため、
iroh の QUIC パケットを WebRTC DataChannel で運ぶ（#1213 D-1・D-15・D-16）。EndpointId の認証・E2E 暗号化・既存の ALPN（gossip・docs の有界 reader・blob）はそのまま使う。

基準 commit `8bd3badcf` と、iroh の fork rev `4d7b079c`（上流 v1.3.0＋上流 #4447）の source で確認した事実:

- Custom Transport は feature `unstable-custom-transports` の `iroh::endpoint::transports` にある（semver の保証外）。`Builder::add_custom_transport(Arc<dyn CustomTransport>)` で登録する。
  trait は `CustomTransport::bind`・`CustomEndpoint`（`watch_local_addrs`・`create_sender`・`poll_recv`・`max_transmit_segments`）・`CustomSender`（`is_valid_send_addr`・`poll_send`）で、すべて `Send + Sync + 'static` を要求する。wasm でも無効化されていない。
- 送受信の単位は datagram。`Transmit::segment_size` がある場合は複数の datagram に分かれる。`max_transmit_segments` の既定は 1（GSO なし）。custom transport 専用の MTU の設定は無く、QUIC の MTU discovery に任せる。
- アドレスは `TransportAddr::Custom(CustomAddr)`（u64 の id と任意長の bytes）。`Endpoint::addr()` と address lookup には載らない。id の登録簿（`TRANSPORTS.md`）は 0x00〜0x1F が予約、0x20 が Test。
- 既定の path selector は、custom を IP と同じ primary（優遇なし、RTT で比較）、relay を backup として扱う。selector が見るのは開いている path だけで、
  既存の接続の途中で custom path を開く公開 API は 1.3.0 に無い。上流 PR #4565（`Endpoint::add_remote_addrs`、+134 −1、open）がその API を足す。
- ブラウザの `RTCPeerConnection` は Window だけで使える。SDP に `max-message-size` が無ければ 64 KiB とみなされ、libwebrtc の SCTP は 1 packet の user payload が約 1160 byte である（推定）。
  QUIC の datagram（1200 byte 以上）は SCTP で 2 つの chunk に分かれうるので、再送しない設定では片方を失うと datagram ごと失う。
- str0m 0.24.0（MIT / Apache-2.0、Sans-IO）は、`rust-crypto` backend で Windows 上に build・実行できた（2026-09-30）。DataChannel の `ordered=false`・`Reliability::MaxRetransmits { retransmits: 0 }`・`negotiated`、
  `buffered_amount`・`set_buffered_amount_low_threshold`・`ChannelBufferedAmountLow` を持つ。wasm では使わない（ブラウザには UDP が無い）。
- 既存の iroh 用 WebRTC transport は 2 つある。SuddenlyHazel/iroh-webrtc-transport（crates.io 0.1.0-alpha.2、iroh 0.98、webrtc-rs）と haydenflinner/iroh-webrtc-transport（未公開、iroh 1.3、str0m 0.19、browser 側は JS）。
  どちらも kukuri の iroh fork rev・browser の Rust 実装・資源の上限・停止の契約を満たさない。
- iroh-relay は STUN を持たない（上流 #3546 で削除）。kukuri に STUN の設備は無い。

## Decision

### 1. crate と backend

- 新しい crate `crates/webrtc-transport`（package `kukuri-webrtc-transport`）に閉じる。iroh の `unstable-custom-transports` を有効にするのはこの crate だけにし、不安定な API をこの crate の外へ出さない。
- 共通部分: `CustomTransport` の実装、アドレスの形、session の登録簿、資源の上限、W10 へ渡す session の API。
- native backend（`cfg(not(target_family = "wasm"))`）: str0m 0.24（`default-features = false, features = ["rust-crypto"]`。aws-lc の C build を避け、Windows・Linux で同じ backend を使う）。
  ICE の UDP は session ごとに 1 つの tokio の socket を持つ（session 数の上限 §4 で socket 数も上限つき）。
- browser backend（`cfg(target_family = "wasm")`）: web-sys の `RTCPeerConnection`・`RTCDataChannel`。ADR 0056 §1・§4 のとおり main thread で動かし、
  JS オブジェクトは session ごとの `spawn_local` の task の中だけで持つ。`CustomEndpoint`・`CustomSender` はその task と channel でやり取りする `Send` の handle だけを持つ。
- 外部の iroh 用 WebRTC transport の crate は依存しない（Context）。str0m と web-sys は直接使う。

### 2. DataChannel と datagram の形

- session ごとに DataChannel を 1 本だけ持つ。binary、`ordered=false`、`maxRetransmits=0`、`negotiated=true`（stream id 0）、label `kukuri-quic/1`。順序と再送は QUIC の stream が担う。
  `negotiated=true` では label は相手へ送られないので、transport の版の互換は W10 の専用 ALPN の値で判定する。
- DataChannel の 1 message に QUIC の datagram を 1 つだけ載せる。`segment_size` のある `Transmit` は segment ごとに 1 message へ分けて送る（各 segment は独立した QUIC の datagram なので、再組立ては要らない）。
  `max_transmit_segments` は 64 にする。iroh は Endpoint の GSO の batch 数を全 transport の最小値にする（`socket/transports.rs` の `max_transmit_segments`）ので、1 にすると native の UDP の GSO まで止まる。
- 送れる datagram（segment）の最大は 16 KiB（DataChannel の相互運用で安全な上限）。これを超える datagram は送らずに破棄する（UDP の MTU 超過と同じ扱いで、QUIC の MTU discovery が下げる）。
  QUIC の MTU は Endpoint 全体の設定なので変えない。SCTP の分割による損失の増え方は AC-2 の固定 workload で計測し、記録する。
- SCTP の輻輳制御と DTLS の暗号化は QUIC と重複する。二重の暗号化は受け入れ、AC-2 で転送量と CPU を記録する（W8 の固定 workload でも計測する）。
- native（str0m）では、SCTP の送信 buffer（約 128 KiB）が `bufferedAmount` の高水位より先に満ちる。`write` が受け付けなかった datagram は捨てて数える（§4 と同じく QUIC の再送に任せる）。

### 3. アドレスと session

- `CustomAddr` の id は `0x4B4B5752`（ASCII の `KKWR`、kukuri WebRTC）。kukuri の中だけで使う値で、iroh の登録簿へは出さない。
- `CustomAddr` の data は session id（16 byte の乱数）と、session を始めた側か受けた側かの 1 byte（両端の addr を区別するため）の 17 byte だけとする。秘密鍵・capability・account 由来の値・SDP・ICE の credential を入れない（#1421 INVAR-2）。
- session は（相手の EndpointId、session id、runtime の世代）に束縛する。`is_valid_send_addr` は開いている session の addr だけを受け付ける。
- 相手の本人確認は QUIC が行う（新しい接続は TLS の EndpointId 認証、既存の接続への path 追加は接続の鍵で暗号化された path の検証）。DataChannel の相手が誰でも、QUIC の外で届いた bytes は信用しない。
  Endpoint の認証を account・audience の認可の代わりにしない（#1421 INVAR-1）。
- custom のアドレスは address lookup・peer ticket・公開の索引へ載せない。W10 の接続交渉で相手へだけ渡す。

### 4. 資源の上限と停止

| 資源 | 上限 | 満杯・超過時 |
| --- | --- | --- |
| 1 つの Endpoint が同時に持つ session | 16 | 新しい session を作らず、既存の経路（relay 等）を使う（受付は W10 の需要の owner が判定） |
| 送信の待ち（Rust 側） | session ごとに 64 datagram | その `Transmit` を捨てて `Ok` を返す。捨てた数を診断に出す |
| DataChannel の未送信 bytes（`bufferedAmount`） | 高水位 1 MiB、低水位 256 KiB | 高水位に達したら低水位を下回る（`bufferedamountlow`、str0m は `ChannelBufferedAmountLow`）まで、送る datagram を捨てる |
| 受信の待ち | Endpoint ごとに 256 datagram | 捨てる（UDP の損失と同じ扱い）。捨てた数を診断に出す |
| 1 datagram | 16 KiB | 送らない・受け取らない |
| SDP | 16 KiB | session を作らない |
| 相手の SDP の ICE の候補 | session ごとに 32 件、1 件 512 byte | 超える SDP を受け付けない（session を作らない） |
| session の event（`Opened`・`Closed`） | transport ごとに 64 件の channel | session の上限（16）の 2 倍を超えるので溢れない |

- 送信が満杯のときは、UDP の送信 buffer が溢れたときと同じく datagram を捨てる。iroh は custom の sender が `Pending` を返してもその datagram を捨てて `Ok` を返す（`socket/transports.rs` の `poll_send`、`"transport pending, dropped transmit"`）ので、
  `Pending` で QUIC の送信を止めることはできない。QUIC は捨てた datagram を損失として再送し、輻輳制御が送信量を下げる。
  このため backend は `Pending` を返さず、自分で捨てて `Ok` を返し、捨てた数を数える。

- session の close・cancel・runtime の世代の終了で、PeerConnection・DataChannel・socket・task・待ちの buffer を解放し、登録簿から消す。
  閉じた session の JS callback が後から届いても、登録簿に無い session には何もしない。未送信の datagram を送信成功として扱わない（QUIC の再送に任せる）。
- 満杯時は待つか捨てるかのどちらかで、上限の無い queue を作らない。

### 5. 経路の選択と、接続途中の経路の追加

- iroh の既定の path selector を使う。独自の selector は作らない。browser と native・browser の間では custom（primary）が relay（backup）より優先される。
  native 同士では custom の session を作らないので、既存の UDP（primary）がそのまま使われる（#1213 D-16）。
- relay で始めた接続へ、後から custom path を足すには上流 #4565 の `Endpoint::add_remote_addrs` が要る。
  これを iroh の fork（`KingYoSun/iroh`）へ載せた（上流の汎用 API であり、Web 固有の差分ではない。#1213 D-2）。W10 AC-1 で、fork の branch `kukuri/add-remote-addrs-v1.3.0` の rev `c47e860f`（上流 v1.3.0＋#4447＋#4565 の cherry-pick）へ更新した。
- W9 AC-2 の試験は、custom のアドレスだけで接続する（relay を使わない）ので #4565 に依存しない。
- 経路の削除は、session の close で行う。閉じた session の addr は `is_valid_send_addr` が false になり、`poll_recv` もその addr の datagram を返さない。
  iroh はその path を検証の失敗・idle で閉じ、既定の selector が relay 等の残りの path へ移る。iroh 側に path を消す API は要らない。
- 依存の owner: iroh の fork rev は W10 AC-1（#4565 を載せる）が更新する。iroh-blobs・iroh-docs は fork しない（#1213 D-3、2026-09-30 改訂）。本 crate はそれらに依存しない。
  #1032 の版更新は #1450 で先行したので、本 crate の依存の owner にしない。

### 6. STUN

- STUN の提供元は Community Node の基盤で自前で運用する（2026-09-30 ユーザー決定）。`cn-iroh-relay` は純粋な iroh relay のまま、STUN は別の process とする。
- 送信先・利用の可否（direct-only・relay 無効・同意の設定との関係）と、Community Node の基盤への配置は W10 が所有する。本番への反映は別の Issue にまとめる。開発と試験は手元の STUN を使う。
- native backend の server reflexive の候補も同じ STUN から得る（iroh の QAD は iroh 自身の socket の値で、session の socket には使えない）。TURN は使わない（#1213 D-16）。

### 7. W10 へ渡す session の API

W10 は認証済みの iroh 接続上の専用 ALPN で SDP と候補を交換し、次の API を呼ぶ。API は上限（§4）を検査し、超えた入力を拒否する。

- `offer(remote: EndpointId) -> (SessionId, SDP)`・`answer(remote, SessionId, SDP) -> SDP`・`accept_answer(SessionId, SDP)`。ICE の候補は SDP に含め、trickle しない（接続交渉を 1 往復にする）。
  browser は候補を集め終えるまで最大 3 秒待ち、過ぎたら集まった候補で返す。native は session の socket の host の候補を含める（server reflexive の候補は W10 が STUN と一緒に足す）。
- `close(SessionId)`、session の event（`Opened { session, remote, addr: CustomAddr }`・`Closed { session, reason }`）、診断と試験の数（`stats`）
- 専用 ALPN の値・同時の交渉数・期限・再試行は W10 AC-1 で次のとおり決めた（`crates/webrtc-transport/src/signaling.rs`）。需要・経路選択・診断との接続と STUN は W10 AC-2。
  - ALPN は `/kukuri/webrtc-signal/1`。既存の iroh の接続（relay 等）で届く相手へ、1 本の bi stream で要求（版 1 byte・宛先の EndpointId・session id・offer の SDP）と応答（answer の SDP か、拒否の理由）を 1 往復させる。要求は SDP の上限（§4）＋49 byte まで。
  - session は、QUIC の TLS で認証された接続の相手の EndpointId・session id・交渉の世代へ束縛する。要求の中の送り手の値は使わず、宛先が自分でない要求は拒否する。拒否の理由は版・宛先違い・満杯・同時開始・offer の不正・世代の終了。
  - 同時の交渉は両方向あわせて 4、自分から始める交渉は相手ごとに 1。期限は交渉の開始から DataChannel が開くまで 15 秒。満杯・期限切れ・拒否では session を残さない。自動の再試行はしない（需要の owner が決める）。
  - 両端が同時に始めたら、EndpointId が小さい側が受けた要求を拒否し、大きい側は受けた要求に答える。拒否された側は、相手が始めた session が開くのを期限まで待つ。
  - `reset` で世代を終え、この交渉が作った session をすべて閉じる。古い世代の応答では session を作らない（W1 AC-3 の世代・W4 の freeze から呼ぶ）。
  - DataChannel が開いたら、両端で `Endpoint::add_remote_addrs(相手, {custom addr})` を呼び、相手への生きた接続に custom path を足す（選ばれた path が custom へ移る）。
    生きた接続が無いときは path が開かず、後の接続がその custom path を使う保証も無い（手元の試験で、交渉の後に張った接続の 27/400 回が期限までに custom へ移らなかった。未使用のアドレスの対応は #4447 で回収される）。
    このため交渉は、需要の接続がある相手に対して行う（需要の owner への接続は AC-2）。需要の接続を先に張った試験では 400/400 回、接続が custom へ移り、以後の読み出しの実データも custom path を通った。
  - `IrohDocsNode` は `NodeOptions::webrtc` を渡したときだけ、この ALPN を Router に登録する。

### 8. AC-2 の固定 workload と判定

| ID | 組 | workload | 判定 |
| --- | --- | --- | --- |
| E1 | native↔native（str0m 同士）、browser↔browser（同じ page の 2 つの PeerConnection）、browser↔native | custom のアドレスだけで接続し、1 MiB の bi stream を往復 | bytes が一致、相手の EndpointId が一致 |
| E2 | 同上 | 1452 byte の datagram を transport へ 1000 回渡す | 受け取った datagram の境界と中身が一致。損失数を記録する |
| E3 | native↔native、browser↔native | 受信側の読み出しを止めて 8 MiB を送る | Rust 側の待ちが 64 以下、`bufferedAmount` が 1 MiB＋1 datagram 以下。捨てた datagram の数を記録し、読み出しの再開後に QUIC の再送で完走して bytes が一致 |
| E4 | 同上 | 転送の途中で session を close | session・socket・task が 0 に戻る。閉じた session の callback で何も再登録されない |
| E5 | native の既存の transport の test | 既存の UDP・relay の試験 | 変更前と同じく成功 |
| E6 | native↔native | fixture で DataChannel の送信の 5% を捨てて、1 MiB の bi stream を往復 | 完走して bytes が一致。所要時間と QUIC の再送数を記録する |

browser の組は headless の Chromium で実行する。browser↔native の SDP の受け渡しは試験の中の fixture が行い、製品の経路に残さない（製品の交渉は W10）。
Firefox・Safari の実測は W8 の matrix が扱う。

経路の切替と回線の全断の固定 workload と判定は、ここで先に決め、実行は W10 AC-2 が所有する（#4565 と需要の owner が要るため）。

| ID | 組 | workload | 判定 |
| --- | --- | --- | --- |
| S1 | browser↔native（relay と custom path の両方が開いている） | 4 MiB の blob の転送の途中で custom の session だけを close する | 同じ QUIC 接続のまま relay へ移って完走し、hash が一致。新しい接続・二重の適用が無い |
| S2 | 同上 | relay の接続だけで始め、接続交渉の後に custom path を足す（#4565） | 再接続なしで custom path が選ばれ、以後の実データが custom を通る（経路ごとの bytes で判定） |
| S3 | 同上 | 回線を全断して戻す | 既存の有界な再開へ戻り、未送信を成功として扱わない。旧 session の資源が 0 に戻る |

## 採らない方式

- 外部の iroh 用 WebRTC transport の crate を使う: Context。維持の状態と iroh の版が合わず、上限と停止の契約を足す差分がかえって大きくなる。
- native でも webrtc-rs を使う: str0m は Sans-IO で socket と task の上限を kukuri 側で持てる。webrtc-rs は内部で task を起こす。
- str0m の aws-lc-rs backend: Windows で C の build 環境が増える。
- 1 message に複数の datagram を詰める、独自の分割・再組立て: 層が増え、上限の管理も増える。
- 独自の path selector: 既定の selector で優先順位が満たされる。
- 公開 STUN: 第三者へ IP を送ることになる（§6）。

## Consequences

- iroh の Custom Transport は不安定な API なので、iroh の fork rev を更新するときは本 crate の build と AC-2 の試験を必ず通す。
- 同じ Endpoint に transport が 1 つ増える。native 同士では custom の session を作らないので経路は変わらない。GSO の batch 数は全 transport の最小値なので、UDP の値が 64 以下（Linux の GSO 等）ならそのまま使われ、Windows で USO が使える場合（512）は 64 に下がる。影響は性能だけで、E5 と W8 の計測で確認する。
- DataChannel の上で QUIC を運ぶため、暗号化と輻輳制御が重なる。計測値は AC-2 と W8 に記録する。

## Data classification

ADR 0002 の template に従う。

- Feature 名: QUIC over WebRTC DataChannel の transport
- Durable / Transient: すべて Transient（session、SDP、ICE の候補と credential、custom のアドレス）。runtime の世代で保持・回収する。
- Canonical Source: なし（その場の接続交渉の結果）。
- Replicated?: しない。アカウント同期・QR の移行・端末の保存・公開の索引へ複製しない（#1213）。
- Rebuildable From: 接続交渉のやり直し。
- Public Replica / Private Replica / Local Only: Local Only。SDP と候補は、認証済みの iroh 接続で接続交渉の相手へだけ送る。
- Gossip Hint 必要有無: なし。
- Blob 必要有無: なし。
- SQLite projection 必要有無: なし。
- 必須 contract: §8 の E1〜E6（S1〜S3 は W10 AC-2）。
- 必須 scenario: W8（#1220）の native↔Web・Web↔Web の直接経路と fallback。
- 新しい外部送信: STUN の要求（IP とポートが STUN の運用者、すなわち Community Node の運用者へ届く）、ICE の候補（IP とポートが接続交渉の相手へ届く）。
  外部送信の一覧（`docs/legal/app-data-flow-inventory.md`・`docs/legal/external-transmission-notice.md`）への反映は W10 と W8 AC-6 が行う。

## References

- Issue #1213（統括）、#1421（本 ADR）、#1422、#1214、ADR 0055・0056
- iroh の Custom Transport: https://www.iroh.computer/blog/iroh-0-97-0-custom-transports-and-noq 、iroh PR #4565
- RFC 8831 §6.1（DataChannel の信頼性の設定）、WebRTC 1.0（`RTCPeerConnection`）
- str0m: https://github.com/algesten/str0m

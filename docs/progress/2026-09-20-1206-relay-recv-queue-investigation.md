# #1206 relay 受信 queue の packet 破棄 調査記録

- 日付: 2026-09-20
- Scope revision: 2026-09-20-v1
- 基準 commit: ee9ee382
- 状態: 経路と容量は特定済み。**現場で queue が溢れた引き金は未再現・未確定**。

## 1. 特定できた事実

| 項目 | 内容 | 根拠 |
| --- | --- | --- |
| 破棄する側(producer) | `ActiveRelayActor::handle_relay_msg` が relay の WebSocket から読んだ datagram を `try_send` し、満杯なら警告を出して最新側を捨てる | iroh 1.0.3 `src/socket/transports/relay/actor.rs:681-687` |
| queue | `RelayTransport::new` の `mpsc::channel(512)`。固定値で設定不可。1 要素は GSO でまとめた複数 datagram を含みうる | iroh 1.0.3 `src/socket/transports/relay.rs:54` |
| 読む側(consumer) | 1 本だけ。`EndpointDriver::poll` → `drive_recv` → `Transports::poll_recv` → `RelayTransport::poll_recv`。1 回の poll は 50µs または 160 回で打ち切り、再 wake する | noq 1.1.1 `src/endpoint.rs:453-490`、`src/lib.rs:136,144` |
| 報告版と現行の差 | なし。v0.2.6-preview.1〜v0.2.8-preview.2 と main はすべて iroh 1.0.3 / noq 1.1.1 | 各 tag の `Cargo.lock` |
| upstream | main(v1.2.0 以降)でも該当箇所は同一。依存更新では変わらない | n0-computer/iroh `iroh/src/socket/transports/relay/actor.rs` |
| 観測件数 | 489 件は下限。export の先頭 489 行がすべてこの警告で `older_lines_dropped: yes` | #1176 添付ログ |
| ログ保持 | ring buffer は同一行をまとめず、drop 警告と `remote_fetch` 1393 件(#1207 所有)で約 21 分ぶんしか残らなかった | `apps/desktop/src-tauri/src/tracing.rs` |
| desktop の endpoint | `crates/iroh-node/src/node.rs:285` と `crates/transport/src/iroh/endpoint.rs:141` の 2 箇所で endpoint を構築する。それぞれが独立した 512 の受信 queue を持つ | 同 path |

## 2. 再現実験(`crates/transport/src/iroh/tests/relay_overload.rs`)

構成: ローカル relay server、IP transport を除いた endpoint(`Relay Fallback` を強制)。受信側は専用の
tokio runtime(worker 2 本)に置き、全 worker を `std::sync::mpsc` の recv で止めてから再開する。
drop は受信側 runtime の thread にだけ設定した tracing subscriber で数える。計数器は毎回、同じ
target・message の警告を 1 件流して拾えることを確認してから使う(陽性対照)。

| 条件 | build | 稼働中の drop | 停止→再開で増えた drop | 送信側 lost_packets | 転送完了 |
| --- | --- | --- | --- | --- | --- |
| sender 1、4MiB warm-up + 16×1MiB | debug | 0 | 0 | 373〜424 | 完了 |
| sender 8、各 4MiB warm-up + 16×1MiB | debug | 399 | 0 | 3878 | 完了 |
| sender 1 | release | 0 | 0 | 5292 | 完了 |
| sender 8 | release | 0 | 0 | 3778 | 完了 |

上の表は調査時の負荷(warm-up 4MiB、16×1MiB、sender 最大 8)での値である。repo に残した test は、
全 workspace の並行実行で他の test の CPU を奪わないよう、warm-up 2MiB・8×512KiB・sender 最大 4 に
軽くしてある。軽量版でも drop は 0 件、転送は完了する。

読み取れること:

- 受信側 runtime の全停止 → 再開は、どの条件でも drop を 1 件も増やさなかった。停止中は ACK が返らず、
  送信側の cwnd と flow control が in-flight を抑える(sender 1 で約 600 datagram)。
  **「OS 復帰・runtime 全停止が引き金」という当初の最有力仮説は、この実験では支持されない。**
- debug build の sender 8 では、consumer が動いている最中に 399 件の drop が出た。ただし release build では
  0 件であり、unoptimized な consumer の遅さによるものと考えられる。配布物は release build なので、
  これを現場の原因とは言えない。
- 送信側の `lost_packets` は受信 queue の drop が 0 件でも数千件ある。損失は受信 queue 以外
  (relay server の client 別送信 queue など)でも起きている。場所は未特定。
- すべての条件で転送は完了した。drop が起きても QUIC の再送で復旧する。

### 追加の探索(repo には残していない)

受信中に endpoint を close し、同じ secret key で bind し直す操作を、sender 8・release build で 5 回繰り返した
(#1208 以前の self-heal による stack 再構築を模したもの)。drop は 5 回とも 0 件だった。
iroh の `Socket::close`(`src/socket.rs:1133-1197`)は、接続の draining が終わるまで consumer を動かし続け、
`closed` を立てるのは actor を止めた後なので、graceful close 中に consumer だけが止まる窓は読み取れない。

## 2.1 v0.2.8-preview.2 から main(ee9ee382)までの commit との関係

| commit | 関係 | 内容 |
| --- | --- | --- |
| df2fd79e (#1208) | 診断に効く。負荷の低減は未確認 | 旧版は、設定済み peer がいる topic で peer 0 が 30 秒続くたびに `force_rebuild_runtime_connectivity_assist` で iroh stack を作り直していた。完了ログは target `kukuri_desktop_runtime` の info で、既定 filter では buffer に残らない。#1208 は local docs actor が健全なら再構築せず、`kukuri_connectivity` target で stack の世代を既定 filter に通す。次に drop が出たとき、再構築と時刻を突き合わせられる。ただし上記の探索では、再構築そのものは drop を起こさなかった |
| e08743fb (#1223) | 負荷の低減。drop への効果は未確認 | `crates/transport/src/peers.rs` に peer 別の取得 backoff(2〜60 秒)と 16 回/秒の上限を追加。`crates/iroh-node/src/remote_fetch.rs` 経由で desktop の blob 取得にも効く。添付ログの取得失敗 1393 件(約 17 分、同一 peer)のような接続試行の連発を抑える。`cn-iroh-relay` 側は既存の受信 rate limit を型付き policy へ寄せただけで、上限値は変えていない |
| b302f942 (#1209) / 49d1cb5c (#1227) | 無関係 | CN indexer 側 |
| 0cd98697 (#1226) / ee9ee382 (#1228) / 361791d3 (#1191) ほか | 無関係 | UI、AppImage、MSIX、release 手順、文書 |

iroh / noq の version はこの区間でも変わっていない。受信 queue と consumer に直接触れる commit は無い。
添付ログの `fetch remote transfer failed` 288 件はすべて `stream reset by peer: error 3` であり、
大きな blob の転送が繰り返されていたわけではない。

## 3. 未確定事項

- 現場(release build、2026-09-18 15:32:54 UTC)で 2ms に 489 件以上の drop が出た引き金。
  直前のログは buffer から失われており、ログからは決められない。
- 未検証の候補: 実 RTT 下で大きく育った cwnd を持つ多数接続の同時受信、consumer 側 mutex
  (`EndpointInner::state`)の競合、2 つの endpoint が同じ runtime を共有することによる影響、
  relay server 側での滞留後の一括送出。いずれも根拠はまだ無い。
- ローカルの loopback relay では release build で再現できていないため、AC-2 の
  「同じ原因を最小の失敗 test で再現」は現時点で満たせていない。

## 4. 今回の変更で保証できること

- AC-3: 同一警告の連発は ring buffer 上で 1 行に集約され、件数・最初と最後の時刻が残る。
  他の行を押し出さないので、次に発生したときは直前の文脈が残る(原因特定の材料になる)。
- TR-1 / TR-2 の回帰保護: release・debug とも、受信側 runtime 停止後に relay 経由の転送が再開・完了する。

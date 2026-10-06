# #1596 公開blob提供者発見PoCと採用判断

対象は[#1596](https://github.com/kukuri-app/kukuri/issues/1596)のScope revision `2026-10-05-r1`、AC-1 / PR-1とINVAR-1〜4。
実装基準は`c836489dfcc9244f7e87f352f1490349dd6c7e4d`。#1594の候補失効・再選択は完了済みであり、
既知peerが提供できる通常ケースをDHTの効果へ算入しない。Current statusはIssueへ集約する。

## データ分類と実験境界

- Feature 名: nativeの合成公開blobに限ったMainline提供者発見PoC。
- Durable / Transient: Transient。試験中の署名recordとprovider announceのみ。
- Canonical Source: 明示選択した合成bytesとそのBLAKE3 hash。発見結果は保持・権限の正本ではない。
- Replicated?: 試験Testnetのprovider記録と自前indexの署名recordだけ。
- Rebuildable From: 同じ合成fixtureの生成・明示告知。
- Public Replica / Private Replica / Local Only: 合成公開fixtureのみ。送信先はloopback Testnet/index。
- Gossip Hint 必要有無: 不要。
- Blob 必要有無: 標準SDKとmemory cacheの2種類。元のhashで取得bytesを検証する。
- SQLite projection 必要有無: 不要。cache-only提供は試験memory SQLiteを用いる。
- 必須contract: `provider_discovery` moduleの実QUIC往復、gate/取消/期限、固定候補窓、shared clientの取消制約。
- 必須scenario: 取得者の既知候補が空→候補発見→既存表示再試行で取得。標準SDK保持とcache-only保持を分ける。

利用者の投稿、実cache一覧、private/DM hash、実account secretは入力にしない。試験Endpointの鍵は生成したものだけ。
indexの保持は最大4行・1行1KiB以下・TTL60秒。infohashは固定合成blobについてだけone-shotで告知する。
定常announceやprovider cache、別retry基盤を追加しない。Testnetは試験終了で破棄する。

## 有限調査の結果

| 調査 | 判定と根拠 |
| --- | --- |
| S1: revisionと型/source互換 | 接続可能。上流`b4493bae93be89d654c0263190870ebb49cd9922`を固定し、現行Cargo.lockを基準にした一時crateのcheck成功。iroh 1.3.0のfork `e0b0ad9892e8881480d0f48a74a3148c8b896d79`、iroh-base/dns/relay、iroh-blobs 0.103.0を維持。n0-mainlineは追加crateの最低要件により0.7.0→0.7.1が必要。hashは32bytesから上流のblake3::Hashへ変換する。 |
| S2: stream・予算・取消 | `resolve_stream`はlazyで、上流のindex並行lookupは16、peer待ち窓は64を閾値とする。PoCはprefix 4件だけを既存候補台帳へ入れ、実転送は次の既存試行へ回す。全件collect版`resolve`や常時反復版は使わない。共有actorの取消と内部queueは別途判定する。 |
| S3: cache-only | gatewayの標準size probeを流用しない。候補EndpointIdから既存の`prepare_display_fetch`へ進み、SDK欠損/拒否後の独自配信fallbackを使用する。 |
| S4: address・公開範囲 | MainlineのSHA-1(BLAKE3 hash)→providerのUDP socket→署名付きEndpointId→通常のEndpointId address lookup→QUICの順。indexキーのsocketへblobを接続しない。自前indexでも公開Mainline上のhash/IP対応の公開は解消しない。browserの直接UDP方式は対象外。 |

根拠は[記事](https://www.iroh.computer/blog/iroh-global-content-discovery)、
[固定revisionのREADME](https://github.com/n0-computer/iroh-content-discovery/blob/b4493bae93be89d654c0263190870ebb49cd9922/iroh-mainline-endpoint-discovery/README.md)、
[resolver](https://github.com/n0-computer/iroh-content-discovery/blob/b4493bae93be89d654c0263190870ebb49cd9922/iroh-mainline-endpoint-discovery/src/resolver.rs)、
[gateway probe](https://github.com/n0-computer/iroh-content-discovery/blob/b4493bae93be89d654c0263190870ebb49cd9922/iroh-link-gateway/src/providers.rs)。
既存address lookupの仕様はADR 0008のままにする。

## 接続と検証方法

PoCはnative test moduleだけへ閉じ、productionの取得入口・公開設定・配布には接続しない。
`probe`はgateを先に確認し、既存表示取得が成功すればlookupしない。未取得の場合だけ、同じ30秒の残時間で
既存`NetworkWorkRuntime`の枠を取得して候補を更新する。新しい候補の転送を同じ試行へ追加しない。
表示需要の既存再試行（最初は5秒後）で更新した候補を使い、hash検証後のbytesを返す。

試験のprovider lookup contextは需要に所有させ、streamとindex/DHTのhandleを同時に解放する。
取消後のUDP socket再bindと、既存ownerの全枠再取得で解放を確認する。
これを、共有DHT/index clientに対してstreamだけdropすれば安全なことの証明には使わない。

ローカルの実行入口:

```powershell
cargo test --locked -p kukuri-iroh-node provider_discovery -- --nocapture
cargo test --locked -p kukuri-iroh-node remote_ -- --nocapture
cargo test --locked -p kukuri-transport dht -- --nocapture
cargo clippy --locked -p kukuri-iroh-node -p kukuri-transport --all-targets -- -D warnings
```

## 判定・採用条件

ローカルPoCは4件のtestがPASSし、次の成立/不成立を観測した。testのPASSは共有actorの採用条件達成を意味しない。

| 条件 | 観測結果 | 判定範囲 |
| --- | --- | --- |
| T1: SDK保持、未知provider | DHT/indexから2候補応答（同じEndpointId）、実転送1試行、40 bytesをBLAKE3検証。direct 40 bytes、relay 0。SDKへの保存なし。 | loopback/Testnetで成立 |
| T2: cache-only保持 | 2候補応答、既存SDK→独自cache fallbackの実転送1試行、39 bytesをBLAKE3検証。direct 39 bytes、relay 0。SDKへの保存なし。 | loopback/Testnetで成立 |
| 既知候補成功 | 次の既存表示試行で成功すると、新規lookupの増分0。 | 成立 |
| gate不許可 | lookup 0、候補0、provider取得観測なし。 | 合成公開fixtureの許可境界で成立 |
| 需要取消 | index要求が始まった後に取得futureをdropし、lookup用UDP socketの再bindと既存ownerの全枠再取得が成功。 | 需要所有contextで成立 |
| 総期限 | 設定30秒、仮想時間で取消・全枠解放。Tokio timerのms丸めを含む観測は30〜30.001秒。期限設定は延長していない。 | 既存owner/期限で成立 |
| T4: 候補20/200/2000件 | すべて読取り4・採用4・peer試行4・採用EndpointId bytes 128・取得bytes 39。 | adapterの窓/保持/試行が候補総数に比例しない |
| shared index client取消 | 呼出しfutureを1回pollしてactorへ委譲した直後にdropしても、その後1,200 bytesのUDP要求を観測。 | **共有clientの取消条件は不成立** |

依存更新後も、関連remote取得25件、既存DHT関連5件、変更crateのclippyを確認する。
固定head独立監査とPR CIの結果はPRへ集約する。

Testnet/loopbackでの成功を公開MainlineやNAT越えの
到達性・性能の確認済みとは扱わない。20/200/2000件はlazyな制御候補streamの試験であり、公開DHTへ2000件を
announceする実験ではない。実データはrelayを無効にしたnative Endpoint間で取得し、direct/relayとbytesを記録する。

上流[n0-mainline 0.7.1のget_peers](https://github.com/n0-computer/n0-mainline/blob/d170dab53c1e10bb41eac552f1ed3a2a23aba0e4/src/dht.rs#L260)はunbounded channelを返す。`AddrIndex`のshared UDP clientはactorへ
操作を委譲し、呼出しfutureのdropがqueued request/内部予約を取消すAPIを持たない。
[上流UDP actor](https://github.com/n0-computer/iroh-content-discovery/blob/b4493bae93be89d654c0263190870ebb49cd9922/iroh-mainline-endpoint-discovery/src/udp.rs)の
incoming queueは256、command queueは32だが、pending mapには明示上限がない。
adapterのtake(4)の成功で、上流内部queueの上限や共有actorの取消を確認済みと扱わない。

本番採用では、公開hash/IP対応の公開を許容する利用条件、native限定の扱い、内部queueの有界化、
共有DHT/index処理の取消と需要ownerとの接続、公開Mainline/実ネットワークでの確認が必要。
本Issueで自動的にそれらの修正・公開・既定ONを決定しない。根拠が揃った段階でユーザーに採用可否を確認する。

現時点の推奨は**本番採用の保留**。未知providerから取得できる技術的な効果は確認できたが、
共有actorの取消と内部queueの上限が現行原則を満たさず、公開Mainline・NAT越えは未確認である。
需要所有contextの成功を、共有actorへそのまま接続する採用根拠に置き換えない。

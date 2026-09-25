# #1221 R5-B: client の通常読取りを有界ページへ移す

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R5-B。区分 C、基準は
#1361 merge `ae0d5950`。R5-B の一つの PR で、通常 timeline/thread と bookmark/CN source、
投稿・返信・reaction・取り下げ・session の対象参照を同じ有界な docs 読取りへ接続する。
public/private の現行・直前・指定過去 bucket、および writer 切替前の旧形式を扱う。
1操作は最大4 bucket、局所照合200行、応答1MiB、record 64KiB、全体30秒。
remote 読取りで namespace を import/open/start_sync しない。

author/profile と private の参加・epoch 制御参照は R5-C、writer 切替と旧定常sync撤去は
R5-H、CN の手動取込と保存回収は R5-E/F の担当。未取得は未解決として返し、全件取得や
旧版との新着相互運用を保証しない。

## 入口と副作用

| 経路 | 対象入力と結果 | 禁止する副作用・確認 |
| --- | --- | --- |
| 通常一覧 | timeline/thread の cursor と表示scopeから、該当する現行・直前・指定過去の key 窓を読む。署名済み対象だけを既存 projection/cacheへ反映し、欠損を残して表示を続ける | 全namespace同期・全履歴照合なし。4 bucket/200行/30秒以内。旧writerの投稿もページから表示できる |
| 対象参照 | bookmark/CN source、投稿・返信・reaction・取り下げ・session が持つ対象ID・保存元で exact key を読む | 投稿scope、著者署名、時刻・bucket、取り下げを既存gateで確かめる。遅着した旧投稿で撤回済み本文を復活させない |
| private | 参加中のchannelと許可epochの capability だけから private bucket/旧replicaの provider に要求する | 退出・epoch失効後の要求/保存0。公開providerや公開routeへ private secret/hash を流さない |
| 旧形式 | 移行中に残る topic/channel replicaを同じ有限ページ/対象読取りで扱う | 新旧の別scheduler/adapterを完成形に残さず、writer切替前の通常利用を止めない |

## 実装・検証の対応

- `crates/iroh-node` / `crates/docs-sync`: #1341 のQUIC読取りをbucketと旧形式の共通
  keyページ・exact対象へ接続する。要求側は単一providerの短いleaseを使い、取得した
  recordは検証後だけR5-Aの保存先へ渡す。prefix走査の全件化とoffsetでの深いskipを使わない。
- `crates/app-api`: `community_index.rs` の既存source解決を共有し、
  `replica_window.rs`、`object_hydration.rs`、`reply_target_support.rs`、reaction/
  withdrawal/sessionの対象経路を同じ所有者へ集約する。表示・保存前のscope世代と
  LocalOnlyの操作境界を維持する。
- 実Irohの2client（privateは参加者/非参加者）で現行・直前・指定過去bucket、
  旧形式、欠損、scope/署名/時刻不一致、退出/epoch失効、撤回後の遅着を確認する。
  局所テストは変更関連だけ、全体suiteはR5-BのPR CIで行う。固定head独立監査の
  blockerが0なら終了する。

## 実装の照合（監査前）

- 旧 `topic::` / `channel::` と新bucketを同じQUIC page/exact protocolへ接続。privateは
  epoch secretの証明と当該gossip scopeの最大4 peerだけで読み、公開側へsecretを渡さない。
- timeline/threadはcursorから現行・隣接・指定過去bucketを最大4候補で選び、局所とremoteを
  合計200行の照合予算と30秒期限に収める。旧writerのローカル窓を維持し、privateの
  archived epochは全履歴の複製をせず時刻で選ぶ。1 objectの検証後にremote leaseを解放する。
- CN source、返信先、投稿の署名済み対象、live/game sessionの対象を同じremote readerへ接続。
  既存の署名・scope・bucket時刻・取り下げgateを通してからprojectionへ保存し、private退出後は
  要求・保存を中止する。bookmark/reaction等の操作は既存の対象projectionを使う。
- 実Irohのclient/providerでpublic current/previous/historic、private current/archived、
  旧形式、thread返信先、bookmark/取り下げ、session対象、退出とnamespace非importを確認。
  欠損・署名・時刻・scopeの負例は関連する単体testで確認する。最終判定は固定head監査とPR CI後。

R5-Cのauthor/private制御参照、R5-Hの新writer・旧定常sync撤去は既定の所有者に残る。
このPRはそれらのwriter切替や新旧相互運用の保証を追加しない。

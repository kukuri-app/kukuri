# ADR 0054: docs replica の時間分割と有限な作業集合

## Status

Proposed（#1243、実装計画承認済み。各段階の実装・監査後に採用状態を更新する）

## Context

[ADR 0052 §7](0052-scale-independent-timeline-sync.md) の残る問題は、iroh-docs の同期が namespace の
全 entry を対象とし、単一の topic/channel/author replica に累積履歴が入っていることである。
query を上限つきにしても、同期と保存の量は減らない。

本 ADR は #1243 の AC-1〜5 / INVAR-1〜4 を所有する。既存の反映の3経路と、
[ADR 0053](0053-account-derived-docs-author.md) の署名検証・docs author 指定の読みを再利用する。
接続先・同時同期数・購読の選択を統合する owner は #1224。本 ADR はその owner に渡す
replica の識別、取得理由、解放、保存の回収を定める。

## Decision

### 1. 識別と時間

- v1 の時間単位は UTC の1日（86,400秒）。bucket は Unix 秒を86,400で割った非負整数。
  常時同期の候補は現在と直前の2 bucket。将来の粒度変更は別の版として導入し、既存IDの意味を変えない。
- 新IDは旧形式と重ならない `bucket::v1::` の接頭辞を持つ。
  public: `bucket::v1::topic::<topic UTF-8の小文字hex>::<bucket>`。
  private: `bucket::v1::channel::<channel UTF-8の小文字hex>::<epoch UTF-8の小文字hex>::<bucket>`。
  author: `bucket::v1::author::<pubkey文字列のUTF-8の小文字hex>::<bucket>`。
- 空のscope、非canonicalなhex/整数、未知版、余分な要素は拒否する。旧topic IDに `::` が含まれても曖昧にならない。
  scopeの各文字列は1〜1,024 UTF-8 byteとし、decode前にもIDのbytes上限を検査する。
  数値は `i64` の非負Unix秒に対応するbucketだけを許す。上限bucketの終端は排他的な `u64` として扱う。
- 未知/不正な `bucket::` ID は public namespace の秘密を導出しない。private は必ず登録した capability を要する。
  private bucket secret はepoch secretをkeyとするBLAKE3 keyed hashから導出する。入力は用途の固定文字列とcanonical replica ID。
  用途の固定bytesはUTF-8の `kukuri.app docs bucket namespace v1` と終端NUL1byteで、その直後にIDのUTF-8を連結する。
  異なるepoch/channel/bucketで鍵を共有しない。派生した秘密はwire/logへ出さない。
- 新規操作は操作開始時にbucketを一度決め、outboxへ保存する。途中で日付が変わっても再試行先を変えない。
  新規投稿の署名済み `created_at` からbucketを導出する。受信側はbucketとの一致を検証する。
  未来10分超の新着は窓へ反映しない（ADR 0052）。過去の署名済み投稿は過去bucketに属し、現在bucketへ読み替えない。
  時計逆行やoffline期間に比例してbucketを開かない。未知版へ黙って旧形式fallbackしない。

### 2. record の配置

| record | 正本の配置 | 現在の購読者への発見 |
| --- | --- | --- |
| 投稿・返信・repost、state/envelope、timeline/thread/profile索引 | 作成時のscope bucket。返信はrootの古いbucketへ追記しない | そのbucketのevent/hint |
| 投稿のmedia manifest | 投稿と同じbucket。refはsource locatorを持つ | 投稿と同じ |
| reaction | reaction作成時のbucket。元投稿のbucketへ無期限追記しない | 現在bucketのeventと対象locator |
| 取り下げ | 元投稿bucketの `withdrawals/<id>/state`（署名済みenvelopeを同じkeyへ上書き） | 操作時bucketにも同じ取り下げとtarget locatorを置く |
| live/game/Dome等の継続状態 | entity別の最新state。更新履歴envelopeを永久に積まない | 更新時bucketへ署名済みlocatorを置く |
| author profile/latest | author別の固定数の最新state/envelopeの制御領域 | 更新時author bucketのevent |
| follow/block | author/target/種別から決定できる対象別最新state | 更新時author bucketのevent。全edgeのコピー/再生はしない |
| author asset/preset | IDから引ける対象別stateとcontent-addressed blob | 作成/更新時author bucketの索引 |

取り下げ以外のeventを元投稿bucketへ蓄積しない。取り下げは元投稿1件あたり高々1keyで、
generationごとにkeyを増やさないため、元bucketの正当なentry数はその期間の投稿数の定数倍で抑えられる。
第三者がnamespaceへ不正entryを積む問題は時間分割だけでは防げない。アプリの署名検証を
iroh-docsの受信前filterと同一視しない。

現在bucketと元bucketへの取り下げ書込みは永続outboxで再開する。対象がprivateなら元epochのcapabilityで書き、
現在epochへの通知は現在のaudience guardを別途通す。秘密や本文を公開scopeへ移さない。
どちらかの書込み失敗を成功としてoutboxから落とさない。世代の重複は冪等に反映する。

### 3. 参照と遡り

- 新しいref/hint/indexは、版、scope、privateならepoch、bucket、object/entity ID、著者/docs authorの手がかりを持つ。
  locatorは探索の手がかりであり権限の証明ではない。署名済みenvelopeとscopeを照合してから反映する。
- objectの既存 `source_replica_id` は保持する。profileやrepostからtopic IDだけで旧replicaを再構成しない。
  旧refは既存ローカルprojection/保存envelopeから解決し、分からなければ取得不能として返す。全bucketを探さない。
- timeline/profile/thread cursorはbucketとbucket内の既存cursor、方向、版を持つ。
  1回で読むbucketは最大4、各bucketの読みはADR 0052の既存上限内。空bucketにも同じ上限を適用し、続き位置を返す。
  空ページを履歴の終端と誤認しない。epochの一覧も全件展開せず、許可された範囲をcursorで進む。
- 古いbucketの取得は明示的な遡り・対象参照・取り下げ照会に限り要求する。表示側は保存済みprojectionを先に返す。
  peer不在/期限切れは取得不能として表示し、taskのcancelや次ページ操作を妨げない。
- 元投稿を反映するときは同bucketの取り下げを著者/docs authorとkeyで確認する。既知の取り下げは本文/添付を隠す。
  現在bucketの取り下げeventは購読中のprojectionへ反映する。購読外の投稿は既存の背景確認経路を使う。
  ネットワーク全体への伝播保証は追加しない（ADR 0053 Context）。不正候補があるだけで本文を伏せない（同§5）。
- 古いreactionの完全回復は行わない。既存の上限つき反映とeventのbest effortを維持する。
  authorの窓外edgeも全回復しない（ADR 0053 §6）。不明なedgeをunfollow/unblockとして保存しない。
  mutual/friend-only等で特定edgeが必要なら対象別stateを上限つきに取得し、未確認状態で権限を広げない。
- 過去に更新したedgeの発見は、author制御領域に署名付きの固定サイズrosterを持つ。follow/block各512件まで、
  既存のkey窓と同じ選択順でtarget locatorを列挙する。更新時bucketだけを見て、長期未更新の著者の全edgeを消さない。
  書込み側は本人storeの索引をLIMIT512で読み、全edgeの列挙・sortをしない。rosterも固定keyへ上書きし、
  更新履歴を制御領域へ積まない。受信側のstate取得は作業集合へ小分けに登録し、512replicaを一斉に開かない。
  rosterの欠損・範囲外をedge削除と解釈しない。profile/latestはrosterと独立の固定keyで、無操作期間に関わらず取得できる。
  自分を指すfollow/blockの2対象は、既存同様にroster外でも個別に確認する。
  restore等で本人storeが部分的な状態なら、部分rosterで保存済みの署名済みrosterを無条件に上書きしない。

### 4. lifecycle と #1224 の接点

- `LocalOnly`の読み取りはnamespaceをネットワークへ参加させない。ローカルopenとsync開始を別操作にする。
  secretの登録だけでもsyncを開始しない。remote要求はaudience/同意/成人向け取得guardを通してからownerへ渡す。
- 要求はreplica ID、理由（表示中/書込み/遡り/背景確認）、scope、期限を持つleaseとする。
  #1224のownerが同時同期数、peer数、優先順位、台帳上限を所有する。時間分割側に並列の無制限ownerを作らない。
- bucket境界は作業集合内のscopeだけを差分更新する。現在/直前の候補は「全scopeの2bucketを必ず同期する」の意味ではない。
  全履歴・全登録replicaを列挙して境界処理や再接続をしない。
- 最後のlease解放時はsyncを止め、event転送taskを停止し、handleを閉じる。保存済みnamespaceの削除とは分離する。
  restart/seed更新が休止したbucketを再開してはならない。古いbucketのleaseも同じ上限内で扱う。
- close/revokeはcallerの待機cancel後もownerが完了する。capabilityの削除だけをcaller側で先に実行しない。
  leave失敗時は対象をquery/restartから隔離して明示再試行し、close済みDocをcacheへ戻さない。
  closeと背景restartは直列化し、古いrestartが新しいhandleを削除/再開する経路を持たない。
- replica作業集合の実装が無い段階では新形式の本番書込みを有効化しない。#1224との統合contractを切替の前提にする。

### 5. 保存と回収

- 再取得可能なremote cacheはentry本体・索引・blob・projectionを含めて予算管理する。
  本人の投稿、bookmark、参加状態、未送信outboxは保護データ。保護データを自動削除して予算を達成しない。
  cacheの有限性と利用者が明示的に増やす保護データの量を別の指標にする。
- 本人の投稿が1件あるだけでbucket全体を保護しない。保護するobjectと依存blobを個別に永続保管し、
  他者のcacheを回収できる参照索引を持つ。blobの共有参照が残っていれば削除しない。
- 回収候補は保存時に索引へ登録し、期限/予算超過の索引から最大128件ずつ処理する。
  namespaceの破棄、blobの回収、projectionの整理の進捗を永続化し、中断から再開する。
  bytesの合計を毎回全件SUMで求めず、増減を台帳へ反映する。
- 対象別stateのnamespace metadata、派生secret、peer記録もremote cacheの対象に含める。
  epochの元capabilityは保護し、派生secretは必要なときに導出し直せるため休止/回収時に解放する。
- 予算超過時は新しいremote cacheの取得を延期し、保護データは残す。保存不足で本人の操作を完了できない場合は
  成功扱いにせず回復可能なエラーを返す。cache予算の具体値と計数方法はT6のcontractで固定する。

### 6. 移行

| 状態 | 許可する動作 | 禁止する動作 |
| --- | --- | --- |
| 旧形式 | 既存の読み書き | 新形式への片側だけの切替 |
| 読取り準備 | CNが新旧を認識、clientが新refを解決。新形式のwriterはまだ無効 | 旧clientに新形式が読めるとの扱い |
| 書込み切替 | 端末に切替状態を保存し、新操作を新形式へ。未完了outboxは記録済みの宛先へ再開 | 再起動ごとの切替や二重投稿 |
| 新形式定常 | 新規参加/起動は作業集合の新bucketだけ。旧保存データはローカルから利用 | 旧replicaの常時同期、旧履歴の起動時全コピー |

- CNの新旧readiness、clientの読取り、#1224の作業集合、保存/回収、対応する移行testの成立後にwriterを切り替える。
  本番での実施順序と実施日は別のrollout Issueが所有する。新形式のwriterを旧CNへ先行配布しない。
- 旧replicaの自動互換同期は書込み切替まで。定常状態では、利用者が旧範囲を必要としたときだけ有限の取得要求を出す。
  旧namespace1つの同期量自体が旧履歴に比例することは残るため、これを新規参加/起動の成功経路に含めない。
- 旧端末が切替後に旧形式へ書いた投稿は、新版へ自動で全件取り込む保証を持たない。
  旧版との無期限の完全相互運用とAC-3の両立は要求しない。旧保存データのうち保護対象は削除しない。
  旧remote cacheは保護対象の個別退避確認後に小分けに回収できる。移行前cacheを永久保護して予算を迂回しない。
- 切替状態/outbox/cursorはbackupに含め、restoreでも旧常時同期へ戻さない。
  切替後のrollbackは新形式を読める版へのrollbackに限る。旧版を黙って書込み可能にしない。
- publicの旧履歴が新形式へコピー済みという印を、全件同期/再索引の代わりに要求しない。
  移行中の未送信操作の保存・冪等再開と、P2Pの完全配送を保証しない契約を混同しない。

### 7. 検証と停止条件

- 過去bucketを増やして累積投稿数を10倍にし、現在/直前bucketの密度は一定とする。
  常時同期のentry数、起動・同期開始で読むentry数、handle/task数をassertする。
  空bucketの連続、時刻不正、途中失敗、再起動、回収中断でも1操作の上限を確認する。
- privateの異なるepoch、未登録secret、LocalOnly、同意不足、mixed scopeで禁止I/Oが0であることを確認する。
- CN→clientの移行、取り下げの2箇所書込みの片側失敗、既存bookmark/本人投稿の回収除外をcontractにする。
- 独立監査と必須CIの両方を満たすまで#1243をCloseしない。基盤だけを追加した段階のPRは `Refs #1243` とする。

## データ分類

- public topic/author bucketは既存の公開投稿・profile/graphと同じ公開範囲。bucket時刻とlocatorは新しい同期metadata。
- private bucket/locator/stateはepoch capabilityの範囲内。鍵の公開導出、公開gossipへのprivate locator送信は禁止。
- outbox、移行位置、lease、cache予算、保護参照索引は端末内の制御state。CNへ本人の保存一覧を送らない。
- CNは許可scopeの索引を継続する。全体の権威・private capabilityの配布ownerにはしない。
- CNの新bucketでは、投稿の値と配置を署名済みenvelopeから確認する。unsigned stateは発見用の印であり、
  書き換え・破損・別bucketへのコピーを別の保存先で確認済みの索引の削除根拠にしない。
  新形式のjob revisionと判定再利用鍵は検証済み署名IDとし、印やJSON表記の変更だけでは外部再検査しない。
  現物envelopeがscan中に取得不能または検証不能になった場合は一時的失敗として再評価する。これはADR 0025の旧形式の
  state破損時の扱いを新形式へそのまま適用しない決定である。署名済み取り下げ、送信防止、scope除外、
  safety verdictの優先度は維持する。
- 通信優先度は Direct P2P → Relay Supported P2P → Relay Fallback のまま。

## References

- [#1243 作業記録](../progress/2026-09-22-1243-replica-time-buckets.md)
- [replica read inventory](../architecture/replica-read-inventory.md)
- [Issue lifecycle](../runbooks/issue-lifecycle.md)
